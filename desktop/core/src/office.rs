//! Claude for Microsoft 365 (Excel, Word, PowerPoint, Outlook) via Anthropic's bootstrap endpoint.
//!
//! The Office add-in runs in a browser sandbox and cannot call `keyzapper-helper`. Instead it fetches its
//! configuration from an HTTPS endpoint (`bootstrap_url` in the add-in manifest) with the user's Microsoft Entra
//! token. This module holds everything that does not need an HTTP server: the per-profile Office settings, the
//! host/identity types, the resolver (which profile does this user get?), the credential providers (which token does
//! the add-in get?) and the sparse bootstrap response. The server lives in the `keyzapper-office-bootstrap` crate.
//!
//! Security boundary: here KeyZapper is the *resource server* that validates a token issued to someone else.
//! `oidc.rs` is the opposite role (KeyZapper as OAuth client) and is deliberately not reused.

use crate::gateway;
use crate::models::{parse_id, AppState, McpServer, McpTransport, Profile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// Origin of the add-in task pane; every browser-fetched URL must allow it via CORS.
pub const DEFAULT_ALLOWED_ORIGIN: &str = "https://pivot.claude.ai";
/// Header the add-in sends on every bootstrap request, e.g. `claude-excel/1.2.3`.
pub const USER_AGENT_HEADER: &str = "X-Claude-User-Agent";
/// Default lifetime of a bootstrap response (and of a vended credential).
pub const DEFAULT_TTL_SECONDS: u64 = 15 * 60;
pub const MIN_TTL_SECONDS: u64 = 60;
pub const MAX_TTL_SECONDS: u64 = 24 * 60 * 60;
/// Application (client) ID of Anthropic's Claude for Microsoft 365 Entra app, the default audience of the add-in's
/// Entra token when no own `graph_client_id` is used. Documented here, never assumed silently.
pub const ANTHROPIC_OFFICE_APP_ID: &str = "c2995f31-11e7-4882-b7a7-ef9def0a0266";

/// Office application the add-in runs in, from `X-Claude-User-Agent: claude-<host>/<version>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OfficeHost {
    Excel,
    Word,
    PowerPoint,
    Outlook,
    /// Anything else, including a missing or malformed header. Never allowed.
    Unknown(String),
}

impl OfficeHost {
    pub const KNOWN: [OfficeHost; 4] = [OfficeHost::Excel, OfficeHost::Word, OfficeHost::PowerPoint, OfficeHost::Outlook];

    pub fn as_str(&self) -> &str {
        match self {
            OfficeHost::Excel => "excel",
            OfficeHost::Word => "word",
            OfficeHost::PowerPoint => "powerpoint",
            OfficeHost::Outlook => "outlook",
            OfficeHost::Unknown(name) => name,
        }
    }

    /// `excel`, `word`, `powerpoint`, `outlook` (case-insensitive); anything else is `Unknown`.
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_lowercase().as_str() {
            "excel" => OfficeHost::Excel,
            "word" => OfficeHost::Word,
            "powerpoint" => OfficeHost::PowerPoint,
            "outlook" => OfficeHost::Outlook,
            other => OfficeHost::Unknown(other.to_string()),
        }
    }

    /// Parses the `X-Claude-User-Agent` value (`claude-excel/1.2.3`). A missing header is `Unknown("")`.
    pub fn from_user_agent(value: Option<&str>) -> Self {
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else { return OfficeHost::Unknown(String::new()) };
        let product = value.split('/').next().unwrap_or_default().trim();
        match product.strip_prefix("claude-") {
            Some(host) if !host.is_empty() => OfficeHost::from_name(host),
            _ => OfficeHost::Unknown(product.to_lowercase()),
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, OfficeHost::Unknown(_))
    }
}

impl fmt::Display for OfficeHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for OfficeHost {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OfficeHost {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(OfficeHost::from_name(&String::deserialize(deserializer)?))
    }
}

/// Header the add-in puts the gateway token into (`gateway_auth_header` in the bootstrap response).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GatewayAuthHeader {
    #[default]
    XApiKey,
    Authorization,
}

impl GatewayAuthHeader {
    pub fn as_str(&self) -> &'static str {
        match self {
            GatewayAuthHeader::XApiKey => "x-api-key",
            GatewayAuthHeader::Authorization => "authorization",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "" | "x-api-key" => Some(GatewayAuthHeader::XApiKey),
            "authorization" => Some(GatewayAuthHeader::Authorization),
            _ => None,
        }
    }

    fn is_default(&self) -> bool {
        *self == GatewayAuthHeader::XApiKey
    }
}

fn default_hosts() -> Vec<OfficeHost> {
    vec![OfficeHost::Excel, OfficeHost::Word, OfficeHost::PowerPoint]
}

/// Office release of one profile. Absent (`None` on the profile) means: not used for Office at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeSettings {
    /// Default off: no key ever reaches an Office client without this.
    #[serde(default)]
    pub enabled: bool,
    /// Office applications that may use the profile (Outlook is not in the default set).
    #[serde(default = "default_hosts")]
    pub allowed_hosts: Vec<OfficeHost>,
    /// Entra object IDs (`oid` claim) of the users the profile is released to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_users: Vec<String>,
    /// Entra group IDs; reserved for a later resolver (Graph or `groups` claim), not evaluated yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_groups: Vec<String>,
    /// Extra headers the add-in sends on every inference request (cost centre, project), never secrets.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inference_headers: BTreeMap<String, String>,
    /// IDs of KeyZapper MCP servers; only remote (http/sse, https) servers reach the add-in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_server_ids: Vec<String>,
    /// Add-in feature slugs to turn off (`disabled_features` in the bootstrap response).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_features: Vec<String>,
    /// Lifetime of the bootstrap response and the vended credential; default 15 minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_ttl_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "GatewayAuthHeader::is_default")]
    pub gateway_auth_header: GatewayAuthHeader,
}

impl Default for OfficeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_hosts: default_hosts(),
            allowed_users: vec![],
            allowed_groups: vec![],
            inference_headers: BTreeMap::new(),
            mcp_server_ids: vec![],
            disabled_features: vec![],
            bootstrap_ttl_seconds: None,
            gateway_auth_header: GatewayAuthHeader::XApiKey,
        }
    }
}

impl OfficeSettings {
    pub fn ttl_seconds(&self) -> u64 {
        self.bootstrap_ttl_seconds.unwrap_or(DEFAULT_TTL_SECONDS).clamp(MIN_TTL_SECONDS, MAX_TTL_SECONDS)
    }

    pub fn allows_host(&self, host: &OfficeHost) -> bool {
        host.is_known() && self.allowed_hosts.contains(host)
    }

    /// Whether a user is on the profile's own list (explicit assignments bypass this list).
    pub fn allows_user(&self, oid: &str) -> bool {
        let oid = normalize_oid(oid);
        self.allowed_users.iter().any(|u| normalize_oid(u) == oid)
    }

    /// True if nothing is configured, so the profile can store `None` instead.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// IDs are normalised; invalid object IDs are dropped.
    pub fn normalized(mut self) -> Self {
        self.allowed_users = self.allowed_users.iter().filter_map(|u| parse_oid(u)).collect();
        self.allowed_users.dedup();
        for id in self.mcp_server_ids.iter_mut() {
            if let Some(n) = parse_id(id) {
                *id = n;
            }
        }
        self
    }
}

/// Entra object IDs are GUIDs; compared and stored lowercase.
pub fn parse_oid(text: &str) -> Option<String> {
    uuid::Uuid::parse_str(text.trim()).ok().map(|id| id.hyphenated().to_string())
}

fn normalize_oid(text: &str) -> String {
    parse_oid(text).unwrap_or_else(|| text.trim().to_lowercase())
}

/// Parses one object ID per line (blank lines and `#` comments skipped); returns the IDs and the rejected lines.
pub fn parse_user_ids(text: &str) -> (Vec<String>, Vec<String>) {
    let mut ids = Vec::new();
    let mut invalid = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_oid(line) {
            Some(id) if !ids.contains(&id) => ids.push(id),
            Some(_) => {}
            None => invalid.push(line.to_string()),
        }
    }
    (ids, invalid)
}

/// The validated caller: only `oid` identifies the user. Names and e-mail addresses are display-only and
/// deliberately not carried here, so nothing downstream can key on them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntraIdentity {
    /// `oid` claim, lowercase GUID.
    pub oid: String,
    /// `tid` claim (or the configured tenant when the token has none).
    pub tenant_id: String,
    /// `scp` claim, split at whitespace.
    pub scopes: Vec<String>,
}

/// Stable, non-reversible user ID for logs and gateway metadata (`sha256(salt ‖ oid)`, 16 hex chars).
pub fn pseudonymous_user_id(oid: &str, salt: &str) -> String {
    let digest = Sha256::digest(format!("{salt}\u{0}{}", normalize_oid(oid)).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Why a user gets no configuration (all answered with 403 by the server).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfficeDenied {
    /// No assignment and no profile lists the user.
    NotAssigned,
    /// The assigned profile is not released for Office.
    OfficeDisabled,
    /// The assigned profile does not allow this Office application (or the host is unknown).
    HostNotAllowed(OfficeHost),
}

impl fmt::Display for OfficeDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OfficeDenied::NotAssigned => f.write_str("user is not assigned to an Office profile"),
            OfficeDenied::OfficeDisabled => f.write_str("the assigned profile is not released for Office"),
            OfficeDenied::HostNotAllowed(host) => write!(f, "office host not allowed: {host}"),
        }
    }
}

impl std::error::Error for OfficeDenied {}

/// The profile a user ends up with, including its Office settings (always `Some` on the profile here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedOfficeProfile {
    pub profile: Profile,
    pub settings: OfficeSettings,
}

/// Which profile an Office user gets. Implementations may later consult groups, departments, Microsoft Graph or
/// an external policy store; the MVP maps `oid` → profile.
pub trait OfficeProfileResolver: Send + Sync {
    fn resolve(&self, identity: &EntraIdentity, host: &OfficeHost) -> Result<ResolvedOfficeProfile, OfficeDenied>;
}

/// `oid` → profile: explicit `assignments` first, then the profiles' own `allowedUsers` lists (first match wins).
#[derive(Clone, Debug, Default)]
pub struct AssignmentResolver {
    pub profiles: Vec<Profile>,
    pub assignments: BTreeMap<String, String>,
}

impl AssignmentResolver {
    pub fn new(profiles: Vec<Profile>, assignments: BTreeMap<String, String>) -> Self {
        let assignments = assignments.into_iter().filter_map(|(oid, id)| Some((parse_oid(&oid)?, parse_id(&id)?))).collect();
        Self { profiles, assignments }
    }

    fn profile_for(&self, oid: &str) -> Option<&Profile> {
        if let Some(id) = self.assignments.get(oid) {
            return self.profiles.iter().find(|p| p.id == *id);
        }
        self.profiles.iter().find(|p| p.office.as_ref().is_some_and(|o| o.enabled && o.allows_user(oid)))
    }
}

impl OfficeProfileResolver for AssignmentResolver {
    fn resolve(&self, identity: &EntraIdentity, host: &OfficeHost) -> Result<ResolvedOfficeProfile, OfficeDenied> {
        let oid = normalize_oid(&identity.oid);
        let profile = self.profile_for(&oid).ok_or(OfficeDenied::NotAssigned)?;
        let settings = profile.office.clone().filter(|o| o.enabled).ok_or(OfficeDenied::OfficeDisabled)?;
        if !settings.allows_host(host) {
            return Err(OfficeDenied::HostNotAllowed(host.clone()));
        }
        Ok(ResolvedOfficeProfile { profile: profile.clone(), settings })
    }
}

/// A gateway token for one Office user. `Debug` never shows the token.
#[derive(Clone, PartialEq, Eq)]
pub struct OfficeCredential {
    pub token: String,
    /// Unix seconds; None for tokens without a known expiry (static keys).
    pub expires_at: Option<i64>,
}

impl fmt::Debug for OfficeCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OfficeCredential").field("token", &"<redacted>").field("expires_at", &self.expires_at).finish()
    }
}

/// Credential vending failed. Messages carry no token material and no upstream response bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialError {
    /// Misconfiguration (no key for the profile, management key rejected): HTTP 500.
    NotConfigured(String),
    /// The vending backend is unreachable or failing: HTTP 503.
    Unavailable(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CredentialError::NotConfigured(m) => write!(f, "credential provider not configured: {m}"),
            CredentialError::Unavailable(m) => write!(f, "credential provider unavailable: {m}"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Where the add-in's gateway token comes from. Implementations: static profile key (less secure), short-lived
/// LiteLLM virtual keys; later OIDC exchange or an external secret store.
pub trait OfficeCredentialProvider: Send + Sync {
    /// Short name for logs, e.g. `litellm-virtual-key`.
    fn kind(&self) -> &'static str;
    fn credential_for(&self, profile: &Profile, identity: &EntraIdentity, ttl_seconds: u64) -> Result<OfficeCredential, CredentialError>;
}

/// Hands out the profile's long-lived key as is. Only for deployments that explicitly accept that every Office
/// user of a profile shares that key; prefer `LiteLlmVirtualKeyProvider`.
pub struct StaticKeyProvider {
    keys: BTreeMap<String, String>,
}

impl StaticKeyProvider {
    pub fn new(keys: BTreeMap<String, String>) -> Self {
        Self { keys: keys.into_iter().filter_map(|(id, key)| Some((parse_id(&id)?, key.trim().to_string()))).filter(|(_, k)| !k.is_empty()).collect() }
    }
}

impl fmt::Debug for StaticKeyProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticKeyProvider").field("profiles", &self.keys.len()).finish()
    }
}

impl OfficeCredentialProvider for StaticKeyProvider {
    fn kind(&self) -> &'static str {
        "static-profile-key"
    }

    fn credential_for(&self, profile: &Profile, _identity: &EntraIdentity, _ttl_seconds: u64) -> Result<OfficeCredential, CredentialError> {
        let token = self.keys.get(&profile.id).ok_or_else(|| CredentialError::NotConfigured(format!("no key for profile {}", profile.id)))?;
        Ok(OfficeCredential { token: token.clone(), expires_at: None })
    }
}

/// Short-lived LiteLLM virtual keys (`POST /key/generate` with `duration`), one per user and bootstrap, issued with
/// a management key that never leaves the server. The key inherits the profile's models and is tagged with the
/// profile and a pseudonymous user ID, so LiteLLM's spend logs stay attributable without e-mail addresses.
pub struct LiteLlmVirtualKeyProvider {
    management_key: String,
    /// LiteLLM to talk to; None means the profile's own endpoint.
    pub endpoint: Option<String>,
    /// Optional spend cap per vended key.
    pub max_budget: Option<f64>,
    /// Salt for the pseudonymous user ID in the key metadata.
    pub user_salt: String,
    pub timeout: Duration,
}

impl LiteLlmVirtualKeyProvider {
    pub fn new(management_key: String) -> Self {
        Self { management_key, endpoint: None, max_budget: None, user_salt: String::new(), timeout: Duration::from_secs(15) }
    }

    /// Request body for `/key/generate`; public so tests and other backends can reuse the shape.
    pub fn request_body(&self, profile: &Profile, identity: &EntraIdentity, ttl_seconds: u64) -> serde_json::Value {
        let mut body = serde_json::json!({
            "duration": format!("{ttl_seconds}s"),
            "metadata": {
                "keyzapper_profile": profile.id,
                "keyzapper_profile_name": profile.name,
                "keyzapper_user": pseudonymous_user_id(&identity.oid, &self.user_salt),
                "keyzapper_source": "office",
            },
        });
        let models = profile.configured_models();
        if !models.is_empty() {
            body["models"] = models.into();
        }
        if let Some(budget) = self.max_budget {
            body["max_budget"] = budget.into();
        }
        body
    }
}

impl fmt::Debug for LiteLlmVirtualKeyProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiteLlmVirtualKeyProvider").field("endpoint", &self.endpoint).field("max_budget", &self.max_budget).finish()
    }
}

impl OfficeCredentialProvider for LiteLlmVirtualKeyProvider {
    fn kind(&self) -> &'static str {
        "litellm-virtual-key"
    }

    fn credential_for(&self, profile: &Profile, identity: &EntraIdentity, ttl_seconds: u64) -> Result<OfficeCredential, CredentialError> {
        let endpoint = self.endpoint.as_deref().unwrap_or(&profile.endpoint);
        let issued_at = crate::oidc::unix_now();
        let response = gateway::agent(self.timeout)
            .post(&gateway::join_url(endpoint, "key/generate"))
            .header("Authorization", &format!("Bearer {}", self.management_key))
            .header("content-type", "application/json")
            .send(self.request_body(profile, identity, ttl_seconds).to_string());
        let mut response = response.map_err(|e| CredentialError::Unavailable(transport_error(&e.to_string())))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        match status {
            200 => {}
            401 | 403 => return Err(CredentialError::NotConfigured(format!("LiteLLM rejected the management key (HTTP {status})"))),
            other => return Err(CredentialError::Unavailable(format!("LiteLLM /key/generate answered HTTP {other}"))),
        }
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let token = value["key"].as_str().map(str::trim).filter(|k| !k.is_empty()).ok_or_else(|| CredentialError::Unavailable("LiteLLM /key/generate returned no key".into()))?;
        Ok(OfficeCredential { token: token.to_string(), expires_at: Some(issued_at + ttl_seconds as i64) })
    }
}

/// Transport errors may echo the URL, never a header; keep them short.
fn transport_error(message: &str) -> String {
    message.chars().take(200).collect()
}

/// MCP server entry of the bootstrap response (`mcp_servers[]`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeMcpServer {
    pub url: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// Header names the add-in sets itself or that would carry credentials; never accepted as inference headers.
const RESERVED_HEADER_NAMES: [&str; 10] = [
    "authorization", "x-api-key", "content-type", "host", "content-length", "user-agent", "cookie", "proxy-authorization",
    "accept", "origin",
];
const RESERVED_HEADER_PREFIXES: [&str; 3] = ["anthropic-", "x-amz-", "x-goog-"];
/// MCP headers that would hand a credential to every Office user; they stay server-side.
const CREDENTIAL_HEADER_NAMES: [&str; 4] = ["authorization", "x-api-key", "cookie", "proxy-authorization"];

fn is_token(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Case-insensitive denylist check for `inference_headers` names.
pub fn is_reserved_inference_header(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    RESERVED_HEADER_NAMES.contains(&lower.as_str()) || RESERVED_HEADER_PREFIXES.iter().any(|p| lower.starts_with(p))
}

/// A usable inference header name: HTTP token characters and not reserved.
pub fn is_valid_inference_header_name(name: &str) -> bool {
    is_token(name.trim()) && !is_reserved_inference_header(name)
}

fn is_credential_header(name: &str) -> bool {
    CREDENTIAL_HEADER_NAMES.contains(&name.trim().to_lowercase().as_str())
}

/// Headers with `${NAME}` placeholders are Claude Code conventions (filled from the local environment); the add-in
/// cannot fill them, so they are dropped rather than sent literally.
fn has_placeholder(value: &str) -> bool {
    value.contains("${")
}

/// Drops reserved and malformed names and fills `{{profile-id}}` / `{{profile-name}}`.
pub fn sanitized_inference_headers(headers: &BTreeMap<String, String>, profile: &Profile) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter(|(name, _)| is_valid_inference_header_name(name))
        .map(|(name, value)| {
            let value = value.replace("{{profile-id}}", &profile.id).replace("{{profile-name}}", &profile.name);
            (name.trim().to_string(), value.trim().to_string())
        })
        .filter(|(_, value)| !value.is_empty() && !value.chars().any(char::is_control))
        .collect()
}

/// `https://` with a host that is not loopback: the add-in fetches from the browser, so plain http and localhost
/// never work there.
pub fn is_office_url(text: &str) -> bool {
    url::Url::parse(text.trim()).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| !matches!(h, "localhost" | "127.0.0.1" | "[::1]")))
}

/// The remote MCP servers of a profile in bootstrap form; local (stdio) servers and non-https URLs are skipped,
/// as are credential headers and headers with unresolved `${…}` placeholders.
pub fn office_mcp_servers(settings: &OfficeSettings, servers: &[McpServer]) -> Vec<OfficeMcpServer> {
    settings
        .mcp_server_ids
        .iter()
        .filter_map(|id| servers.iter().find(|s| s.id == *id))
        .filter(|s| s.transport != McpTransport::Stdio && is_office_url(&s.url))
        .map(|s| OfficeMcpServer {
            url: s.url.trim().to_string(),
            label: s.name.clone(),
            headers: s.headers.iter().filter(|(n, v)| is_token(n) && !is_credential_header(n) && !has_placeholder(v)).map(|(n, v)| (n.clone(), v.clone())).collect(),
        })
        .collect()
}

/// Body of `GET /office/bootstrap`: only the keys the user needs. Field names are the add-in's. `Debug` redacts
/// the token and MCP headers, so a logged response never leaks a credential.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BootstrapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_api_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_auth_header: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inference_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<OfficeMcpServer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_features: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_models: Vec<String>,
    /// Reserved: `{name, description?, content | url}` objects; not produced yet, kept so adding skills later
    /// changes no field names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<serde_json::Value>,
    /// Unix seconds; the add-in re-fetches shortly before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_expires_at: Option<i64>,
}

impl BootstrapResponse {
    /// `{}`: a valid user who only gets the organisation-wide defaults from the manifest.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Non-secret one-line summary for logs.
    pub fn summary(&self) -> String {
        format!(
            "gateway_url={} token={} headers={} mcp={} models={} expires_at={}",
            self.gateway_url.as_deref().unwrap_or("-"),
            if self.gateway_token.is_some() { "yes" } else { "no" },
            self.inference_headers.len(),
            self.mcp_servers.len(),
            self.available_models.len(),
            self.bootstrap_expires_at.map(|t| t.to_string()).unwrap_or_else(|| "-".into())
        )
    }
}

impl fmt::Debug for BootstrapResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootstrapResponse")
            .field("gateway_url", &self.gateway_url)
            .field("gateway_token", &self.gateway_token.as_ref().map(|_| "<redacted>"))
            .field("gateway_api_format", &self.gateway_api_format)
            .field("gateway_auth_header", &self.gateway_auth_header)
            .field("inference_headers", &self.inference_headers)
            .field("mcp_servers", &self.mcp_servers.iter().map(|s| (&s.url, &s.label, s.headers.len())).collect::<Vec<_>>())
            .field("disabled_features", &self.disabled_features)
            .field("available_models", &self.available_models)
            .field("bootstrap_expires_at", &self.bootstrap_expires_at)
            .finish()
    }
}

/// Assembles the response for a resolved profile. `now` is Unix seconds.
pub fn build_response(resolved: &ResolvedOfficeProfile, credential: &OfficeCredential, servers: &[McpServer], now: i64) -> BootstrapResponse {
    let profile = &resolved.profile;
    let settings = &resolved.settings;
    let mut expires_at = now + settings.ttl_seconds() as i64;
    if let Some(credential_expiry) = credential.expires_at {
        expires_at = expires_at.min(credential_expiry);
    }
    BootstrapResponse {
        gateway_url: Some(profile.endpoint.trim().trim_end_matches('/').to_string()),
        gateway_token: Some(credential.token.clone()),
        gateway_api_format: Some("anthropic".into()),
        gateway_auth_header: (!settings.gateway_auth_header.is_default()).then(|| settings.gateway_auth_header.as_str().to_string()),
        inference_headers: sanitized_inference_headers(&settings.inference_headers, profile),
        mcp_servers: office_mcp_servers(settings, servers),
        disabled_features: settings.disabled_features.clone(),
        available_models: profile.configured_models(),
        skills: vec![],
        bootstrap_expires_at: Some(expires_at),
    }
}

/// What happens to a valid user without a profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnassignedPolicy {
    /// 403 (default, fail closed).
    #[default]
    Forbid,
    /// `200 {}`: the add-in keeps the organisation-wide manifest defaults.
    Defaults,
}

/// Tenant, audience and bootstrap URL as kept in the desktop app (non-secret).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDeploymentSettings {
    #[serde(default)]
    pub tenant_id: String,
    /// Expected `aud` of the Entra token: Anthropic's add-in app ID or the organisation's own app / `api://` URI.
    #[serde(default)]
    pub audience: String,
    /// Public HTTPS URL of the bootstrap service, for the manifest (`bootstrap_url`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bootstrap_url: String,
}

/// The file the desktop app exports for the bootstrap service. Contains no secrets: no keys, no tokens, no
/// management credentials. Those are configured on the server (environment or a separate file).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDeployment {
    pub schema_version: u32,
    pub tenant_id: String,
    /// Accepted `aud` values (one or more).
    pub audiences: Vec<String>,
    #[serde(default = "default_origins")]
    pub allowed_origins: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_url: Option<String>,
    #[serde(default)]
    pub unassigned_policy: UnassignedPolicy,
    /// Only profiles released for Office, as stored (endpoint, models, Office settings).
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// Only the remote MCP servers those profiles reference, without credential headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// `oid` → profile ID, derived from the profiles' user lists (first profile wins); may be edited on the server.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assignments: BTreeMap<String, String>,
}

fn default_origins() -> Vec<String> {
    vec![DEFAULT_ALLOWED_ORIGIN.to_string()]
}

impl OfficeDeployment {
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// Builds the export from the app state. Fails (with a user-facing reason) without tenant, audience or an
    /// Office-enabled profile.
    pub fn from_state(state: &AppState) -> Result<Self, String> {
        let settings = state.office_deployment.clone().unwrap_or_default();
        let tenant_id = parse_oid(&settings.tenant_id).ok_or_else(|| crate::l!("Bitte die Entra-Tenant-ID (GUID) angeben."))?;
        let audience = settings.audience.trim().to_string();
        if audience.is_empty() {
            return Err(crate::l!("Bitte die erwartete Audience des Entra-Tokens angeben."));
        }
        let profiles: Vec<Profile> = state
            .profiles
            .iter()
            .filter(|p| p.is_office_enabled())
            .cloned()
            .map(|mut p| {
                // The app rejects reserved names on input; older or hand-edited state is cleaned here as well.
                if let Some(office) = p.office.as_mut() {
                    office.inference_headers.retain(|name, _| is_valid_inference_header_name(name));
                }
                p
            })
            .collect();
        if profiles.is_empty() {
            return Err(crate::l!("Kein Profil ist für Claude for Microsoft 365 freigegeben."));
        }
        let mut assignments = BTreeMap::new();
        for profile in &profiles {
            for oid in profile.office.iter().flat_map(|o| o.allowed_users.iter()).filter_map(|u| parse_oid(u)) {
                assignments.entry(oid).or_insert_with(|| profile.id.clone());
            }
        }
        let referenced: Vec<&str> = profiles.iter().flat_map(|p| p.office.iter().flat_map(|o| o.mcp_server_ids.iter())).map(String::as_str).collect();
        let mcp_servers = state
            .mcp_servers
            .iter()
            .filter(|s| referenced.contains(&s.id.as_str()) && s.transport != McpTransport::Stdio && is_office_url(&s.url))
            .map(|s| {
                let mut s = s.clone();
                s.headers.retain(|n, v| !is_credential_header(n) && !has_placeholder(v));
                s.env.clear();
                s
            })
            .collect();
        Ok(Self {
            schema_version: Self::CURRENT_SCHEMA_VERSION,
            tenant_id,
            audiences: vec![audience],
            allowed_origins: default_origins(),
            bootstrap_url: Some(settings.bootstrap_url.trim().to_string()).filter(|u| !u.is_empty()),
            unassigned_policy: UnassignedPolicy::Forbid,
            profiles,
            mcp_servers,
            assignments,
        })
    }

    /// Entra v2 issuer of the tenant.
    pub fn issuer(&self) -> String {
        format!("https://login.microsoftonline.com/{}/v2.0", self.tenant_id)
    }

    /// Microsoft's signing keys for the tenant.
    pub fn jwks_url(&self) -> String {
        format!("https://login.microsoftonline.com/{}/discovery/v2.0/keys", self.tenant_id)
    }

    pub fn resolver(&self) -> AssignmentResolver {
        AssignmentResolver::new(self.profiles.clone(), self.assignments.clone())
    }

    /// Problems that make the deployment unusable; empty when it can be served.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.schema_version != Self::CURRENT_SCHEMA_VERSION {
            problems.push(format!("unsupported schemaVersion {}", self.schema_version));
        }
        if parse_oid(&self.tenant_id).is_none() {
            problems.push("tenantId must be a GUID".into());
        }
        if self.audiences.iter().all(|a| a.trim().is_empty()) {
            problems.push("audiences must not be empty".into());
        }
        if self.allowed_origins.is_empty() || self.allowed_origins.iter().any(|o| o.trim() == "*") {
            problems.push("allowedOrigins must list explicit origins (no *)".into());
        }
        for profile in &self.profiles {
            if !crate::managed::is_secure_endpoint(&profile.endpoint) {
                problems.push(format!("profile {}: endpoint is not https", profile.id));
            }
            if let Some(office) = &profile.office {
                for name in office.inference_headers.keys().filter(|n| !is_valid_inference_header_name(n)) {
                    problems.push(format!("profile {}: inference header {name:?} is reserved or invalid", profile.id));
                }
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ALPHA: &str = "11111111-1111-1111-1111-111111111111";
    const USER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const TENANT: &str = "99999999-9999-9999-9999-999999999999";

    fn profile() -> Profile {
        let mut p = Profile::new(ALPHA.into(), "Consulting".into(), "https://litellm.example.test/".into(), String::new());
        p.sonnet_model = Some("company-sonnet".into());
        p.office = Some(OfficeSettings {
            enabled: true,
            allowed_users: vec![USER.into()],
            inference_headers: BTreeMap::from([
                ("x-keyzapper-profile".into(), "{{profile-name}}".into()),
                ("Authorization".into(), "Bearer nope".into()),
                ("anthropic-beta".into(), "x".into()),
                ("x-cost-center".into(), "4711".into()),
            ]),
            mcp_server_ids: vec!["22222222-2222-2222-2222-222222222222".into(), "33333333-3333-3333-3333-333333333333".into()],
            ..OfficeSettings::default()
        });
        p
    }

    fn identity() -> EntraIdentity {
        EntraIdentity { oid: USER.to_uppercase(), tenant_id: TENANT.into(), scopes: vec![] }
    }

    fn servers() -> Vec<McpServer> {
        let mut remote = McpServer::new("22222222-2222-2222-2222-222222222222".into(), "search".into());
        remote.transport = McpTransport::Http;
        remote.url = "https://mcp.example.test/search".into();
        remote.headers = BTreeMap::from([("X-Team".into(), "dev".into()), ("Authorization".into(), "Bearer s".into()), ("X-Token".into(), "${TOKEN}".into())]);
        let mut local = McpServer::new("33333333-3333-3333-3333-333333333333".into(), "github".into());
        local.command = "npx".into();
        vec![remote, local]
    }

    #[test]
    fn hosts_parse_from_user_agent_and_round_trip() {
        assert_eq!(OfficeHost::from_user_agent(Some("claude-excel/1.2.3")), OfficeHost::Excel);
        assert_eq!(OfficeHost::from_user_agent(Some(" claude-PowerPoint/2 ")), OfficeHost::PowerPoint);
        assert_eq!(OfficeHost::from_user_agent(Some("claude-teams/1")), OfficeHost::Unknown("teams".into()));
        assert_eq!(OfficeHost::from_user_agent(Some("Mozilla/5.0")), OfficeHost::Unknown("mozilla".into()));
        assert_eq!(OfficeHost::from_user_agent(None), OfficeHost::Unknown(String::new()));
        assert!(!OfficeHost::from_user_agent(None).is_known());
        let hosts: Vec<OfficeHost> = serde_json::from_value(json!(["excel", "Word", "outlook", "visio"])).unwrap();
        assert_eq!(hosts, [OfficeHost::Excel, OfficeHost::Word, OfficeHost::Outlook, OfficeHost::Unknown("visio".into())]);
        assert_eq!(serde_json::to_value(&hosts).unwrap(), json!(["excel", "word", "outlook", "visio"]));
    }

    #[test]
    fn settings_default_to_disabled_and_office_apps_without_outlook() {
        let settings = OfficeSettings::default();
        assert!(!settings.enabled && settings.is_empty());
        assert!(settings.allows_host(&OfficeHost::Excel) && settings.allows_host(&OfficeHost::Word) && settings.allows_host(&OfficeHost::PowerPoint));
        assert!(!settings.allows_host(&OfficeHost::Outlook) && !settings.allows_host(&OfficeHost::Unknown("excel2".into())));
        assert_eq!(settings.ttl_seconds(), DEFAULT_TTL_SECONDS);
        let short = OfficeSettings { bootstrap_ttl_seconds: Some(1), ..OfficeSettings::default() };
        assert_eq!(short.ttl_seconds(), MIN_TTL_SECONDS);
        let parsed: OfficeSettings = serde_json::from_value(json!({"enabled": true, "allowedUsers": [USER]})).unwrap();
        assert_eq!(parsed.allowed_hosts, default_hosts());
        assert!(parsed.allows_user(&USER.to_uppercase()));
    }

    #[test]
    fn profiles_without_office_stay_identical_on_disk() {
        let old = json!({"id": ALPHA, "name": "A", "endpoint": "https://x.example"});
        let legacy: Profile = serde_json::from_value(old).unwrap();
        assert!(legacy.office.is_none() && !legacy.is_office_enabled());
        let value = serde_json::to_value(&legacy).unwrap();
        assert!(value.get("office").is_none());
        let value = serde_json::to_value(profile()).unwrap();
        assert_eq!(value["office"]["enabled"], true);
        assert_eq!(value["office"]["allowedHosts"], json!(["excel", "word", "powerpoint"]));
        assert_eq!(serde_json::from_value::<Profile>(value).unwrap(), profile());
    }

    #[test]
    fn user_ids_parse_and_normalise() {
        let (ids, invalid) = parse_user_ids(&format!("# comment\n{}\n\n{}\nnot-a-guid\n", USER.to_uppercase(), USER));
        assert_eq!(ids, [USER]);
        assert_eq!(invalid, ["not-a-guid"]);
        assert_eq!(pseudonymous_user_id(&USER.to_uppercase(), "s"), pseudonymous_user_id(USER, "s"));
        assert_ne!(pseudonymous_user_id(USER, "s"), pseudonymous_user_id(USER, "t"));
        assert_eq!(pseudonymous_user_id(USER, "s").len(), 16);
    }

    #[test]
    fn resolver_maps_known_users_and_denies_the_rest() {
        let resolver = AssignmentResolver::new(vec![profile()], BTreeMap::new());
        let resolved = resolver.resolve(&identity(), &OfficeHost::Excel).unwrap();
        assert_eq!(resolved.profile.id, ALPHA);
        assert!(resolver.resolve(&identity(), &OfficeHost::Word).is_ok() && resolver.resolve(&identity(), &OfficeHost::PowerPoint).is_ok());
        assert_eq!(resolver.resolve(&identity(), &OfficeHost::Outlook), Err(OfficeDenied::HostNotAllowed(OfficeHost::Outlook)));
        assert_eq!(resolver.resolve(&identity(), &OfficeHost::Unknown(String::new())), Err(OfficeDenied::HostNotAllowed(OfficeHost::Unknown(String::new()))));
        let stranger = EntraIdentity { oid: TENANT.into(), ..identity() };
        assert_eq!(resolver.resolve(&stranger, &OfficeHost::Excel), Err(OfficeDenied::NotAssigned));

        let explicit = AssignmentResolver::new(vec![profile()], BTreeMap::from([(TENANT.to_uppercase(), ALPHA.to_lowercase())]));
        assert!(explicit.resolve(&stranger, &OfficeHost::Excel).is_ok());

        let mut disabled = profile();
        disabled.office.as_mut().unwrap().enabled = false;
        let resolver = AssignmentResolver::new(vec![disabled], BTreeMap::from([(USER.into(), ALPHA.into())]));
        assert_eq!(resolver.resolve(&identity(), &OfficeHost::Excel), Err(OfficeDenied::OfficeDisabled));
        let mut none = profile();
        none.office = None;
        let resolver = AssignmentResolver::new(vec![none], BTreeMap::from([(USER.into(), ALPHA.into())]));
        assert_eq!(resolver.resolve(&identity(), &OfficeHost::Excel), Err(OfficeDenied::OfficeDisabled));
    }

    #[test]
    fn reserved_headers_are_rejected_case_insensitively() {
        for name in ["Authorization", "X-API-KEY", "content-type", "Host", "Content-Length", "User-Agent", "Cookie", "anthropic-version", "X-Amz-Date", "x-goog-user-project"] {
            assert!(is_reserved_inference_header(name), "{name}");
            assert!(!is_valid_inference_header_name(name), "{name}");
        }
        assert!(is_valid_inference_header_name("x-keyzapper-cost-center"));
        assert!(!is_valid_inference_header_name("bad header") && !is_valid_inference_header_name(""));
    }

    #[test]
    fn mcp_mapping_keeps_only_remote_servers_without_credentials() {
        let settings = profile().office.unwrap();
        let mapped = office_mcp_servers(&settings, &servers());
        assert_eq!(mapped.len(), 1);
        assert_eq!((mapped[0].url.as_str(), mapped[0].label.as_str()), ("https://mcp.example.test/search", "search"));
        assert_eq!(mapped[0].headers, BTreeMap::from([("X-Team".to_string(), "dev".to_string())]));
        let mut plain = servers();
        plain[0].url = "http://mcp.example.test/search".into();
        assert!(office_mcp_servers(&settings, &plain).is_empty());
        assert!(is_office_url("https://a.example/x") && !is_office_url("https://localhost/x") && !is_office_url("http://a.example"));
    }

    #[test]
    fn response_is_sparse_and_redacted() {
        let resolved = AssignmentResolver::new(vec![profile()], BTreeMap::new()).resolve(&identity(), &OfficeHost::Excel).unwrap();
        let credential = OfficeCredential { token: "sk-secret-token".into(), expires_at: Some(1_000_000 + 600) };
        let response = build_response(&resolved, &credential, &servers(), 1_000_000);
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["gateway_url"], "https://litellm.example.test");
        assert_eq!(value["gateway_token"], "sk-secret-token");
        assert_eq!(value["gateway_api_format"], "anthropic");
        assert!(value.get("gateway_auth_header").is_none() && value.get("disabled_features").is_none() && value.get("skills").is_none());
        assert_eq!(value["inference_headers"], json!({"x-keyzapper-profile": "Consulting", "x-cost-center": "4711"}));
        assert_eq!(value["mcp_servers"][0]["label"], "search");
        assert_eq!(value["available_models"], json!(["company-sonnet"]));
        assert_eq!(value["bootstrap_expires_at"], 1_000_600);
        let debug = format!("{response:?} {credential:?}");
        assert!(!debug.contains("sk-secret") && debug.contains("<redacted>"), "{debug}");
        assert!(!response.summary().contains("sk-secret"));
        assert_eq!(serde_json::to_value(BootstrapResponse::empty()).unwrap(), json!({}));

        let mut bearer = resolved.clone();
        bearer.settings.gateway_auth_header = GatewayAuthHeader::Authorization;
        let unlimited = OfficeCredential { token: "k".into(), expires_at: None };
        let value = serde_json::to_value(build_response(&bearer, &unlimited, &[], 1_000_000)).unwrap();
        assert_eq!(value["gateway_auth_header"], "authorization");
        assert_eq!(value["bootstrap_expires_at"], 1_000_000 + DEFAULT_TTL_SECONDS as i64);
    }

    #[test]
    fn static_provider_hands_out_only_configured_keys() {
        let provider = StaticKeyProvider::new(BTreeMap::from([(ALPHA.to_lowercase(), " sk-static ".to_string())]));
        assert_eq!(provider.credential_for(&profile(), &identity(), 60).unwrap().token, "sk-static");
        let mut other = profile();
        other.id = TENANT.into();
        assert!(matches!(provider.credential_for(&other, &identity(), 60), Err(CredentialError::NotConfigured(_))));
        assert!(!format!("{provider:?}").contains("sk-static"));
    }

    #[test]
    fn litellm_request_body_tags_profile_and_user() {
        let mut provider = LiteLlmVirtualKeyProvider::new("sk-mgmt".into());
        provider.max_budget = Some(5.0);
        let body = provider.request_body(&profile(), &identity(), 900);
        assert_eq!(body["duration"], "900s");
        assert_eq!(body["models"], json!(["company-sonnet"]));
        assert_eq!(body["max_budget"], 5.0);
        assert_eq!(body["metadata"]["keyzapper_profile"], ALPHA);
        assert_eq!(body["metadata"]["keyzapper_user"], pseudonymous_user_id(USER, ""));
        assert!(!body.to_string().contains("sk-mgmt") && !format!("{provider:?}").contains("sk-mgmt"));
    }

    #[test]
    fn deployment_export_has_no_secrets_and_derives_assignments() {
        let mut state = AppState { profiles: vec![profile(), Profile::new(TENANT.into(), "Plain".into(), "https://b.example".into(), String::new())], mcp_servers: servers(), ..AppState::default() };
        assert!(OfficeDeployment::from_state(&state).is_err());
        state.office_deployment = Some(OfficeDeploymentSettings { tenant_id: TENANT.into(), audience: ANTHROPIC_OFFICE_APP_ID.into(), bootstrap_url: "https://kz.example/office/bootstrap".into() });
        let deployment = OfficeDeployment::from_state(&state).unwrap();
        assert_eq!(deployment.profiles.len(), 1);
        assert_eq!(deployment.assignments, BTreeMap::from([(USER.to_string(), ALPHA.to_string())]));
        assert_eq!(deployment.mcp_servers.len(), 1);
        assert_eq!(deployment.mcp_servers[0].headers, BTreeMap::from([("X-Team".to_string(), "dev".to_string())]));
        assert_eq!(deployment.allowed_origins, [DEFAULT_ALLOWED_ORIGIN]);
        assert_eq!(deployment.issuer(), format!("https://login.microsoftonline.com/{TENANT}/v2.0"));
        assert!(deployment.validate().is_empty(), "{:?}", deployment.validate());
        let text = serde_json::to_string(&deployment).unwrap();
        assert!(!text.contains("Bearer s") && !text.contains("${TOKEN}"));
        let parsed: OfficeDeployment = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, deployment);
        assert!(parsed.resolver().resolve(&identity(), &OfficeHost::Word).is_ok());

        let mut bad = deployment.clone();
        bad.allowed_origins = vec!["*".into()];
        bad.profiles[0].office.as_mut().unwrap().inference_headers.insert("x-api-key".into(), "k".into());
        assert_eq!(bad.validate().len(), 2);
    }
}
