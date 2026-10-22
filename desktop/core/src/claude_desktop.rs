//! Claude Desktop with a KeyZapper profile. Claude Desktop's third-party mode reads one configuration file from its
//! configuration library (`Claude-3p/configLibrary/<id>.json`, `_meta.json` names the applied one). KeyZapper keeps
//! one entry there: requests go to the profile's gateway, and Claude Desktop gets the key or SSO token from
//! `keyzapper-helper` (`inferenceCredentialHelper`), just like Claude Code. A profile with a `DesktopSso` block instead
//! lets Claude Desktop sign in itself with the gateway's own OIDC client (`inferenceCredentialKind: external-idp`,
//! or the older `interactive` spelling for Claude Desktop before 2.7032.0); then no key or token is involved at
//! all. IT can add further Claude Desktop keys to the block (`config`) and have KeyZapper apply it without a click
//! (`auto_apply`). A configuration IT deploys via MDM profile or registry policy that sets more than the app-behaviour
//! keys takes precedence over the library, so KeyZapper does not write one then.

use crate::models::Profile;
use crate::{json, l, paths};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// ID of KeyZapper's entry in the configuration library.
pub const ENTRY_ID: &str = "ca2e9fe9-a839-47f6-81cc-7229ac419d96";
const META_FILE: &str = "_meta.json";
const TTL_ENV: &str = "CLAUDE_CODE_API_KEY_HELPER_TTL_MS";
/// Claude Code's default helper TTL; KeyZapper's SSO tokens are refreshed with this margin.
const DEFAULT_TTL_SECS: u64 = 300;
/// Keys that only steer the app itself (updates, restarts, proxy). A managed source that sets nothing else leaves the local
/// configuration in effect (Anthropic's precedence rule).
const APP_BEHAVIOUR_KEYS: [&str; 8] = [
    "disableAutoUpdates", "autoUpdaterEnforcementHours", "updateViaUpdatesHost", "releaseChannel", "relaunchEnforcementHours",
    "configRecheckIntervalMinutes", "egressProxyUrl", "egressProxyPacUrl",
];
/// Keys KeyZapper derives from the block; the same keys in `config` are left out.
const DERIVED_KEYS: [&str; 5] = ["inferenceProvider", "inferenceGatewayBaseUrl", "inferenceCredentialKind", "inferenceIdpOidc", "inferenceGatewayOidc"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// KeyZapper profile in KeyZapper's entry, if there is one.
    pub profile_id: Option<String>,
    /// KeyZapper's entry is the one Claude Desktop applies.
    pub applied: bool,
    /// IT deploys a Claude Desktop configuration; the library may then be ignored.
    pub managed: bool,
}

/// The token Claude Desktop sends to the gateway as bearer (`bearerTokenType` of the OIDC block).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearerTokenType {
    #[default]
    IdToken,
    AccessToken,
}

/// How the sign-in block is spelled for Claude Desktop. `ExternalIdp` (`inferenceCredentialKind: external-idp` with
/// `inferenceIdpOidc`) is read from Claude Desktop 2.7032.0 on; `Interactive` (`interactive` with `inferenceGatewayOidc`) is the
/// older spelling every version reads. The fields of both blocks are identical. `Unknown` is an unrecognised `CredentialKind`
/// from IT; `problems()` reports it and nothing is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    #[default]
    ExternalIdp,
    Interactive,
    #[serde(other)]
    Unknown,
}

/// Claude Desktop signs in itself with the gateway's OIDC client, which may differ from the one Claude Code uses
/// (another client ID, another gateway route). A field missing in a stored object takes its empty default;
/// `problems()` then reports the block as unusable.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSso {
    /// Gateway route for Claude Desktop; empty = the profile's endpoint.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub gateway_url: String,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    /// Space-separated; empty = Claude Desktop's default (`openid profile email offline_access`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scopes: String,
    pub bearer_token_type: BearerTokenType,
    /// Spelling of the block; a stored block without it (KeyZapper 1.5.0) takes the default.
    pub credential_kind: CredentialKind,
    /// Further Claude Desktop configuration keys from IT, written into the entry as they are (`Config`).
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub config: Map<String, Value>,
    /// IT: KeyZapper writes and applies the entry without a click and restores it at start and refresh (`AutoApply`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_apply: bool,
}

/// A `config` key KeyZapper does not hand on: secrets and headers (they can carry tokens), local programs or credential
/// files, and bootstrap delivery.
fn is_rejected_key(key: &str) -> bool {
    let key = key.to_lowercase();
    ["apikey", "secret", "password", "token", "headers", "credentialsfile"].iter().any(|end| key.ends_with(end))
        || key.starts_with("inferencecredentialhelper")
        || key.starts_with("bootstrap")
        || key == "trustbootstrapdelivery"
}

/// An https URL with host and without credentials.
fn is_https_url(text: &str) -> bool {
    url::Url::parse(text).is_ok_and(|u| u.scheme() == "https" && u.host().is_some() && u.username().is_empty() && u.password().is_none())
}

impl DesktopSso {
    /// The gateway Claude Desktop talks to.
    pub fn gateway<'a>(&'a self, profile: &'a Profile) -> &'a str {
        if self.gateway_url.is_empty() { &profile.endpoint } else { &self.gateway_url }
    }

    /// What keeps this block from being used; empty means usable.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.gateway_url.is_empty() && !is_https_url(&self.gateway_url) {
            problems.push(l!("Die Gateway-URL für Claude Desktop muss eine https-URL mit Host sein."));
        }
        if !is_https_url(&self.oidc_issuer) || self.oidc_issuer.contains("/.well-known/") {
            problems.push(l!("Der OIDC-Issuer für Claude Desktop muss eine https-URL ohne /.well-known/… sein."));
        }
        if self.oidc_client_id.is_empty() || self.oidc_client_id.chars().any(char::is_whitespace) {
            problems.push(l!("Die OIDC-Client-ID für Claude Desktop darf nicht leer sein und keine Leerzeichen enthalten."));
        }
        if self.bearer_token_type == BearerTokenType::AccessToken && self.scopes.trim().is_empty() {
            problems.push(l!("Mit Access-Token braucht Claude Desktop die Scopes des Gateways."));
        }
        if self.credential_kind == CredentialKind::Unknown {
            problems.push(l!("Die Anmeldeart für Claude Desktop (CredentialKind) muss „external-idp“ oder „interactive“ sein."));
        }
        for key in self.config.keys().filter(|key| is_rejected_key(key)) {
            problems.push(l!("Claude Desktop: „%@“ verteilt KeyZapper nicht (Geheimnis, Header, Programm oder Bootstrap).", key));
        }
        problems
    }
}

/// `~/Library/Application Support/Claude-3p/configLibrary` (macOS), `%LOCALAPPDATA%\Claude-3p\configLibrary` (Windows).
pub fn library_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(local).join(r"Claude-3p\configLibrary");
    }
    #[cfg(target_os = "macos")]
    return paths::home_dir().join("Library/Application Support/Claude-3p/configLibrary");
    #[allow(unreachable_code)]
    paths::home_dir().join(".config/Claude-3p/configLibrary")
}

/// Gateway provider with `keyzapper-helper` as credential helper; the profile's model tiers become the model list
/// (Sonnet first, the default), else Claude Desktop asks the gateway (`/v1/models`). With a `DesktopSso` block, the
/// OIDC sign-in instead.
pub fn configuration(profile: &Profile, helper: &str) -> Value {
    if let Some(sso) = &profile.claude_desktop {
        return sso_configuration(profile, sso);
    }
    let ttl = profile
        .environment
        .as_ref()
        .and_then(|env| env.get(TTL_ENV))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|ms| (ms / 1000).max(60))
        .unwrap_or(DEFAULT_TTL_SECS);
    let mut config = json!({
        "inferenceProvider": "gateway",
        "inferenceGatewayBaseUrl": profile.endpoint,
        "inferenceCredentialKind": "helper-script",
        "inferenceCredentialHelper": helper,
        "inferenceCredentialHelperArgs": ["credential", "--profile", profile.id],
        "inferenceCredentialHelperTtlSec": ttl,
    });
    let tiers = [("sonnet", &profile.sonnet_model), ("opus", &profile.opus_model), ("haiku", &profile.haiku_model)];
    let models: Vec<Value> = tiers
        .into_iter()
        .filter_map(|(tier, model)| Some(json!({"name": model.as_deref().map(str::trim).filter(|m| !m.is_empty())?, "anthropicFamilyTier": tier})))
        .collect();
    if !models.is_empty() {
        config["inferenceModels"] = Value::Array(models);
    }
    config
}

/// Sign-in with the gateway's OIDC client: Claude Desktop signs in in the browser, keeps the refresh token itself and lists the
/// gateway's models (`/v1/models`), which follow the user's access. `external-idp` with `inferenceIdpOidc`, or with
/// `CredentialKind::Interactive` the older spelling `interactive` with `inferenceGatewayOidc`.
fn sso_configuration(profile: &Profile, sso: &DesktopSso) -> Value {
    let mut oidc = json!({
        "issuer": sso.oidc_issuer,
        "clientId": sso.oidc_client_id,
        "bearerTokenType": sso.bearer_token_type,
        "appendOfflineAccess": true,
    });
    if !sso.scopes.trim().is_empty() {
        oidc["scopes"] = sso.scopes.trim().into();
    }
    // `Unknown` never gets here: `problems()` stops it before `apply`.
    let (kind, key) = match sso.credential_kind {
        CredentialKind::Interactive => ("interactive", "inferenceGatewayOidc"),
        CredentialKind::ExternalIdp | CredentialKind::Unknown => ("external-idp", "inferenceIdpOidc"),
    };
    // IT's further keys first; the derived ones win, so `config` cannot redirect the sign-in or mix both spellings.
    let mut config: Map<String, Value> = sso.config.iter().filter(|(key, _)| !DERIVED_KEYS.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    config.insert("inferenceProvider".into(), "gateway".into());
    config.insert("inferenceGatewayBaseUrl".into(), sso.gateway(profile).into());
    config.insert("inferenceCredentialKind".into(), kind.into());
    config.insert(key.into(), oidc);
    Value::Object(config)
}

fn read(path: &Path) -> Option<json::Object> {
    json::parse_lenient(&std::fs::read_to_string(path).ok()?)
}

fn write(path: &Path, value: &Value) -> std::io::Result<()> {
    let data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    paths::write_atomic(path, &data, true)
}

/// Writes KeyZapper's entry for `profile` and makes it the applied configuration. Takes effect when Claude Desktop starts.
pub fn apply(dir: &Path, profile: &Profile, helper: &str) -> std::io::Result<()> {
    write(&dir.join(format!("{ENTRY_ID}.json")), &configuration(profile, helper))?;
    let meta_path = dir.join(META_FILE);
    let mut meta = read(&meta_path).unwrap_or_default();
    let mut entries: Vec<Value> = meta.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
    entries.retain(|e| e.get("id").and_then(Value::as_str) != Some(ENTRY_ID));
    entries.push(json!({"id": ENTRY_ID, "name": format!("KeyZapper – {}", profile.name)}));
    meta.insert("entries".into(), Value::Array(entries));
    meta.insert("appliedId".into(), ENTRY_ID.into());
    write(&meta_path, &Value::Object(meta))
}

/// The configuration Claude Desktop applies now (`appliedId`), if any.
pub fn applied_id(dir: &Path) -> Option<String> {
    read(&dir.join(META_FILE))?.get("appliedId")?.as_str().map(str::to_string)
}

/// KeyZapper's entry holds exactly `config` and is the applied one.
pub fn is_current(dir: &Path, config: &Value) -> bool {
    let listed = read(&dir.join(META_FILE)).and_then(|meta| meta.get("entries")?.as_array().cloned()).is_some_and(|entries| {
        entries.iter().any(|e| e.get("id").and_then(Value::as_str) == Some(ENTRY_ID))
    });
    listed && applied_id(dir).as_deref() == Some(ENTRY_ID) && read(&dir.join(format!("{ENTRY_ID}.json"))).map(Value::Object).as_ref() == Some(config)
}

/// Removes KeyZapper's entry. If it was the applied one, `previous` (what was applied before KeyZapper's) is applied again
/// when its file is still there; otherwise none is, and Claude Desktop starts with its normal sign-in. Other entries stay.
pub fn remove(dir: &Path, previous: Option<&str>) -> std::io::Result<()> {
    match std::fs::remove_file(dir.join(format!("{ENTRY_ID}.json"))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let meta_path = dir.join(META_FILE);
    let Some(mut meta) = read(&meta_path) else { return Ok(()) };
    if let Some(entries) = meta.get_mut("entries").and_then(Value::as_array_mut) {
        entries.retain(|e| e.get("id").and_then(Value::as_str) != Some(ENTRY_ID));
    }
    if meta.get("appliedId").and_then(Value::as_str) == Some(ENTRY_ID) {
        let safe = |id: &&str| !id.is_empty() && *id != ENTRY_ID && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        let previous = previous.filter(safe).filter(|id| dir.join(format!("{id}.json")).is_file());
        meta.insert("appliedId".into(), previous.map_or(Value::Null, Value::from));
    }
    write(&meta_path, &Value::Object(meta))
}

/// `recorded` is the profile KeyZapper last wrote its entry for: the sign-in variant has no helper arguments that name it.
pub fn status(dir: &Path, recorded: Option<&str>) -> Status {
    let config = read(&dir.join(format!("{ENTRY_ID}.json")));
    let from_helper = config.as_ref().and_then(|config| {
        let args = config.get("inferenceCredentialHelperArgs")?.as_array()?;
        let at = args.iter().position(|a| a == "--profile")?;
        args.get(at + 1)?.as_str().map(str::to_string)
    });
    let signed_in = config
        .as_ref()
        .filter(|config| ["inferenceIdpOidc", "inferenceGatewayOidc"].iter().any(|key| config.contains_key(*key)))
        .and(recorded)
        .map(str::to_string);
    let profile_id = from_helper.or(signed_in);
    let applied = profile_id.is_some() && read(&dir.join(META_FILE)).and_then(|m| m.get("appliedId")?.as_str().map(str::to_string)).as_deref() == Some(ENTRY_ID);
    Status { profile_id, applied, managed: managed_by_it() }
}

/// The managed keys set more than the app-behaviour ones: then Claude Desktop ignores the local library.
pub fn configures_claude_desktop<'a>(keys: impl IntoIterator<Item = &'a str>) -> bool {
    keys.into_iter().any(|key| !APP_BEHAVIOUR_KEYS.contains(&key) && !key.starts_with("Payload"))
}

/// IT configures Claude Desktop through an MDM profile (`com.anthropic.claudefordesktop`) or registry policy
/// (`SOFTWARE\Policies\Claude`) with more than the app-behaviour keys.
fn managed_by_it() -> bool {
    managed_keys().is_some_and(|keys| configures_claude_desktop(keys.iter().map(String::as_str)))
}

/// The key names of the managed configuration, None without one. An unreadable plist counts as configuring everything.
#[cfg(target_os = "macos")]
fn managed_keys() -> Option<Vec<String>> {
    let domain = "com.anthropic.claudefordesktop.plist";
    let user = std::env::var("USER").unwrap_or_default();
    let base = Path::new("/Library/Managed Preferences");
    let files: Vec<PathBuf> = [Some(base.join(domain)), (!user.is_empty()).then(|| base.join(&user).join(domain))].into_iter().flatten().filter(|f| f.is_file()).collect();
    if files.is_empty() {
        return None;
    }
    Some(files.iter().flat_map(|file| plist_keys(file).unwrap_or_else(|| vec![String::from("?")])).collect())
}

/// Top-level key names of a plist file, read with `plutil` (part of macOS); None if it cannot be read as JSON.
#[cfg(target_os = "macos")]
fn plist_keys(file: &Path) -> Option<Vec<String>> {
    let output = std::process::Command::new("/usr/bin/plutil").args(["-convert", "json", "-o", "-"]).arg(file).output().ok()?;
    let value: Value = serde_json::from_slice(&output.stdout).ok().filter(|_| output.status.success())?;
    Some(value.as_object()?.keys().cloned().collect())
}

/// Value names directly under `HKLM` or, without machine policy (which hides user policy completely), `HKCU`.
#[cfg(windows)]
fn managed_keys() -> Option<Vec<String>> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER].into_iter().find_map(|hive| policy_value_names(hive, r"SOFTWARE\Policies\Claude"))
}

#[cfg(windows)]
fn policy_value_names(hive: winreg::HKEY, subkey: &str) -> Option<Vec<String>> {
    let key = winreg::RegKey::predef(hive).open_subkey(subkey).ok()?;
    let names: Vec<String> = key.enum_values().filter_map(|value| value.ok().map(|(name, _)| name)).collect();
    (!names.is_empty()).then_some(names)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn managed_keys() -> Option<Vec<String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_uses_the_helper_and_the_model_tiers() {
        let mut profile = Profile::new("A1B2C3D4-0000-4000-8000-000000000001".into(), "Team".into(), "https://litellm.firma.example".into(), String::new());
        let config = configuration(&profile, "/Applications/KeyZapper.app/Contents/MacOS/keyzapper-helper");
        assert_eq!(config["inferenceProvider"], "gateway");
        assert_eq!(config["inferenceGatewayBaseUrl"], "https://litellm.firma.example");
        assert_eq!(config["inferenceCredentialKind"], "helper-script");
        assert_eq!(config["inferenceCredentialHelperArgs"], json!(["credential", "--profile", "A1B2C3D4-0000-4000-8000-000000000001"]));
        assert_eq!(config["inferenceCredentialHelperTtlSec"], 300);
        assert!(config.get("inferenceModels").is_none());
        profile.opus_model = Some("eu.opus".into());
        profile.sonnet_model = Some("eu.sonnet".into());
        profile.haiku_model = Some(" ".into());
        profile.environment = Some([(TTL_ENV.to_string(), "900000".to_string())].into());
        let config = configuration(&profile, "helper");
        assert_eq!(config["inferenceModels"], json!([{"name": "eu.sonnet", "anthropicFamilyTier": "sonnet"}, {"name": "eu.opus", "anthropicFamilyTier": "opus"}]));
        assert_eq!(config["inferenceCredentialHelperTtlSec"], 900);
    }

    #[test]
    fn a_desktop_sso_block_writes_the_external_idp_sign_in_without_any_secret() {
        let mut profile = Profile::new("A1B2C3D4-0000-4000-8000-000000000001".into(), "Team".into(), "https://llm.firma.example/code".into(), String::new());
        profile.sonnet_model = Some("eu.sonnet".into());
        let mut sso = DesktopSso {
            oidc_issuer: "https://login.microsoftonline.com/00000000-0000-4000-8000-000000000000/v2.0".into(),
            oidc_client_id: "11111111-2222-4333-8444-555555555555".into(),
            ..DesktopSso::default()
        };
        assert!(sso.problems().is_empty());
        profile.claude_desktop = Some(sso.clone());
        let config = configuration(&profile, "helper");
        assert_eq!(
            config,
            json!({
                "inferenceProvider": "gateway",
                "inferenceGatewayBaseUrl": "https://llm.firma.example/code",
                "inferenceCredentialKind": "external-idp",
                "inferenceIdpOidc": {
                    "issuer": "https://login.microsoftonline.com/00000000-0000-4000-8000-000000000000/v2.0",
                    "clientId": "11111111-2222-4333-8444-555555555555",
                    "bearerTokenType": "id_token",
                    "appendOfflineAccess": true,
                },
            })
        );
        // Own gateway route and access tokens with the gateway's scope.
        sso.gateway_url = "https://llm.firma.example/desktop".into();
        sso.bearer_token_type = BearerTokenType::AccessToken;
        assert_eq!(sso.problems().len(), 1, "access tokens need scopes");
        sso.scopes = " api://gateway/.default ".into();
        assert!(sso.problems().is_empty());
        profile.claude_desktop = Some(sso.clone());
        let config = configuration(&profile, "helper");
        assert_eq!(config["inferenceGatewayBaseUrl"], "https://llm.firma.example/desktop");
        assert_eq!(config["inferenceIdpOidc"]["bearerTokenType"], "access_token");
        assert_eq!(config["inferenceIdpOidc"]["scopes"], "api://gateway/.default");
        assert!(config.get("inferenceGatewayOidc").is_none());
        // No helper, no key, no token; the gateway lists the models itself.
        let text = config.to_string();
        assert!(!text.contains("helper") && !text.contains("inferenceModels") && !text.contains("eu.sonnet"));
        // Unusable values are reported, the metadata URI instead of the issuer too.
        let broken = DesktopSso { gateway_url: "http://x".into(), oidc_issuer: "https://login.example/.well-known/openid-configuration".into(), ..DesktopSso::default() };
        assert_eq!(broken.problems().len(), 3);

        // The entry names no profile: the recorded one is shown, as long as the entry is there.
        let dir = std::env::temp_dir().join(format!("keyzapper-claude-desktop-sso-{}", uuid::Uuid::new_v4()));
        apply(&dir, &profile, "helper").unwrap();
        assert_eq!(status(&dir, Some(profile.id.as_str())).profile_id.as_deref(), Some(profile.id.as_str()));
        assert_eq!(status(&dir, None).profile_id, None);
        remove(&dir, None).unwrap();
        assert_eq!(status(&dir, Some(profile.id.as_str())).profile_id, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_interactive_credential_kind_keeps_the_older_spelling() {
        let mut profile = Profile::new("A1B2C3D4-0000-4000-8000-000000000001".into(), "Team".into(), "https://llm.example.com/code".into(), String::new());
        let mut sso = DesktopSso {
            oidc_issuer: "https://login.example.com/00000000-0000-4000-8000-000000000000/v2.0".into(),
            oidc_client_id: "11111111-2222-4333-8444-555555555555".into(),
            credential_kind: CredentialKind::Interactive,
            ..DesktopSso::default()
        };
        assert!(sso.problems().is_empty());
        profile.claude_desktop = Some(sso.clone());
        let old = configuration(&profile, "helper");
        assert_eq!(
            old,
            json!({
                "inferenceProvider": "gateway",
                "inferenceGatewayBaseUrl": "https://llm.example.com/code",
                "inferenceCredentialKind": "interactive",
                "inferenceGatewayOidc": {
                    "issuer": "https://login.example.com/00000000-0000-4000-8000-000000000000/v2.0",
                    "clientId": "11111111-2222-4333-8444-555555555555",
                    "bearerTokenType": "id_token",
                    "appendOfflineAccess": true,
                },
            })
        );
        // Both spellings carry the same fields.
        sso.credential_kind = CredentialKind::ExternalIdp;
        profile.claude_desktop = Some(sso);
        let new = configuration(&profile, "helper");
        assert_eq!(new["inferenceCredentialKind"], "external-idp");
        assert_eq!(new["inferenceIdpOidc"], old["inferenceGatewayOidc"]);
    }

    #[test]
    fn status_recognises_both_spellings_of_the_sign_in_entry() {
        const ID: &str = "A1B2C3D4-0000-4000-8000-000000000001";
        let dir = std::env::temp_dir().join(format!("keyzapper-claude-desktop-kind-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(META_FILE), json!({"appliedId": ENTRY_ID, "entries": [{"id": ENTRY_ID, "name": "KeyZapper"}]}).to_string()).unwrap();
        let entry = dir.join(format!("{ENTRY_ID}.json"));
        for key in ["inferenceIdpOidc", "inferenceGatewayOidc"] {
            let config = json!({"inferenceProvider": "gateway", "inferenceGatewayBaseUrl": "https://llm.example.com", key: {"issuer": "https://login.example.com/t/v2.0", "clientId": "c1"}});
            std::fs::write(&entry, config.to_string()).unwrap();
            let found = status(&dir, Some(ID));
            assert_eq!((found.profile_id.as_deref(), found.applied), (Some(ID), true), "{key}");
            assert_eq!(status(&dir, None).profile_id, None, "{key}");
        }
        std::fs::write(&entry, r#"{"inferenceProvider":"gateway"}"#).unwrap();
        assert_eq!(status(&dir, Some(ID)).profile_id, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credential_kind_defaults_round_trips_and_flags_unknown_values() {
        // A block stored by KeyZapper 1.5.0 has no `credentialKind`.
        let stored: DesktopSso =
            serde_json::from_value(json!({"oidcIssuer": "https://login.example.com/t/v2.0", "oidcClientId": "c1", "bearerTokenType": "id_token"})).unwrap();
        assert_eq!(stored.credential_kind, CredentialKind::ExternalIdp);
        assert_eq!(serde_json::to_value(&stored).unwrap()["credentialKind"], "external-idp");
        let interactive = DesktopSso { credential_kind: CredentialKind::Interactive, ..stored.clone() };
        let value = serde_json::to_value(&interactive).unwrap();
        assert_eq!(value["credentialKind"], "interactive");
        assert_eq!(serde_json::from_value::<DesktopSso>(value).unwrap().credential_kind, CredentialKind::Interactive);
        let unknown: DesktopSso = serde_json::from_value(json!({"credentialKind": "bogus"})).unwrap();
        assert_eq!(unknown.credential_kind, CredentialKind::Unknown);
        // What `state.json` holds for it reads back as `Unknown`.
        let stored = serde_json::to_value(&unknown).unwrap();
        assert_eq!(stored["credentialKind"], "unknown");
        assert_eq!(serde_json::from_value::<DesktopSso>(stored).unwrap().credential_kind, CredentialKind::Unknown);
        // An unknown kind is the one problem; the same block with a known kind has none.
        let block = |credential_kind| DesktopSso {
            oidc_issuer: "https://login.example.com/t/v2.0".into(),
            oidc_client_id: "c1".into(),
            credential_kind,
            ..DesktopSso::default()
        };
        assert_eq!(block(CredentialKind::Unknown).problems().len(), 1);
        assert!(block(CredentialKind::ExternalIdp).problems().is_empty());
        assert!(block(CredentialKind::Interactive).problems().is_empty());
    }

    fn sso_profile(config: Value) -> Profile {
        let mut profile = Profile::new("A1B2C3D4-0000-4000-8000-000000000001".into(), "Team".into(), "https://llm.example.com/code".into(), String::new());
        profile.claude_desktop = Some(DesktopSso {
            oidc_issuer: "https://login.example.com/00000000-0000-4000-8000-000000000000/v2.0".into(),
            oidc_client_id: "11111111-2222-4333-8444-555555555555".into(),
            config: config.as_object().cloned().unwrap_or_default(),
            ..DesktopSso::default()
        });
        profile
    }

    #[test]
    fn it_adds_further_keys_but_cannot_override_the_sign_in_or_hand_on_secrets() {
        let profile = sso_profile(json!({
            "disableDeploymentModeChooser": true,
            "coworkEgressAllowedHosts": ["example.com"],
            "inferenceGatewayBaseUrl": "https://elsewhere.example.com",
            "inferenceGatewayOidc": {"issuer": "https://evil.example.com", "clientId": "x"},
        }));
        let sso = profile.claude_desktop.as_ref().unwrap();
        assert!(sso.problems().is_empty());
        let config = configuration(&profile, "helper");
        assert_eq!(config["disableDeploymentModeChooser"], true);
        assert_eq!(config["coworkEgressAllowedHosts"], json!(["example.com"]));
        // The derived keys win, and only one spelling of the sign-in block is written.
        assert_eq!(config["inferenceGatewayBaseUrl"], "https://llm.example.com/code");
        assert_eq!(config["inferenceCredentialKind"], "external-idp");
        assert!(config.get("inferenceGatewayOidc").is_none());
        // Secrets, headers, local programs and bootstrap delivery are refused; then nothing is written.
        for key in ["inferenceGatewayApiKey", "inferenceAnthropicApiKey", "someSecret", "otlpHeaders", "inferenceCredentialHelper", "awsCredentialsFile", "bootstrapUrl", "trustBootstrapDelivery"] {
            let profile = sso_profile(json!({ key: "x" }));
            assert_eq!(profile.claude_desktop.unwrap().problems().len(), 1, "{key}");
        }
        // `config` and `auto_apply` are only stored when set.
        let plain = serde_json::to_value(DesktopSso::default()).unwrap();
        assert!(plain.get("config").is_none() && plain.get("autoApply").is_none());
    }

    #[test]
    fn removing_restores_what_was_applied_before_and_is_current_tells_whether_to_write() {
        let dir = std::env::temp_dir().join(format!("keyzapper-claude-desktop-restore-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let own = "11111111-0000-4000-8000-000000000001";
        std::fs::write(dir.join(format!("{own}.json")), "{}").unwrap();
        std::fs::write(dir.join(META_FILE), json!({"appliedId": own, "entries": [{"id": own, "name": "Eigene"}]}).to_string()).unwrap();
        let profile = sso_profile(json!({"disableDeploymentModeChooser": true}));
        let config = configuration(&profile, "helper");
        assert!(!is_current(&dir, &config));
        let previous = applied_id(&dir);
        assert_eq!(previous.as_deref(), Some(own));
        apply(&dir, &profile, "helper").unwrap();
        assert!(is_current(&dir, &config));
        // An edited entry, or another one applied, is no longer current.
        std::fs::write(dir.join(format!("{ENTRY_ID}.json")), "{}").unwrap();
        assert!(!is_current(&dir, &config));
        apply(&dir, &profile, "helper").unwrap();
        // Removing applies the earlier configuration again.
        remove(&dir, previous.as_deref()).unwrap();
        assert_eq!(applied_id(&dir).as_deref(), Some(own));
        assert!(dir.join(format!("{own}.json")).is_file());
        // Without its file, or after someone else applied another one meanwhile, nothing is restored.
        apply(&dir, &profile, "helper").unwrap();
        std::fs::remove_file(dir.join(format!("{own}.json"))).unwrap();
        remove(&dir, Some(own)).unwrap();
        assert_eq!(applied_id(&dir), None);
        apply(&dir, &profile, "helper").unwrap();
        let mut meta = read(&dir.join(META_FILE)).unwrap();
        meta.insert("appliedId".into(), "other".into());
        write(&dir.join(META_FILE), &Value::Object(meta)).unwrap();
        remove(&dir, Some(own)).unwrap();
        assert_eq!(applied_id(&dir).as_deref(), Some("other"));
        // A previous ID with path syntax is never used.
        apply(&dir, &profile, "helper").unwrap();
        remove(&dir, Some("../x")).unwrap();
        assert_eq!(applied_id(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_more_than_the_app_behaviour_keys_counts_as_it_configuring_claude_desktop() {
        assert!(!configures_claude_desktop(["disableAutoUpdates", "releaseChannel", "egressProxyUrl"]));
        assert!(!configures_claude_desktop([]));
        assert!(configures_claude_desktop(["disableAutoUpdates", "disableNonessentialTelemetry"]));
        assert!(configures_claude_desktop(["inferenceProvider"]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plist_keys_reads_the_top_level_names() {
        let dir = std::env::temp_dir().join(format!("keyzapper-claude-desktop-plist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("com.anthropic.claudefordesktop.plist");
        std::fs::write(&file, r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>disableAutoUpdates</key><true/><key>releaseChannel</key><string>stable</string></dict></plist>"#).unwrap();
        let mut keys = plist_keys(&file).unwrap();
        keys.sort();
        assert_eq!(keys, ["disableAutoUpdates", "releaseChannel"]);
        assert!(!configures_claude_desktop(keys.iter().map(String::as_str)));
        std::fs::write(&file, "not a plist").unwrap();
        assert!(plist_keys(&file).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn policy_value_names_reads_the_registry_policy() {
        use winreg::enums::HKEY_CURRENT_USER;
        let subkey = format!(r"SOFTWARE\KeyZapperTest\Claude-{}", uuid::Uuid::new_v4());
        assert!(policy_value_names(HKEY_CURRENT_USER, &subkey).is_none());
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(&subkey).unwrap();
        assert!(policy_value_names(HKEY_CURRENT_USER, &subkey).is_none(), "an empty key is no policy");
        key.set_value("disableAutoUpdates", &1u32).unwrap();
        let names = policy_value_names(HKEY_CURRENT_USER, &subkey).unwrap();
        assert!(!configures_claude_desktop(names.iter().map(String::as_str)));
        key.set_value("disableNonessentialTelemetry", &1u32).unwrap();
        let names = policy_value_names(HKEY_CURRENT_USER, &subkey).unwrap();
        assert!(configures_claude_desktop(names.iter().map(String::as_str)));
        let _ = winreg::RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&subkey);
    }

    #[test]
    fn apply_and_remove_keep_other_entries() {
        let dir = std::env::temp_dir().join(format!("keyzapper-claude-desktop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(META_FILE), r#"{"appliedId": "other", "entries": [{"id": "other", "name": "Eigene"}]}"#).unwrap();
        assert_eq!(status(&dir, None).profile_id, None);
        let profile = Profile::new("A1B2C3D4-0000-4000-8000-000000000001".into(), "Team".into(), "https://litellm.firma.example".into(), String::new());
        apply(&dir, &profile, "helper").unwrap();
        apply(&dir, &profile, "helper").unwrap();
        let meta = read(&dir.join(META_FILE)).unwrap();
        assert_eq!(meta["appliedId"], ENTRY_ID);
        assert_eq!(meta["entries"], json!([{"id": "other", "name": "Eigene"}, {"id": ENTRY_ID, "name": "KeyZapper – Team"}]));
        let current = status(&dir, None);
        assert_eq!((current.profile_id.as_deref(), current.applied), (Some(profile.id.as_str()), true));
        remove(&dir, None).unwrap();
        let meta = read(&dir.join(META_FILE)).unwrap();
        assert_eq!((&meta["appliedId"], &meta["entries"]), (&Value::Null, &json!([{"id": "other", "name": "Eigene"}])));
        assert_eq!(status(&dir, None), Status { managed: status(&dir, None).managed, ..Status::default() });
        remove(&dir, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
