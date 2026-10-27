//! Settings IT can enforce. macOS: Intune "Preference file" profile for the domain `io.github.bl0rb.keyzapper`
//! (read through CFPreferences, so `/Library/Managed Preferences` wins). Windows: registry policy values under
//! `HKLM\SOFTWARE\Policies\KeyZapper` (or HKCU), same names; `ManagedProfiles` is a JSON string there.

use crate::claude_desktop::{BearerTokenType, CredentialKind, DesktopSso};
use crate::claude_office::OfficeClient;
use crate::models::{AuthType, OidcTokenType};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const DOMAIN: &str = "io.github.bl0rb.keyzapper";
pub const KEYS: [&str; 14] = [
    "ManagedProfiles", "AllowedGatewayHosts", "DefaultEndpoint", "DefaultModelAlias", "MinimumClaudeCodeVersion",
    "OneDriveBackup", "BackupDirectory", "UpdateCheckEnabled", "AllowKeyExport", "ManagedMcpServersURL", "ClaudeToolsEnabled",
    "ClaudeDesktopAutoInstall", "ProxyURL", "IgnoreDownloadCertificateErrors",
];

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedProfile {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub model_alias: String,
    pub opus_model: Option<String>,
    pub sonnet_model: Option<String>,
    pub haiku_model: Option<String>,
    pub environment: Option<BTreeMap<String, String>>,
    pub auth_type: AuthType,
    pub oidc_issuer: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_scope: Option<String>,
    pub oidc_token_type: OidcTokenType,
    pub office: Option<OfficeClient>,
    pub claude_desktop: Option<DesktopSso>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedConfig {
    /// `ManagedProfiles`: dicts with `Name`, `Endpoint`, optional `ModelAlias`, `OpusModel`, `SonnetModel`,
    /// `HaikuModel`, `Environment` (dict) and `ID` (UUID); `Type` (`apiKey` default | `oidc`) with
    /// `OIDCIssuer`, `OIDCClientID` (both required for `oidc`), `OIDCScope` and `OIDCTokenType` (`access` default | `id`),
    /// and an optional `Office` dict (Claude in Microsoft 365): `Enabled` (default on), `AddinID`, `GatewayURL`,
    /// `EntraClientID`, `EntraScope`, `DisabledFeatures` (array or comma separated string); an optional `ClaudeDesktop`
    /// dict (Claude Desktop signs in itself): `OIDCIssuer`, `OIDCClientID`, optional `GatewayURL`, `Scopes` and
    /// `BearerTokenType` (`id_token` default | `access_token`) and `CredentialKind` (`external-idp` default | `interactive`,
    /// the older spelling for Claude Desktop before 2.7032.0; anything else makes `problems()` report the block), `Config`
    /// (dict of further Claude Desktop configuration keys) and `AutoApply` (Bool, default off: set up without a click).
    pub profiles: Vec<ManagedProfile>,
    /// `AllowedGatewayHosts`: exact hosts or `*.domain`; empty = no restriction.
    pub allowed_gateway_hosts: Vec<String>,
    /// `DefaultEndpoint` / `DefaultModelAlias`: prefill for self-created profiles.
    pub default_endpoint: Option<String>,
    pub default_model_alias: Option<String>,
    /// `MinimumClaudeCodeVersion`: threshold for the outdated-CLI warning.
    pub minimum_claude_code_version: Option<String>,
    /// `OneDriveBackup`: back up profiles and bindings (never keys) to the user's OneDrive.
    pub one_drive_backup: bool,
    /// `BackupDirectory`: explicit backup folder (supports `~`); enables the backup on its own.
    pub backup_directory: Option<String>,
    /// `UpdateCheckEnabled`: in-app update check against GitHub releases (default on).
    pub update_check_enabled: bool,
    /// `AllowKeyExport`: whether keys may be copied and put into encrypted backups (default on).
    pub allow_key_export: bool,
    /// `ManagedMcpServersURL`: https address of the central MCP server list (`.mcp.json` format), see `mcp_catalog`.
    pub managed_mcp_servers_url: Option<String>,
    /// `ClaudeToolsEnabled`: page for installing Claude Code, its IDE plugins and Claude Desktop (default on).
    pub claude_tools_enabled: bool,
    /// `ClaudeDesktopAutoInstall`: install a missing Claude Desktop in the background, independent of the tools page (default off).
    pub claude_desktop_auto_install: bool,
    /// `ProxyURL`: proxy for KeyZapper's own internet traffic (update check, downloads from GitHub and claude.ai), e.g.
    /// `http://proxy.firma.example:3128`; without it the environment variables and the system proxy apply (see `proxy`).
    pub proxy_url: Option<String>,
    /// `IgnoreDownloadCertificateErrors`: the internet traffic accepts certificates the system does not trust, for a proxy
    /// with TLS inspection whose root certificate is not installed (default off). Packages stay protected by their signature.
    pub ignore_download_certificate_errors: bool,
}

impl Default for ManagedConfig {
    fn default() -> Self {
        Self::from_values(&Map::new())
    }
}

fn as_bool(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|n| n != 0),
        Value::String(s) => match s.to_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn as_string(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(String::from)
}

impl ManagedConfig {
    pub fn from_values(values: &Map<String, Value>) -> Self {
        let profiles = values
            .get("ManagedProfiles")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| managed_profile(item.as_object()?)).collect())
            .unwrap_or_default();
        let allowed_gateway_hosts = match values.get("AllowedGatewayHosts") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(String::from).collect(),
            Some(Value::String(text)) => text.split([',', ';', ' ', '\n']).map(String::from).collect(),
            _ => Vec::<String>::new(),
        };
        Self {
            profiles,
            allowed_gateway_hosts: allowed_gateway_hosts.into_iter().map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()).collect(),
            default_endpoint: as_string(values.get("DefaultEndpoint")),
            default_model_alias: as_string(values.get("DefaultModelAlias")),
            minimum_claude_code_version: as_string(values.get("MinimumClaudeCodeVersion")),
            one_drive_backup: as_bool(values.get("OneDriveBackup")).unwrap_or(false),
            backup_directory: as_string(values.get("BackupDirectory")),
            update_check_enabled: as_bool(values.get("UpdateCheckEnabled")).unwrap_or(true),
            allow_key_export: as_bool(values.get("AllowKeyExport")).unwrap_or(true),
            managed_mcp_servers_url: as_string(values.get("ManagedMcpServersURL")).map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
            claude_tools_enabled: as_bool(values.get("ClaudeToolsEnabled")).unwrap_or(true),
            claude_desktop_auto_install: as_bool(values.get("ClaudeDesktopAutoInstall")).unwrap_or(false),
            proxy_url: as_string(values.get("ProxyURL")).map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
            ignore_download_certificate_errors: as_bool(values.get("IgnoreDownloadCertificateErrors")).unwrap_or(false),
        }
    }

    pub fn load() -> Self {
        Self::from_values(&platform::load())
    }

    pub fn is_endpoint_allowed(&self, endpoint: &str) -> bool {
        if !is_secure_endpoint(endpoint) {
            return false;
        }
        if self.allowed_gateway_hosts.is_empty() {
            return true;
        }
        let Some(host) = host_of(endpoint) else { return false };
        self.allowed_gateway_hosts.iter().any(|pattern| match pattern.strip_prefix('*') {
            Some(suffix) if suffix.starts_with('.') => host.ends_with(suffix),
            _ => host == *pattern,
        })
    }

    pub fn is_managed(&self, id: &str) -> bool {
        self.profiles.iter().any(|p| p.id == id)
    }
}

/// The key travels in the request headers, so it may only go over https; plain http is left for a
/// gateway on this machine (loopback).
pub fn is_secure_endpoint(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint.trim()) else { return false };
    match url.scheme() {
        "https" => url.host_str().is_some_and(|h| !h.is_empty()),
        "http" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    }
}

/// Lowercased host of an http(s) URL.
pub fn host_of(endpoint: &str) -> Option<String> {
    url::Url::parse(endpoint.trim()).ok()?.host_str().filter(|h| !h.is_empty()).map(str::to_lowercase)
}

fn managed_profile(dict: &Map<String, Value>) -> Option<ManagedProfile> {
    let name = dict.get("Name")?.as_str()?.trim().to_string();
    let endpoint = dict.get("Endpoint")?.as_str()?.trim().to_string();
    if name.is_empty() || host_of(&endpoint).is_none() {
        return None;
    }
    let auth_type = match as_string(dict.get("Type")).map(|t| t.trim().to_lowercase()).as_deref() {
        None | Some("") | Some("apikey") => AuthType::ApiKey,
        Some("oidc") => AuthType::Oidc,
        Some(_) => return None,
    };
    let trimmed = |key: &str| as_string(dict.get(key)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let (oidc_issuer, oidc_client_id, oidc_scope) = (trimmed("OIDCIssuer"), trimmed("OIDCClientID"), trimmed("OIDCScope"));
    if auth_type == AuthType::Oidc && (oidc_issuer.is_none() || oidc_client_id.is_none()) {
        return None;
    }
    let oidc_token_type = match trimmed("OIDCTokenType").map(|t| t.to_lowercase()).as_deref() {
        None | Some("access") => OidcTokenType::Access,
        Some("id") => OidcTokenType::Id,
        Some(_) => return None,
    };
    let id = dict.get("ID").and_then(Value::as_str).and_then(crate::models::parse_id).unwrap_or_else(|| derived_id(&name));
    let environment = dict.get("Environment").and_then(Value::as_object).map(|env| {
        env.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect::<BTreeMap<_, _>>()
    });
    Some(ManagedProfile {
        id,
        name,
        endpoint,
        model_alias: as_string(dict.get("ModelAlias")).unwrap_or_default(),
        opus_model: as_string(dict.get("OpusModel")),
        sonnet_model: as_string(dict.get("SonnetModel")),
        haiku_model: as_string(dict.get("HaikuModel")),
        environment,
        auth_type,
        oidc_issuer,
        oidc_client_id,
        oidc_scope,
        oidc_token_type,
        office: dict.get("Office").and_then(Value::as_object).map(office_client),
        claude_desktop: dict.get("ClaudeDesktop").and_then(Value::as_object).map(desktop_sso),
    })
}

/// The `ClaudeDesktop` dict of a managed profile. Like `Office`, a broken block is kept and reported by `problems()`.
fn desktop_sso(dict: &Map<String, Value>) -> DesktopSso {
    let text = |key: &str| as_string(dict.get(key)).map(|v| v.trim().to_string()).unwrap_or_default();
    DesktopSso {
        gateway_url: text("GatewayURL"),
        oidc_issuer: text("OIDCIssuer"),
        oidc_client_id: text("OIDCClientID"),
        scopes: text("Scopes"),
        bearer_token_type: match text("BearerTokenType").to_lowercase().as_str() {
            "access_token" | "access" => BearerTokenType::AccessToken,
            _ => BearerTokenType::IdToken,
        },
        credential_kind: match text("CredentialKind").to_lowercase().as_str() {
            "" | "external-idp" => CredentialKind::ExternalIdp,
            "interactive" => CredentialKind::Interactive,
            _ => CredentialKind::Unknown,
        },
        config: dict.get("Config").and_then(Value::as_object).cloned().unwrap_or_default(),
        auto_apply: as_bool(dict.get("AutoApply")).unwrap_or(false),
    }
}

/// The `Office` dict of a managed profile. Missing or malformed values stay as they are (empty), so a broken block is kept
/// and the sync reports `problems()` instead of the profile silently losing its Office setup.
fn office_client(dict: &Map<String, Value>) -> OfficeClient {
    let text = |key: &str| as_string(dict.get(key)).map(|v| v.trim().to_string()).unwrap_or_default();
    let disabled_features = match dict.get("DisabledFeatures") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(String::from).collect(),
        Some(Value::String(list)) => list.split(',').map(String::from).collect(),
        _ => Vec::<String>::new(),
    };
    OfficeClient {
        enabled: as_bool(dict.get("Enabled")).unwrap_or(true),
        addin_id: text("AddinID"),
        gateway_url: text("GatewayURL"),
        entra_client_id: text("EntraClientID"),
        entra_scope: text("EntraScope"),
        disabled_features: disabled_features.into_iter().map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect(),
    }
}

/// Stable profile ID for a managed profile without explicit `ID`, so bindings survive app restarts
/// (same derivation as the macOS app).
pub fn derived_id(name: &str) -> String {
    uuid_from(&format!("keyzapper-managed-profile:{name}"))
}

/// Stable ID for an MCP server of the central list, derived from its name, so project choices survive reloads.
pub fn derived_mcp_id(name: &str) -> String {
    uuid_from(&format!("keyzapper-managed-mcp:{name}"))
}

fn uuid_from(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0F) | 0x50; // version 5 style
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant
    uuid::Uuid::from_bytes(bytes).hyphenated().to_string().to_uppercase()
}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation_sys::preferences::CFPreferencesCopyAppValue;
    use serde_json::{Map, Value};

    pub fn load() -> Map<String, Value> {
        let domain = CFString::new(super::DOMAIN);
        let mut values = Map::new();
        for key in super::KEYS {
            let name = CFString::new(key);
            let raw = unsafe { CFPreferencesCopyAppValue(name.as_concrete_TypeRef(), domain.as_concrete_TypeRef()) };
            if raw.is_null() {
                continue;
            }
            let value = unsafe { CFType::wrap_under_create_rule(raw) };
            if let Some(json) = to_json(&value) {
                values.insert(key.to_string(), json);
            }
        }
        values
    }

    fn to_json(value: &CFType) -> Option<Value> {
        if let Some(s) = value.downcast::<CFString>() {
            return Some(Value::String(s.to_string()));
        }
        if let Some(b) = value.downcast::<CFBoolean>() {
            return Some(Value::Bool(b.into()));
        }
        if let Some(n) = value.downcast::<CFNumber>() {
            return n.to_i64().map(Value::from).or_else(|| n.to_f64().map(Value::from));
        }
        if let Some(array) = value.downcast::<CFArray>() {
            let items = array.iter().filter_map(|item| to_json(&unsafe { CFType::wrap_under_get_rule(*item) })).collect();
            return Some(Value::Array(items));
        }
        if let Some(dict) = value.downcast::<CFDictionary>() {
            let (keys, entries) = dict.get_keys_and_values();
            let mut map = Map::new();
            for (k, v) in keys.into_iter().zip(entries) {
                let key = unsafe { CFType::wrap_under_get_rule(k) };
                if let (Some(key), Some(v)) = (key.downcast::<CFString>(), to_json(&unsafe { CFType::wrap_under_get_rule(v) })) {
                    map.insert(key.to_string(), v);
                }
            }
            return Some(Value::Object(map));
        }
        None
    }
}

#[cfg(windows)]
mod platform {
    use serde_json::{Map, Value};
    use winreg::enums::{RegType, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    pub const POLICY_KEY: &str = r"SOFTWARE\Policies\KeyZapper";

    /// Machine policies win over user policies, value by value.
    pub fn load() -> Map<String, Value> {
        let mut values = Map::new();
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let Ok(key) = RegKey::predef(hive).open_subkey(POLICY_KEY) else { continue };
            for name in super::KEYS {
                if let Some(value) = read(&key, name) {
                    values.insert(name.to_string(), value);
                }
            }
        }
        values
    }

    fn read(key: &RegKey, name: &str) -> Option<Value> {
        let raw = key.get_raw_value(name).ok()?;
        match raw.vtype {
            RegType::REG_DWORD => key.get_value::<u32, _>(name).ok().map(|n| Value::Bool(n != 0)),
            RegType::REG_MULTI_SZ => key.get_value::<Vec<String>, _>(name).ok().map(|v| Value::Array(v.into_iter().map(Value::String).collect())),
            RegType::REG_SZ | RegType::REG_EXPAND_SZ => {
                let text: String = key.get_value(name).ok()?;
                if name == "ManagedProfiles" {
                    serde_json::from_str(&text).ok()
                } else {
                    Some(Value::String(text))
                }
            }
            _ => None,
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub fn load() -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_https_or_loopback_http_is_secure() {
        assert!(is_secure_endpoint("https://litellm.firma.example"));
        assert!(is_secure_endpoint(" http://127.0.0.1:4000 "));
        assert!(is_secure_endpoint("http://localhost:4000/v1"));
        assert!(is_secure_endpoint("http://[::1]:4000"));
        assert!(!is_secure_endpoint("http://litellm.firma.example"));
        assert!(!is_secure_endpoint("http://localhost.evil.example"));
        assert!(!is_secure_endpoint("ftp://litellm.firma.example"));
        assert!(!is_secure_endpoint("litellm.firma.example"));
        assert!(ManagedConfig::default().is_endpoint_allowed("https://any.example"));
        assert!(!ManagedConfig::default().is_endpoint_allowed("http://any.example"));
    }

    #[test]
    fn parses_profiles_hosts_and_flags() {
        let values = json!({
            "ManagedProfiles": [{"Name": "Projekt Alpha", "Endpoint": "https://litellm.firma.example", "OpusModel": "o",
                                 "Environment": {"CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS": "1"}},
                                {"Name": "", "Endpoint": "https://x"},
                                {"Name": "SSO", "Endpoint": "https://gw.firma.example", "Type": "oidc",
                                 "OIDCIssuer": "https://login.example/t/v2.0", "OIDCClientID": "abc", "OIDCScope": "openid offline_access",
                                 "OIDCTokenType": "ID"},
                                {"Name": "SSO falsche Token-Art", "Endpoint": "https://gw.firma.example", "Type": "oidc",
                                 "OIDCIssuer": "https://i", "OIDCClientID": "abc", "OIDCTokenType": "jwt"},
                                {"Name": "SSO ohne Client", "Endpoint": "https://gw.firma.example", "Type": "oidc", "OIDCIssuer": "https://i"}],
            "AllowedGatewayHosts": ["LiteLLM.firma.example", "*.litellm.firma.example"],
            "AllowKeyExport": false
        });
        let config = ManagedConfig::from_values(values.as_object().unwrap());
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.profiles[0].id, derived_id("Projekt Alpha"));
        assert_eq!(config.profiles[0].auth_type, AuthType::ApiKey);
        assert_eq!(config.profiles[0].oidc_token_type, OidcTokenType::Access);
        let sso = &config.profiles[1];
        assert_eq!(sso.auth_type, AuthType::Oidc);
        assert_eq!((sso.oidc_issuer.as_deref(), sso.oidc_client_id.as_deref()), (Some("https://login.example/t/v2.0"), Some("abc")));
        assert_eq!(sso.oidc_scope.as_deref(), Some("openid offline_access"));
        assert_eq!(sso.oidc_token_type, OidcTokenType::Id);
        assert!(config.is_endpoint_allowed("https://litellm.firma.example/v1"));
        assert!(config.is_endpoint_allowed("https://eu.litellm.firma.example"));
        assert!(!config.is_endpoint_allowed("https://evil.example"));
        assert!(!config.is_endpoint_allowed("http://litellm.firma.example/v1"));
        assert!(!config.allow_key_export && config.update_check_enabled);
        assert!(ManagedConfig::default().allow_key_export && ManagedConfig::default().claude_tools_enabled);
        assert_eq!(ManagedConfig::default().managed_mcp_servers_url, None);
        let values = json!({"ManagedMcpServersURL": " https://sso.firma.example/keyzapper/mcp.json ", "ClaudeToolsEnabled": 0, "ProxyURL": " http://proxy.firma.example:3128 "});
        let config = ManagedConfig::from_values(values.as_object().unwrap());
        assert_eq!(config.managed_mcp_servers_url.as_deref(), Some("https://sso.firma.example/keyzapper/mcp.json"));
        assert!(!config.claude_tools_enabled);
        assert_eq!(config.proxy_url.as_deref(), Some("http://proxy.firma.example:3128"));
        assert_eq!(ManagedConfig::from_values(json!({"ProxyURL": "  "}).as_object().unwrap()).proxy_url, None);
        assert!(!config.ignore_download_certificate_errors);
        assert!(ManagedConfig::from_values(json!({"IgnoreDownloadCertificateErrors": true}).as_object().unwrap()).ignore_download_certificate_errors);
    }

    #[test]
    fn derived_ids_match_the_macos_app() {
        let id = derived_id("Projekt Alpha");
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "5");
        assert_eq!(id, id.to_uppercase());
        assert_ne!(derived_mcp_id("Projekt Alpha"), id);
        assert_eq!(derived_mcp_id("github"), derived_mcp_id("github"));
    }
}
