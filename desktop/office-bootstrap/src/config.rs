//! Server configuration: one JSON file (path via `--config` or `KEYZAPPER_OFFICE_CONFIG`). It references the
//! deployment the desktop app exported and says where credentials come from. Secrets are never in this file and
//! never on the command line: they are read from environment variables or owner-only files named here.

use crate::logging::LogFormat;
use keyzapper_core::office::{LiteLlmVirtualKeyProvider, OfficeCredentialProvider, OfficeDeployment, StaticKeyProvider};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_ENV: &str = "KEYZAPPER_OFFICE_CONFIG";
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
const DEFAULT_JWKS_CACHE_SECONDS: u64 = 3600;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerConfig {
    /// `host:port` to bind; loopback by default (put a TLS-terminating proxy in front).
    #[serde(default = "default_listen")]
    pub listen: String,
    /// The export of the desktop app, inline …
    #[serde(default)]
    pub deployment: Option<OfficeDeployment>,
    /// … or as a separate file (relative to this config file).
    #[serde(default)]
    pub deployment_file: Option<PathBuf>,
    pub credential_provider: CredentialProviderConfig,
    /// Override of the tenant's JWKS URL (tests, mirrors); default derived from the tenant.
    #[serde(default)]
    pub jwks_url: Option<String>,
    #[serde(default = "default_jwks_cache")]
    pub jwks_cache_seconds: u64,
    /// Scopes (`scp`) the token must carry, e.g. when the manifest sets `entra_scope`.
    #[serde(default)]
    pub required_scopes: Vec<String>,
    /// Environment variable with the salt for pseudonymous user IDs in logs and key metadata. Without it a random
    /// salt per process is used (IDs then differ between restarts).
    #[serde(default)]
    pub log_salt_env: Option<String>,
    #[serde(default)]
    pub log_format: LogFormat,
}

fn default_listen() -> String {
    DEFAULT_LISTEN.to_string()
}

fn default_jwks_cache() -> u64 {
    DEFAULT_JWKS_CACHE_SECONDS
}

/// Where the add-in's gateway token comes from.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum CredentialProviderConfig {
    /// Short-lived LiteLLM virtual keys via `/key/generate` (recommended). The management key comes from the
    /// environment variable or file named here.
    #[serde(rename_all = "camelCase")]
    LitellmVirtualKey {
        #[serde(default)]
        management_key_env: Option<String>,
        #[serde(default)]
        management_key_file: Option<PathBuf>,
        /// LiteLLM to issue keys on; default: each profile's own endpoint.
        #[serde(default)]
        endpoint: Option<String>,
        /// Spend cap per vended key.
        #[serde(default)]
        max_budget: Option<f64>,
    },
    /// The profiles' long-lived keys from an owner-only JSON file `{"<profileId>": "sk-…"}`. Every Office user of
    /// a profile shares that key; use only where virtual keys are not possible.
    #[serde(rename_all = "camelCase")]
    StaticProfileKeys { keys_file: PathBuf },
}

/// Everything the server needs after loading, with secrets already resolved into the provider.
pub struct Loaded {
    pub config: ServerConfig,
    pub deployment: OfficeDeployment,
    pub provider: Box<dyn OfficeCredentialProvider>,
    pub log_salt: String,
}

fn read_secret_file(path: &Path) -> Result<String, String> {
    warn_if_readable_by_others(path);
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let secret = text.trim().to_string();
    if secret.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    Ok(secret)
}

#[cfg(unix)]
fn warn_if_readable_by_others(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            tracing::warn!(file = %path.display(), "secret file is readable by other users; use mode 0600");
        }
    }
}

#[cfg(not(unix))]
fn warn_if_readable_by_others(_path: &Path) {}

fn secret_from(env_name: Option<&str>, file: Option<&Path>, what: &str) -> Result<String, String> {
    if let Some(name) = env_name {
        let value = std::env::var(name).map_err(|_| format!("environment variable {name} ({what}) is not set"))?;
        let value = value.trim().to_string();
        if value.is_empty() {
            return Err(format!("environment variable {name} ({what}) is empty"));
        }
        return Ok(value);
    }
    if let Some(file) = file {
        return read_secret_file(file);
    }
    Err(format!("{what}: set managementKeyEnv or managementKeyFile"))
}

impl ServerConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("invalid configuration: {e}"))
    }

    /// Reads the file, resolves the deployment and the credential provider (including its secret).
    pub fn load(path: &Path) -> Result<Loaded, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let config = Self::parse(&text)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        config.resolve(base)
    }

    pub fn resolve(self, base: &Path) -> Result<Loaded, String> {
        let deployment = match (&self.deployment, &self.deployment_file) {
            (Some(inline), None) => inline.clone(),
            (None, Some(file)) => {
                let path = if file.is_absolute() { file.clone() } else { base.join(file) };
                let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                serde_json::from_str(&text).map_err(|e| format!("invalid deployment file {}: {e}", path.display()))?
            }
            (Some(_), Some(_)) => return Err("set either deployment or deploymentFile, not both".into()),
            (None, None) => return Err("deployment or deploymentFile is required".into()),
        };
        let problems = deployment.validate();
        if !problems.is_empty() {
            return Err(format!("deployment is not usable: {}", problems.join("; ")));
        }
        let provider = self.provider(base)?;
        let log_salt = match &self.log_salt_env {
            Some(name) => std::env::var(name).map_err(|_| format!("environment variable {name} (logSaltEnv) is not set"))?,
            None => uuid::Uuid::new_v4().to_string(),
        };
        Ok(Loaded { config: self, deployment, provider, log_salt })
    }

    fn provider(&self, base: &Path) -> Result<Box<dyn OfficeCredentialProvider>, String> {
        let absolute = |p: &Path| if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
        match &self.credential_provider {
            CredentialProviderConfig::LitellmVirtualKey { management_key_env, management_key_file, endpoint, max_budget } => {
                let file = management_key_file.as_deref().map(absolute);
                let key = secret_from(management_key_env.as_deref(), file.as_deref(), "LiteLLM management key")?;
                if let Some(endpoint) = endpoint {
                    if !keyzapper_core::managed::is_secure_endpoint(endpoint) {
                        return Err("credentialProvider.endpoint must be an https URL".into());
                    }
                }
                let mut provider = LiteLlmVirtualKeyProvider::new(key);
                provider.endpoint = endpoint.clone();
                provider.max_budget = *max_budget;
                Ok(Box::new(provider))
            }
            CredentialProviderConfig::StaticProfileKeys { keys_file } => {
                let path = absolute(keys_file);
                let text = read_secret_file(&path)?;
                let keys: BTreeMap<String, String> = serde_json::from_str(&text).map_err(|_| format!("{} is not a JSON object of profile IDs to keys", path.display()))?;
                tracing::warn!("static profile keys in use: every Office user of a profile shares its key; prefer litellmVirtualKey");
                Ok(Box::new(StaticKeyProvider::new(keys)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config_and_rejects_unknown_fields() {
        let config = ServerConfig::parse(r#"{"deploymentFile": "deployment.json", "credentialProvider": {"type": "litellmVirtualKey", "managementKeyEnv": "KZ_MGMT"}}"#).unwrap();
        assert_eq!(config.listen, DEFAULT_LISTEN);
        assert_eq!(config.jwks_cache_seconds, DEFAULT_JWKS_CACHE_SECONDS);
        assert!(matches!(config.credential_provider, CredentialProviderConfig::LitellmVirtualKey { .. }));
        assert!(ServerConfig::parse(r#"{"credentialProvider": {"type": "litellmVirtualKey", "managementKey": "sk-inline"}}"#).is_err());
        assert!(ServerConfig::parse(r#"{"credentialProvider": {"type": "staticProfileKeys", "keysFile": "k.json"}, "managementKey": "x"}"#).is_err());
    }

    #[test]
    fn missing_secret_source_fails_closed() {
        let config = ServerConfig::parse(r#"{"deploymentFile": "nope.json", "credentialProvider": {"type": "litellmVirtualKey"}}"#).unwrap();
        let error = config.resolve(Path::new("/nonexistent")).err().expect("missing deployment file must fail");
        assert!(error.contains("cannot read"), "{error}");
        let error = secret_from(Some("KEYZAPPER_TEST_UNSET_VARIABLE"), None, "x").unwrap_err();
        assert!(error.contains("not set"));
        assert!(secret_from(None, None, "x").is_err());
    }
}
