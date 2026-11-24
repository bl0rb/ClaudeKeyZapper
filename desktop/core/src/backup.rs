//! `.kzbackup` file, byte-compatible with the macOS app: AES-256-GCM, key derived with PBKDF2-HMAC-SHA256
//! from the NFC-normalised password and a random salt. The envelope parameters are authenticated as
//! associated data, so tampering with them fails decryption.

use crate::l;
use crate::models::AppState;
use aes_gcm::aead::{Aead, Generate, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

pub const FILE_EXTENSION: &str = "kzbackup";
pub const MINIMUM_PASSWORD_LENGTH: usize = 12;
pub const DEFAULT_ITERATIONS: u32 = 600_000;
const FORMAT: &str = "keyzapper-backup";

/// Contents of a password-protected backup: metadata plus (optionally) the keys, by profile ID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPayload {
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub state: AppState,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq)]
pub enum BackupError {
    PasswordTooShort,
    NotABackup,
    UnsupportedVersion,
    WrongPasswordOrCorrupt,
    Corrupt,
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            BackupError::PasswordTooShort => l!("Das Passwort muss mindestens %@ Zeichen haben.", MINIMUM_PASSWORD_LENGTH),
            BackupError::NotABackup => l!("Die Datei ist kein KeyZapper-Backup."),
            BackupError::UnsupportedVersion => l!("Dieses Backup stammt von einer neueren KeyZapper-Version. Bitte App aktualisieren."),
            BackupError::WrongPasswordOrCorrupt => l!("Falsches Passwort oder beschädigte Backup-Datei."),
            BackupError::Corrupt => l!("Die Backup-Datei ist beschädigt."),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    cipher: String,
    format: String,
    iterations: u32,
    kdf: String,
    salt: String,
    sealed: String,
    version: u32,
}

impl Envelope {
    fn associated_data(&self) -> String {
        format!("{}|{}|{}|{}|{}|{}", self.format, self.version, self.kdf, self.iterations, self.salt, self.cipher)
    }
}

fn derive_key(password: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let normalized: String = password.nfc().collect();
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(normalized.as_bytes(), salt, iterations, &mut key);
    key
}

pub fn seal(payload: &BackupPayload, password: &str, iterations: u32) -> Result<Vec<u8>, BackupError> {
    if password.nfc().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(BackupError::PasswordTooShort);
    }
    let salt = <[u8; 16]>::generate();
    let mut envelope = Envelope {
        cipher: "aes-256-gcm".into(),
        format: FORMAT.into(),
        iterations,
        kdf: "pbkdf2-sha256".into(),
        salt: B64.encode(salt),
        sealed: String::new(),
        version: 1,
    };
    let key = derive_key(password, &salt, iterations);
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let nonce = Nonce::generate();
    let plaintext = serde_json::to_vec(payload).map_err(|_| BackupError::Corrupt)?;
    let aad = envelope.associated_data();
    let ciphertext = cipher.encrypt(&nonce, Payload { msg: &plaintext, aad: aad.as_bytes() }).map_err(|_| BackupError::Corrupt)?;
    // CryptoKit's combined representation: nonce ‖ ciphertext ‖ tag.
    envelope.sealed = B64.encode([nonce.as_slice(), &ciphertext].concat());
    serde_json::to_vec_pretty(&envelope).map_err(|_| BackupError::Corrupt)
}

pub fn open(data: &[u8], password: &str) -> Result<BackupPayload, BackupError> {
    let envelope: Envelope = serde_json::from_slice(data).map_err(|_| BackupError::NotABackup)?;
    if envelope.format != FORMAT {
        return Err(BackupError::NotABackup);
    }
    if envelope.version != 1 || envelope.kdf != "pbkdf2-sha256" || envelope.cipher != "aes-256-gcm" {
        return Err(BackupError::UnsupportedVersion);
    }
    let salt = B64.decode(&envelope.salt).map_err(|_| BackupError::Corrupt)?;
    // Bounds guard against hostile files that would stall the KDF.
    if !(100_000..=10_000_000).contains(&envelope.iterations) || salt.len() < 16 {
        return Err(BackupError::Corrupt);
    }
    let sealed = B64.decode(&envelope.sealed).map_err(|_| BackupError::WrongPasswordOrCorrupt)?;
    if sealed.len() < 12 + 16 {
        return Err(BackupError::WrongPasswordOrCorrupt);
    }
    let key = derive_key(password, &salt, envelope.iterations);
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let (nonce, ciphertext) = sealed.split_at(12);
    let nonce = Nonce::try_from(nonce).map_err(|_| BackupError::WrongPasswordOrCorrupt)?;
    let aad = envelope.associated_data();
    let plaintext = cipher
        .decrypt(&nonce, Payload { msg: ciphertext, aad: aad.as_bytes() })
        .map_err(|_| BackupError::WrongPasswordOrCorrupt)?;
    let mut payload: BackupPayload = serde_json::from_slice(&plaintext).map_err(|_| BackupError::Corrupt)?;
    payload.state = payload.state.normalized();
    Ok(payload)
}
