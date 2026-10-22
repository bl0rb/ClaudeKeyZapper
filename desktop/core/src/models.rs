use crate::claude_desktop::DesktopSso;
use crate::claude_office::{OfficeApplied, OfficeClient};
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
/// `Subscription` uses no gateway: Claude Code's own claude.ai login (Pro, Max, Team, Enterprise).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthType {
    #[default]
    ApiKey,
    Oidc,
    Subscription,
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
    /// Claude for Microsoft 365 behind this profile's company gateway (sideloaded add-in); None = not configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office: Option<OfficeClient>,
    /// Claude Desktop signs in itself with the gateway's OIDC client instead of using the helper; None = helper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_desktop: Option<DesktopSso>,
}

fn placeholder_credential() -> CredentialReference {
    CredentialReference::for_id("")
}

/// Models of one tenant for a project binding; replaces the profile's model values, not helper or endpoint.
/// Gateway model IDs of a multi-tenant gateway look like `<tenant>/<model>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub tenant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opus_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonnet_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haiku_model: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_alias: String,
}

impl ModelSelection {
    /// The model whose spend the budget is asked for (`?model=`): Sonnet, else Opus, else Haiku.
    pub fn budget_model(&self) -> Option<&str> {
        [&self.sonnet_model, &self.opus_model, &self.haiku_model].into_iter().flatten().map(|m| m.trim()).find(|m| !m.is_empty())
    }
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
               oidc_token_type: OidcTokenType::Access, office: None, claude_desktop: None }
    }

    pub fn is_sso(&self) -> bool {
        self.auth_type == AuthType::Oidc
    }

    pub fn is_subscription(&self) -> bool {
        self.auth_type == AuthType::Subscription
    }

    /// IDs from files are normalised and `credential` is always derived from the ID.
    pub fn normalized(mut self) -> Self {
        if let Some(id) = parse_id(&self.id) {
            self.id = id;
        }
        self.credential = CredentialReference::for_id(&self.id);
        self
    }

    /// Environment entries KeyZapper writes for the model settings (only non-empty values).
    pub fn model_environment(&self) -> BTreeMap<String, String> {
        Self::model_pairs(&self.model_alias, self.opus_model.as_deref(), self.sonnet_model.as_deref(), self.haiku_model.as_deref())
    }

    /// Like `model_environment`, but with the models of a project's tenant selection instead of the profile's own.
    /// Nothing of the profile's models is kept, so a tier the tenant lacks is not written at all.
    pub fn model_environment_for(&self, selection: Option<&ModelSelection>) -> BTreeMap<String, String> {
        match selection {
            Some(s) => Self::model_pairs(&s.model_alias, s.opus_model.as_deref(), s.sonnet_model.as_deref(), s.haiku_model.as_deref()),
            None => self.model_environment(),
        }
    }

    fn model_pairs(alias: &str, opus: Option<&str>, sonnet: Option<&str>, haiku: Option<&str>) -> BTreeMap<String, String> {
        let pairs = [
            ("ANTHROPIC_MODEL", Some(alias)),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", opus),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", sonnet),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", haiku),
        ];
        pairs
            .into_iter()
            .filter_map(|(name, value)| {
                let trimmed = value?.trim();
                (!trimmed.is_empty()).then(|| (name.to_string(), trimmed.to_string()))
            })
            .collect()
    }

    /// The model whose spend the profile's own budget is asked for (`?model=`): like `ModelSelection::budget_model` Sonnet,
    /// else Opus, else Haiku; failing those the default model, unless it is one of Claude Code's tier aliases (`opus`,
    /// `sonnet[1m]`, …), which are no gateway IDs.
    pub fn budget_model(&self) -> Option<&str> {
        [&self.sonnet_model, &self.opus_model, &self.haiku_model]
            .into_iter()
            .flatten()
            .map(|m| m.trim())
            .find(|m| !m.is_empty())
            .or_else(|| Some(self.model_alias.trim()).filter(|alias| !alias.is_empty() && !is_tier_alias(alias)))
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

/// The tenants of a multi-tenant gateway: the distinct, non-empty prefixes before the first `/` of its model IDs
/// (`<tenant>/<model>`), naturally sorted. IDs without `/` belong to no tenant.
pub fn tenants_of(models: &[String]) -> Vec<String> {
    let mut tenants: Vec<String> = models
        .iter()
        .filter_map(|id| id.split_once('/').map(|(tenant, _)| tenant))
        .filter(|tenant| !tenant.is_empty())
        .map(String::from)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    tenants.sort_by(|a, b| natural_cmp(a, b).then_with(|| a.cmp(b)));
    tenants
}

/// One budget the gateway is asked for: the tenant it is shown under (None = the profile itself) and the model
/// passed as `?model=` (None = the gateway decides).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetTarget {
    pub tenant: Option<String>,
    pub model: Option<String>,
}

/// The budgets to load for `profile`. A key profile has one, without model. An SSO profile has one per distinct tenant
/// of its bindings' model selections (asked for the selection's `budget_model()`, tenants sorted), plus one for the profile
/// itself with its `budget_model()`; that one is left out when its model belongs to a tenant that is already listed (the
/// tenant prefix `<tenant>/` is compared, not the model). Without any of them (no tenant entry, no model that can be asked
/// for, e.g. only a tier alias as default model) the user's budget is asked for without a model, as before tenants existed.
pub fn budget_targets(profile: &Profile, bindings: &[WorkspaceBinding]) -> Vec<BudgetTarget> {
    let none = BudgetTarget { tenant: None, model: None };
    if !profile.is_sso() {
        return vec![none];
    }
    let mut tenants: Vec<(&str, &str)> = vec![];
    for selection in bindings.iter().filter(|b| b.profile_id == profile.id).filter_map(|b| b.models.as_ref()) {
        if let Some(model) = selection.budget_model().filter(|_| !tenants.iter().any(|(tenant, _)| *tenant == selection.tenant)) {
            tenants.push((selection.tenant.as_str(), model));
        }
    }
    tenants.sort_by(|a, b| natural_cmp(a.0, b.0).then_with(|| a.0.cmp(b.0)));
    let mut targets: Vec<BudgetTarget> =
        tenants.iter().map(|(tenant, model)| BudgetTarget { tenant: Some(tenant.to_string()), model: Some(model.to_string()) }).collect();
    match profile.budget_model() {
        Some(model) => {
            let listed = model.split_once('/').is_some_and(|(tenant, _)| tenants.iter().any(|(t, _)| *t == tenant));
            if !listed {
                targets.push(BudgetTarget { tenant: None, model: Some(model.to_string()) });
            }
        }
        None if targets.is_empty() => targets.push(none),
        None => {}
    }
    targets
}

/// The profile's model values moved to `tenant`; None if no model ID starts with `<tenant>/`.
/// Per tier the profile's value (`<x>/<name>` or `<name>`) becomes `<tenant>/<name>` when the tenant has that model,
/// otherwise the tenant's newest-looking model of the tier is used (like `suggest_models`; none if it has none).
/// The default model: empty stays empty, the tier aliases (`opus`, `sonnet`, `haiku`, `opusplan`, also with `[1m]`) stay,
/// a model ID moves along if the tenant has it, else the Sonnet tier of the tenant is used.
pub fn selection_for_tenant(models: &[String], tenant: &str, profile: &Profile) -> Option<ModelSelection> {
    let prefix = format!("{tenant}/");
    let names: Vec<&str> = models.iter().filter_map(|id| id.strip_prefix(prefix.as_str())).collect();
    if tenant.is_empty() || names.is_empty() {
        return None;
    }
    // Suggestions run over the names without prefix, so a tenant called e.g. "opus-lab" cannot be mistaken for a tier.
    let (opus, sonnet, haiku) = suggest_models(&names.iter().map(|n| n.to_string()).collect::<Vec<_>>());
    // `<x>/<name>` or `<name>` of the profile, found in the tenant (a name that itself contains `/` is tried as written too).
    let moved = |value: &str| -> Option<String> {
        let value = value.trim();
        value.split_once('/').map(|(_, name)| name).into_iter().chain(Some(value)).find(|n| !n.is_empty() && names.contains(n)).map(String::from)
    };
    let tier = |value: &Option<String>, suggested: Option<String>| -> Option<String> {
        value.as_deref().and_then(moved).or(suggested).map(|name| format!("{prefix}{name}"))
    };
    let (opus_model, sonnet_model, haiku_model) = (tier(&profile.opus_model, opus), tier(&profile.sonnet_model, sonnet), tier(&profile.haiku_model, haiku));
    let alias = profile.model_alias.trim();
    let model_alias = if alias.is_empty() {
        String::new()
    } else if is_tier_alias(alias) {
        alias.to_string()
    } else {
        moved(alias).map(|name| format!("{prefix}{name}")).or_else(|| sonnet_model.clone()).unwrap_or_default()
    };
    Some(ModelSelection { tenant: tenant.to_string(), opus_model, sonnet_model, haiku_model, model_alias })
}

/// Claude Code's own model aliases, optionally with the 1M-context suffix: they resolve through the tier variables.
fn is_tier_alias(alias: &str) -> bool {
    let lower = alias.to_lowercase();
    matches!(lower.strip_suffix("[1m]").unwrap_or(&lower), "opus" | "sonnet" | "haiku" | "opusplan")
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
        self.definition_for(cfg!(windows))
    }

    /// Like `definition`; on Windows, npm launchers (`npx`, `npm`, … or `*.cmd`/`*.bat`) run via `cmd /c`, since
    /// Claude Code starts the command without a shell there.
    pub fn definition_for(&self, windows: bool) -> serde_json::Value {
        let mut entry = serde_json::Map::new();
        let map = |m: &BTreeMap<String, String>| serde_json::to_value(m).unwrap_or_default();
        match self.transport {
            McpTransport::Stdio => {
                let (command, args) = if windows && Self::needs_cmd_wrapper(&self.command) {
                    ("cmd".to_string(), ["/c".to_string(), self.command.clone()].into_iter().chain(self.args.clone()).collect())
                } else {
                    (self.command.clone(), self.args.clone())
                };
                entry.insert("type".into(), "stdio".into());
                entry.insert("command".into(), command.into());
                if !args.is_empty() {
                    entry.insert("args".into(), args.into());
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

    /// True for commands that are `.cmd`/`.bat` scripts on Windows and cannot be started without a shell.
    fn needs_cmd_wrapper(command: &str) -> bool {
        let file = command.rsplit(['/', '\\']).next().unwrap_or(command).to_ascii_lowercase();
        match file.rsplit_once('.') {
            Some((_, ext)) => matches!(ext, "cmd" | "bat"),
            None => matches!(file.as_str(), "npx" | "npm" | "pnpm" | "pnpx" | "yarn"),
        }
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
    /// Models of one tenant of a multi-tenant gateway instead of the profile's own; None = as in the profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<ModelSelection>,
}

impl WorkspaceBinding {
    pub fn new(path: String, profile_id: String) -> Self {
        Self { id: new_id(), path, profile_id, managed_values: BTreeMap::new(), git_exclude_entry: None, pooled: None,
               mcp_server_ids: vec![], mcp_managed: BTreeMap::new(), mcp_git_exclude_entry: None, models: None }
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
    /// Update checks offer beta releases too (switch in the app). None = only while a beta is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta_updates: Option<bool>,
    /// MCP servers that can be assigned to projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// What KeyZapper last registered for Claude in Office on this machine; the sync compares against it and never reads
    /// the Office containers. Machine-specific: a backup restore does not take it over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office_applied: Option<OfficeApplied>,
    /// Why the last registration or removal did not fully work. While set, the Office status shows it and only a repeated
    /// request (settings change, refresh) tries the registration again, not every start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub office_error: Option<String>,
    /// The profile KeyZapper last set up Claude Desktop with. Kept while KeyZapper is deactivated (the entry is removed
    /// then and written again on activation); the sign-in variant's entry does not name its profile itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_desktop_profile_id: Option<String>,
    /// The configuration Claude Desktop applied before KeyZapper's entry; applied again when KeyZapper removes its entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_desktop_previous_applied: Option<String>,
    /// Why the background installation of Claude Desktop (`ClaudeDesktopAutoInstall`) failed; while set, only an explicit
    /// refresh tries again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_desktop_install_error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA_VERSION,
            profiles: vec![],
            bindings: vec![],
            global_binding: None,
            disabled: None,
            beta_updates: None,
            mcp_servers: vec![],
            office_applied: None,
            office_error: None,
            claude_desktop_profile_id: None,
            claude_desktop_previous_applied: None,
            claude_desktop_install_error: None,
        }
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
