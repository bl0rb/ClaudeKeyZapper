use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Profile IDs are uppercase UUID strings, as written by the macOS app (`apiKeyHelper … --profile <ID>`).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// Normalised profile ID, or None if `text` is no UUID.
pub fn parse_id(text: &str) -> Option<String> {
    uuid::Uuid::parse_str(text.trim()).ok().map(|id| id.hyphenated().to_string().to_uppercase())
}

/// Kept for file compatibility with the macOS app; KeyZapper no longer uses the keychain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialReference {
    pub service: String,
    pub account: String,
}

impl CredentialReference {
    pub const DEFAULT_SERVICE: &'static str = "KeyZapper.LiteLLM";
    pub fn for_id(id: &str) -> Self {
        Self { service: Self::DEFAULT_SERVICE.into(), account: id.into() }
    }
}

/// How a profile authenticates against the gateway: static LiteLLM key or OIDC access token (SSO).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthType {
    #[default]
    ApiKey,
    Oidc,
}

/// Which token of the OIDC token response an SSO profile hands to the gateway as bearer token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OidcTokenType {
    #[default]
    Access,
    Id,
}

impl OidcTokenType {
    pub fn is_access(&self) -> bool {
        *self == Self::Access
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// LiteLLM base URL, written to `env.ANTHROPIC_BASE_URL`.
    pub endpoint: String,
    /// Optional model alias, written to `env.ANTHROPIC_MODEL` when non-empty.
    #[serde(default)]
    pub model_alias: String,
    /// Gateway model names for Claude Code's model tiers (`env.ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opus_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonnet_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haiku_model: Option<String>,
    /// Further environment variables for Claude Code, e.g. `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<BTreeMap<String, String>>,
    #[serde(default = "placeholder_credential")]
    pub credential: CredentialReference,
    #[serde(default)]
    pub auth_type: AuthType,
    /// OIDC issuer URL (Entra ID tenant or Keycloak realm), only for `AuthType::Oidc`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc_issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc_scope: Option<String>,
    /// `access` (default) or `id`: the gateway expects the OIDC ID token instead of the access token.
    #[serde(default, skip_serializing_if = "OidcTokenType::is_access")]
    pub oidc_token_type: OidcTokenType,
    /// Release for Claude for Microsoft 365 (bootstrap endpoint); absent for profiles that only serve Claude Code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<crate::office::OfficeSettings>,
}

fn placeholder_credential() -> CredentialReference {
    CredentialReference::for_id("")
}

/// Names that the extra environment must not set: credentials, endpoint, model fields and provider switches.
pub const RESERVED_ENVIRONMENT_NAMES: [&str; 10] = [
    "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY",
];

impl Profile {
    pub fn new(id: String, name: String, endpoint: String, model_alias: String) -> Self {
        let credential = CredentialReference::for_id(&id);
        Self { id, name, endpoint, model_alias, opus_model: None, sonnet_model: None, haiku_model: None, environment: None, credential,
               auth_type: AuthType::ApiKey, oidc_issuer: None, oidc_client_id: None, oidc_scope: None,
               oidc_token_type: OidcTokenType::Access, office: None }
    }

    /// Released for Claude for Microsoft 365.
    pub fn is_office_enabled(&self) -> bool {
        self.office.as_ref().is_some_and(|o| o.enabled)
    }

    pub fn is_sso(&self) -> bool {
        self.auth_type == AuthType::Oidc
    }

    /// IDs from files are normalised and `credential` is always derived from the ID.
    pub fn normalized(mut self) -> Self {
        if let Some(id) = parse_id(&self.id) {
            self.id = id;
        }
        self.credential = CredentialReference::for_id(&self.id);
        self.office = self.office.map(crate::office::OfficeSettings::normalized);
        self
    }

    /// Environment entries KeyZapper writes for the model settings (only non-empty values).
    pub fn model_environment(&self) -> BTreeMap<String, String> {
        let pairs = [
            ("ANTHROPIC_MODEL", Some(&self.model_alias)),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", self.opus_model.as_ref()),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", self.sonnet_model.as_ref()),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", self.haiku_model.as_ref()),
        ];
        pairs
            .into_iter()
            .filter_map(|(name, value)| {
                let trimmed = value?.trim();
                (!trimmed.is_empty()).then(|| (name.to_string(), trimmed.to_string()))
            })
            .collect()
    }

    /// All configured model names, used to verify them against the gateway.
    pub fn configured_models(&self) -> Vec<String> {
        let mut models: Vec<String> = self.model_environment().into_values().collect::<HashSet<_>>().into_iter().collect();
        models.sort();
        models
    }

    pub fn is_allowed_environment_name(name: &str) -> bool {
        let mut chars = name.chars();
        let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        valid && !RESERVED_ENVIRONMENT_NAMES.contains(&name)
    }

    /// Parses `NAME=value` lines; returns the valid entries and the lines that were rejected.
    pub fn parse_environment(text: &str) -> (BTreeMap<String, String>, Vec<String>) {
        let mut values = BTreeMap::new();
        let mut invalid = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split_once('=') {
                Some((name, value)) if Self::is_allowed_environment_name(name.trim()) => {
                    values.insert(name.trim().to_string(), value.trim().to_string());
                }
                _ => invalid.push(line.to_string()),
            }
        }
        (values, invalid)
    }

    pub fn format_environment(values: Option<&BTreeMap<String, String>>) -> String {
        values.map(|v| v.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    }
}

/// Picks the newest-looking model name per tier from a gateway model list (by name, e.g. "sonnet-5" over "sonnet-4-5").
pub fn suggest_models(models: &[String]) -> (Option<String>, Option<String>, Option<String>) {
    let best = |tier: &str| {
        models.iter().filter(|m| m.to_lowercase().contains(tier)).max_by(|a, b| natural_cmp(a, b)).cloned()
    };
    (best("opus"), best("sonnet"), best("haiku"))
}

/// Case-insensitive comparison with digit runs compared by value (like Finder's `localizedStandardCompare`).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(c);
                        it.next();
                    }
                    n.trim_start_matches('0').to_string()
                };
                let (na, nb) = (take(&mut a), take(&mut b));
                let order = na.len().cmp(&nb.len()).then(na.cmp(&nb));
                if order.is_ne() {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order.is_ne() {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// How Claude Code reaches an MCP server: a local process (stdio) or a remote endpoint (http, sse).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
    Sse,
}

/// An MCP server, written as one `mcpServers` entry into the `.mcp.json` of the projects that use it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub id: String,
    /// Key in `mcpServers`, e.g. `github`; Claude Code names the tools `mcp__<name>__…`.
    pub name: String,
    #[serde(default)]
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// OAuth for remote servers (e.g. Weave-MCP via Entra ID or Keycloak): pre-registered public client ID.
    /// Claude Code runs the browser login itself (`/mcp`) and keeps the tokens.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub oauth_client_id: String,
    /// Fixed callback port, so `http://localhost:<port>/callback` can be registered at the identity provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_callback_port: Option<u16>,
    /// Space-separated scopes to request, e.g. `mcp.read` or `api://<id>/mcp.read`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub oauth_scopes: String,
}

impl McpServer {
    pub fn new(id: String, name: String) -> Self {
        Self { id, name, transport: McpTransport::Stdio, command: String::new(), args: vec![], env: BTreeMap::new(), url: String::new(), headers: BTreeMap::new(),
               oauth_client_id: String::new(), oauth_callback_port: None, oauth_scopes: String::new() }
    }

    pub fn is_valid_name(name: &str) -> bool {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    }

    /// The `.mcp.json` entry, e.g. `{"type": "stdio", "command": "npx", "args": […], "env": {…}}` or `{"type": "http", "url": …, "headers": {…}}`.
    pub fn definition(&self) -> serde_json::Value {
        let mut entry = serde_json::Map::new();
        let map = |m: &BTreeMap<String, String>| serde_json::to_value(m).unwrap_or_default();
        match self.transport {
            McpTransport::Stdio => {
                entry.insert("type".into(), "stdio".into());
                entry.insert("command".into(), self.command.clone().into());
                if !self.args.is_empty() {
                    entry.insert("args".into(), self.args.clone().into());
                }
                if !self.env.is_empty() {
                    entry.insert("env".into(), map(&self.env));
                }
            }
            transport => {
                entry.insert("type".into(), if transport == McpTransport::Http { "http" } else { "sse" }.into());
                entry.insert("url".into(), self.url.clone().into());
                if !self.headers.is_empty() {
                    entry.insert("headers".into(), map(&self.headers));
                }
                let mut oauth = serde_json::Map::new();
                if !self.oauth_client_id.is_empty() {
                    oauth.insert("clientId".into(), self.oauth_client_id.clone().into());
                }
                if let Some(port) = self.oauth_callback_port {
                    oauth.insert("callbackPort".into(), port.into());
                }
                if !self.oauth_scopes.is_empty() {
                    oauth.insert("scopes".into(), self.oauth_scopes.clone().into());
                }
                if !oauth.is_empty() {
                    entry.insert("oauth".into(), serde_json::Value::Object(oauth));
                }
            }
        }
        serde_json::Value::Object(entry)
    }

    /// One argument per line; blank lines are skipped.
    pub fn parse_args(text: &str) -> Vec<String> {
        text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    /// `NAME=value` lines (environment) or `Name: value` lines (headers); returns the entries and the rejected lines.
    pub fn parse_pairs(text: &str, separator: char) -> (BTreeMap<String, String>, Vec<String>) {
        let mut values = BTreeMap::new();
        let mut invalid = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split_once(separator) {
                Some((name, value)) if Self::is_valid_pair_name(name.trim(), separator) => {
                    values.insert(name.trim().to_string(), value.trim().to_string());
                }
                _ => invalid.push(line.to_string()),
            }
        }
        (values, invalid)
    }

    fn is_valid_pair_name(name: &str, separator: char) -> bool {
        let mut chars = name.chars();
        if separator == '=' {
            matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        } else {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }
    }

    pub fn format_pairs(values: &BTreeMap<String, String>, separator: &str) -> String {
        values.iter().map(|(k, v)| format!("{k}{separator}{v}")).collect::<Vec<_>>().join("\n")
    }

    fn normalized(mut self) -> Self {
        if let Some(id) = parse_id(&self.id) {
            self.id = id;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceBinding {
    pub id: String,
    /// Canonical path of the folder whose `.claude/settings.local.json` Claude Code reads
    /// (the main worktree root for git repositories).
    pub path: String,
    #[serde(rename = "profileID")]
    pub profile_id: String,
    /// Exactly what the app wrote, keyed by settings key path (e.g. `env.ANTHROPIC_BASE_URL`).
    /// Used for idempotent re-apply and for reverting only app-made changes.
    #[serde(default)]
    pub managed_values: BTreeMap<String, String>,
    /// Line the app appended to `<git-common-dir>/info/exclude`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_exclude_entry: Option<String>,
    /// Hidden pool mode: once the profile's budget is used up, the helper hands out other keys on the same endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pooled: Option<bool>,
    /// MCP servers (IDs) this project uses; written to `<root>/.mcp.json`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_server_ids: Vec<String>,
    /// Exactly the `mcpServers` entries the app wrote, so only those are reverted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_managed: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_git_exclude_entry: Option<String>,
}

impl WorkspaceBinding {
    pub fn new(path: String, profile_id: String) -> Self {
        Self { id: new_id(), path, profile_id, managed_values: BTreeMap::new(), git_exclude_entry: None, pooled: None,
               mcp_server_ids: vec![], mcp_managed: BTreeMap::new(), mcp_git_exclude_entry: None }
    }

    pub fn is_pooled(&self) -> bool {
        self.pooled == Some(true)
    }

    fn normalized(mut self) -> Self {
        for id in std::iter::once(&mut self.id).chain(std::iter::once(&mut self.profile_id)).chain(self.mcp_server_ids.iter_mut()) {
            if let Some(n) = parse_id(id) {
                *id = n;
            }
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub schema_version: u32,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub bindings: Vec<WorkspaceBinding>,
    /// Default profile written into `~/.claude/settings.json` for folders without their own assignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_binding: Option<WorkspaceBinding>,
    /// True while KeyZapper is deactivated: assignments stay saved, their settings are removed from the files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// MCP servers that can be assigned to projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// Entra tenant, audience and bootstrap URL for the Claude for Microsoft 365 export (non-secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office_deployment: Option<crate::office::OfficeDeploymentSettings>,
}

impl Default for AppState {
    fn default() -> Self {
        Self { schema_version: Self::CURRENT_SCHEMA_VERSION, profiles: vec![], bindings: vec![], global_binding: None, disabled: None, mcp_servers: vec![], office_deployment: None }
    }
}

impl AppState {
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled == Some(true)
    }

    pub fn mcp_server(&self, id: &str) -> Option<&McpServer> {
        self.mcp_servers.iter().find(|s| s.id == id)
    }

    pub fn normalized(mut self) -> Self {
        self.profiles = self.profiles.into_iter().map(Profile::normalized).collect();
        self.bindings = self.bindings.into_iter().map(WorkspaceBinding::normalized).collect();
        self.global_binding = self.global_binding.map(WorkspaceBinding::normalized);
        self.mcp_servers = self.mcp_servers.into_iter().map(McpServer::normalized).collect();
        self
    }
}
