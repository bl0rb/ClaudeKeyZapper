//! App state and actions, ported from the macOS app's `AppModel`. Keys live in the key file and are read
//! directly (no keychain, no prompts). Slow work (git status, gateway, updates) runs in `commands`.

use keyzapper_core::audit::Finding;
use keyzapper_core::backup::BackupPayload;
use keyzapper_core::backup_store::BackupStore;
use keyzapper_core::gateway::KeyBudget;
use keyzapper_core::keys::{masked_hint, KeyStore};
use keyzapper_core::managed::{host_of, is_secure_endpoint, ManagedConfig};
use keyzapper_core::metadata::MetadataStore;
use keyzapper_core::models::{new_id, parse_id, AppState, AuthType, McpServer, McpTransport, OidcTokenType, Profile, WorkspaceBinding};
use keyzapper_core::office::{self, GatewayAuthHeader, OfficeDeployment, OfficeDeploymentSettings, OfficeHost, OfficeSettings};
use keyzapper_core::paths::{abbreviate_home, folder_name};
use keyzapper_core::settings::{self, BindingHealth, BindingStatus, SettingsBinder};
use keyzapper_core::update::ReleaseInfo;
use keyzapper_core::tokens::TokenStore;
use keyzapper_core::{agents, cli, helper, l, legacy_keychain, mcp, oidc, update, whats_new};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct Model {
    pub state: AppState,
    pub budgets: HashMap<String, KeyBudget>,
    pub available_update: Option<ReleaseInfo>,
    pub installing_update: bool,
    /// Highlights of the releases since the version last seen, shown once after an update.
    pub whats_new: Vec<whats_new::Release>,
    /// Findings of the logical check of `~/.claude/settings.json`.
    pub global_findings: Vec<Finding>,
    /// State of the default profile in `~/.claude/settings.json`, None if none is set.
    pub global_health: Option<BindingHealth>,
    pub statuses: HashMap<String, BindingStatus>,
    /// Per binding: whether its `.mcp.json` entries are still as written.
    pub mcp_active: HashMap<String, bool>,
    pub outdated_clis: Vec<String>,
    /// Unrestored OneDrive backup found on a fresh install; while set, the backup is not overwritten.
    pub available_backup: Option<AppState>,
    pub backup_problem: Option<String>,
    /// Profiles whose key is still only in the macOS keychain (KeyZapper ≤ 1.x).
    pub keychain_profiles: Vec<String>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub config: ManagedConfig,
    /// None for development builds.
    pub app_version: Option<String>,
    pub keys: KeyStore,
    pub tokens: TokenStore,
    /// SSO profiles whose refresh failed (session expired); cleared on login/logout.
    pub sso_expired: HashSet<String>,
    /// SSO profiles with a browser login in progress.
    pub sso_logging_in: HashSet<String>,
    pub binder: Option<SettingsBinder>,
    pub refresh_generation: u64,
    backup_dir: Option<PathBuf>,
    metadata: MetadataStore,
    /// Set when metadata could not be read (e.g. newer schema); saving is blocked to avoid data loss.
    metadata_unreadable: bool,
}

/// Editor input for a new or changed profile.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInput {
    pub id: Option<String>,
    pub name: String,
    pub endpoint: String,
    pub model_alias: String,
    pub opus_model: String,
    pub sonnet_model: String,
    pub haiku_model: String,
    pub environment_text: String,
    pub key: String,
    pub auth_type: AuthType,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_scope: String,
    pub oidc_token_type: OidcTokenType,
    /// Claude for Microsoft 365 section; absent when the editor did not show it (settings are kept as they are).
    #[serde(default)]
    pub office: Option<OfficeInput>,
}

/// Editor input of the Claude for Microsoft 365 section of a profile.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeInput {
    pub enabled: bool,
    /// `excel`, `word`, `powerpoint`, `outlook`.
    pub allowed_hosts: Vec<String>,
    /// One Entra object ID per line.
    pub users_text: String,
    /// `Name: value` lines.
    pub headers_text: String,
    pub mcp_server_ids: Vec<String>,
    /// Lifetime of a bootstrap response in minutes; empty for the default.
    pub ttl_minutes: String,
    /// `x-api-key` (default) or `authorization`.
    #[serde(default)]
    pub gateway_auth_header: String,
}

/// Tenant, audience and bootstrap URL for the Office export.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficeDeploymentInput {
    pub tenant_id: String,
    pub audience: String,
    pub bootstrap_url: String,
}

/// Editor input for a new or changed MCP server.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub id: Option<String>,
    pub name: String,
    pub transport: McpTransport,
    pub command: String,
    /// One argument per line.
    pub args_text: String,
    /// `NAME=value` lines.
    pub env_text: String,
    pub url: String,
    /// `Name: value` lines.
    pub headers_text: String,
    #[serde(default)]
    pub oauth_client_id: String,
    #[serde(default)]
    pub oauth_callback_port: String,
    #[serde(default)]
    pub oauth_scopes: String,
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// https URL with a host (http only for loopback), as typed (trimmed).
pub fn valid_endpoint(text: &str) -> Option<String> {
    let trimmed = text.trim();
    is_secure_endpoint(trimmed).then(|| trimmed.to_string())
}

impl Model {
    pub fn load(version: Option<String>) -> Self {
        let config = ManagedConfig::load();
        let backup_dir = BackupStore::directory(&config);
        let metadata = MetadataStore::default();
        let mut model = Model {
            state: AppState::default(),
            budgets: HashMap::new(),
            available_update: None,
            installing_update: false,
            whats_new: vec![],
            global_findings: vec![],
            global_health: None,
            statuses: HashMap::new(),
            mcp_active: HashMap::new(),
            outdated_clis: vec![],
            available_backup: None,
            backup_problem: None,
            keychain_profiles: vec![],
            error: None,
            notice: None,
            config,
            app_version: version,
            keys: KeyStore::default(),
            tokens: TokenStore::default(),
            sso_expired: HashSet::new(),
            sso_logging_in: HashSet::new(),
            binder: None,
            refresh_generation: 0,
            backup_dir,
            metadata,
            metadata_unreadable: false,
        };
        match model.metadata.load() {
            Ok(state) => model.state = state,
            Err(e) => {
                model.metadata_unreadable = true;
                model.error = Some(e.to_string());
            }
        }
        model.load_whats_new();
        if model.config.one_drive_backup || model.config.backup_directory.is_some() {
            match &model.backup_dir {
                Some(dir) => {
                    if model.state.profiles.is_empty() {
                        if let Ok(Some(backup)) = BackupStore::new(dir.clone()).read() {
                            if !backup.profiles.is_empty() {
                                model.available_backup = Some(backup);
                            }
                        }
                    }
                }
                None => {
                    model.backup_problem =
                        Some(l!("OneDrive-Backup ist aktiviert, aber es wurde kein OneDrive-Ordner gefunden. Bitte in OneDrive anmelden."))
                }
            }
        }
        match helper_path() {
            Some(path) => model.binder = Some(SettingsBinder::new(path.to_string_lossy().into_owned())),
            None => model.error = Some(l!("Hilfsprogramm %@ wurde nicht gefunden. Bitte App neu installieren.", helper::EXECUTABLE_NAME)),
        }
        model.sync_managed_profiles();
        model.migrate_helper_path();
        model
    }

    /// After an update: the highlights since the version last seen. Existing users updating from a release before this
    /// dialog have no seen version; a fresh install (no profiles yet) shows nothing.
    fn load_whats_new(&mut self) {
        let Some(current) = self.app_version.clone() else { return };
        let seen = whats_new::seen_version();
        self.whats_new = match &seen {
            Some(seen) if update::is_newer(&current, seen) => whats_new::since(Some(seen), &current),
            None if !self.state.profiles.is_empty() => whats_new::since(None, &current),
            _ => vec![],
        };
        if self.whats_new.is_empty() && seen.as_deref() != Some(current.as_str()) {
            let _ = whats_new::mark_seen(&current);
        }
    }

    pub fn dismiss_whats_new(&mut self) {
        if let Some(current) = &self.app_version {
            let _ = whats_new::mark_seen(current);
        }
        self.whats_new.clear();
    }

    pub fn is_disabled(&self) -> bool {
        self.state.is_disabled()
    }

    pub fn minimum_cli_version(&self) -> String {
        self.config.minimum_claude_code_version.clone().unwrap_or_else(|| cli::MINIMUM_TESTED_VERSION.into())
    }

    fn profile(&self, id: &str) -> Option<Profile> {
        self.state.profile(id).cloned()
    }

    fn binding(&self, id: &str) -> Option<WorkspaceBinding> {
        self.state.bindings.iter().find(|b| b.id == id).cloned()
    }

    fn not_allowed_message(&self, endpoint: &str) -> String {
        if !is_secure_endpoint(endpoint) {
            return l!("Der Endpunkt %@ verwendet kein https. Keys werden nur verschlüsselt übertragen (http nur für localhost).", endpoint);
        }
        l!(
            "Der Endpunkt %@ ist laut Firmenrichtlinie nicht freigegeben. Erlaubt: %@",
            host_of(endpoint).unwrap_or_default(),
            self.config.allowed_gateway_hosts.join(", ")
        )
    }

    /// Creates/updates the profiles IT defines via `ManagedProfiles`; developers only add their key.
    fn sync_managed_profiles(&mut self) {
        for managed in self.config.profiles.clone() {
            let mut profile = self.profile(&managed.id).unwrap_or_else(|| {
                Profile::new(managed.id.clone(), managed.name.clone(), managed.endpoint.clone(), managed.model_alias.clone())
            });
            profile.name = managed.name.clone();
            profile.endpoint = managed.endpoint.clone();
            profile.model_alias = managed.model_alias.clone();
            profile.opus_model = managed.opus_model.clone();
            profile.sonnet_model = managed.sonnet_model.clone();
            profile.haiku_model = managed.haiku_model.clone();
            profile.environment = managed.environment.clone();
            profile.auth_type = managed.auth_type;
            profile.oidc_issuer = managed.oidc_issuer.clone();
            profile.oidc_client_id = managed.oidc_client_id.clone();
            profile.oidc_scope = managed.oidc_scope.clone();
            profile.oidc_token_type = managed.oidc_token_type;
            if self.state.profile(&managed.id) != Some(&profile) {
                self.save_profile(profile, None);
            }
        }
    }

    /// Settings written by KeyZapper 1.x (or an install in another place) call a helper that may no longer exist.
    /// Rewrites them for the bundled helper, quietly.
    fn migrate_helper_path(&mut self) {
        let Some(binder) = self.binder.clone() else { return };
        if self.is_disabled() {
            return;
        }
        let notice = self.notice.clone();
        for binding in self.state.bindings.clone() {
            let Some(profile) = self.profile(&binding.profile_id) else { continue };
            let wanted = binder.desired_values(&profile, binding.is_pooled());
            if binding.managed_values.get("apiKeyHelper") != wanted.get("apiKeyHelper") && Path::new(&binding.path).is_dir() {
                self.apply(&profile, &binding.path, Some(&binding), None);
            }
        }
        if let Some(global) = self.state.global_binding.clone() {
            if let Some(profile) = self.profile(&global.profile_id) {
                if global.managed_values.get("apiKeyHelper") != binder.desired_values(&profile, false).get("apiKeyHelper") {
                    self.apply_global(&profile, Some(&global));
                }
            }
        }
        self.notice = notice;
    }

    // MARK: Profiles

    pub fn save_profile_input(&mut self, input: ProfileInput) -> bool {
        let Some(endpoint) = valid_endpoint(&input.endpoint) else {
            self.error = Some(l!("Bitte eine vollständige https-URL angeben (http nur für localhost)."));
            return false;
        };
        let (environment, invalid) = Profile::parse_environment(&input.environment_text);
        if input.name.trim().is_empty() || !invalid.is_empty() {
            return false;
        }
        let existing = input.id.as_deref().and_then(parse_id).and_then(|id| self.profile(&id));
        let sso = existing.as_ref().map_or(input.auth_type, |p| p.auth_type) == AuthType::Oidc;
        if sso {
            if let Err(e) = oidc::validate_url(&input.oidc_issuer) {
                self.error = Some(e.to_string());
                return false;
            }
            if input.oidc_client_id.trim().is_empty() {
                return false;
            }
        } else if existing.is_none() && input.key.trim().is_empty() {
            return false;
        }
        let mut profile = existing.unwrap_or_else(|| Profile::new(new_id(), String::new(), String::new(), String::new()));
        if !self.config.is_managed(&profile.id) {
            profile.name = input.name.trim().to_string();
            profile.endpoint = endpoint;
            profile.model_alias = input.model_alias.trim().to_string();
            profile.opus_model = non_empty(&input.opus_model);
            profile.sonnet_model = non_empty(&input.sonnet_model);
            profile.haiku_model = non_empty(&input.haiku_model);
            profile.environment = (!environment.is_empty()).then_some(environment);
            if sso {
                profile.auth_type = AuthType::Oidc;
                profile.oidc_issuer = Some(input.oidc_issuer.trim().to_string());
                profile.oidc_client_id = Some(input.oidc_client_id.trim().to_string());
                profile.oidc_scope = non_empty(&input.oidc_scope);
                profile.oidc_token_type = input.oidc_token_type;
            }
        }
        if let Some(office) = input.office {
            // The Office release is a local decision, also for IT-defined profiles.
            match self.office_settings(office) {
                Some(settings) => profile.office = settings,
                None => return false,
            }
        }
        self.save_profile(profile, (!sso).then_some(input.key))
    }

    /// Editor section → settings; `Some(None)` when nothing is configured. Reports the first problem via `error`.
    fn office_settings(&mut self, input: OfficeInput) -> Option<Option<OfficeSettings>> {
        let (users, invalid_users) = office::parse_user_ids(&input.users_text);
        if let Some(line) = invalid_users.first() {
            self.error = Some(l!("Keine gültige Entra-Objekt-ID (GUID): %@", line));
            return None;
        }
        let (headers, invalid_headers) = McpServer::parse_pairs(&input.headers_text, ':');
        if let Some(line) = invalid_headers.first() {
            self.error = Some(l!("Ungültig oder nicht erlaubt: %@", line));
            return None;
        }
        if let Some(name) = headers.keys().find(|n| !office::is_valid_inference_header_name(n)) {
            self.error = Some(l!("Der Header „%@“ ist reserviert und darf nicht als Inference-Header gesetzt werden.", name));
            return None;
        }
        let ttl = input.ttl_minutes.trim();
        let bootstrap_ttl_seconds = if ttl.is_empty() {
            None
        } else {
            match ttl.parse::<u64>().ok().map(|m| m * 60).filter(|s| (office::MIN_TTL_SECONDS..=office::MAX_TTL_SECONDS).contains(s)) {
                Some(seconds) if seconds == office::DEFAULT_TTL_SECONDS => None,
                Some(seconds) => Some(seconds),
                None => {
                    self.error = Some(l!("Die Gültigkeit muss zwischen %@ und %@ Minuten liegen.", office::MIN_TTL_SECONDS / 60, office::MAX_TTL_SECONDS / 60));
                    return None;
                }
            }
        };
        let Some(gateway_auth_header) = GatewayAuthHeader::from_name(&input.gateway_auth_header) else {
            self.error = Some(l!("Ungültig oder nicht erlaubt: %@", input.gateway_auth_header));
            return None;
        };
        let mut allowed_hosts: Vec<OfficeHost> = input.allowed_hosts.iter().map(|h| OfficeHost::from_name(h)).filter(OfficeHost::is_known).collect();
        allowed_hosts.sort();
        allowed_hosts.dedup();
        let settings = OfficeSettings {
            enabled: input.enabled,
            allowed_hosts,
            allowed_users: users,
            inference_headers: headers,
            mcp_server_ids: input.mcp_server_ids.iter().filter_map(|id| parse_id(id)).filter(|id| self.state.mcp_server(id).is_some()).collect(),
            bootstrap_ttl_seconds,
            gateway_auth_header,
            ..OfficeSettings::default()
        }
        .normalized();
        Some((!settings.is_empty()).then_some(settings))
    }

    /// Tenant, audience and bootstrap URL used by the Office export (and shown as manifest help).
    pub fn save_office_deployment(&mut self, input: OfficeDeploymentInput) -> bool {
        let Some(tenant_id) = office::parse_oid(&input.tenant_id) else {
            self.error = Some(l!("Bitte die Entra-Tenant-ID (GUID) angeben."));
            return false;
        };
        let audience = input.audience.trim().to_string();
        if audience.is_empty() {
            self.error = Some(l!("Bitte die erwartete Audience des Entra-Tokens angeben."));
            return false;
        }
        let bootstrap_url = input.bootstrap_url.trim().to_string();
        if !bootstrap_url.is_empty() && !office::is_office_url(&bootstrap_url) {
            self.error = Some(l!("Die Bootstrap-URL muss eine öffentliche https-URL sein (kein localhost)."));
            return false;
        }
        self.state.office_deployment = Some(OfficeDeploymentSettings { tenant_id, audience, bootstrap_url });
        self.persist()
    }

    /// The deployment file for `keyzapper-office-bootstrap` (pretty JSON, no secrets).
    pub fn office_deployment_json(&self) -> Result<String, String> {
        let deployment = OfficeDeployment::from_state(&self.state)?;
        serde_json::to_string_pretty(&deployment).map_err(|e| e.to_string())
    }

    pub fn save_profile(&mut self, profile: Profile, new_key: Option<String>) -> bool {
        if !self.config.is_endpoint_allowed(&profile.endpoint) {
            self.error = Some(self.not_allowed_message(&profile.endpoint));
            return false;
        }
        let old = self.profile(&profile.id);
        if profile.is_sso() && old.as_ref().is_some_and(|o| o.oidc_issuer != profile.oidc_issuer || o.oidc_client_id != profile.oidc_client_id) {
            self.sso_logout(&profile.id);
        }
        match self.state.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile.clone(),
            None => self.state.profiles.push(profile.clone()),
        }
        if !self.persist() {
            return false;
        }
        if let Some(key) = new_key.filter(|k| !k.trim().is_empty()) {
            self.store_key(&profile.id, &key);
        }
        if let Some(old) = old.filter(|o| *o != profile) {
            let _ = old;
            if !self.is_disabled() {
                for binding in self.state.bindings.clone().iter().filter(|b| b.profile_id == profile.id) {
                    self.apply(&profile, &binding.path, Some(binding), None);
                }
                if let Some(global) = self.state.global_binding.clone().filter(|g| g.profile_id == profile.id) {
                    self.apply_global(&profile, Some(&global));
                }
            }
        }
        true
    }

    pub fn store_key(&mut self, id: &str, key: &str) -> bool {
        let Some(profile) = self.profile(id) else { return false };
        if !self.config.is_endpoint_allowed(&profile.endpoint) {
            self.error = Some(self.not_allowed_message(&profile.endpoint));
            return false;
        }
        match self.keys.write(id, key) {
            Ok(()) => {
                self.keychain_profiles.retain(|p| p != id);
                self.notice = Some(l!(
                    "Key für „%@“ gespeichert. Neue Claude-Sitzungen verwenden ihn sofort, laufende nach Ablauf des Helper-Caches (Standard 5 min), nach einem 401 oder nach Neustart.",
                    profile.name
                ));
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    pub fn delete_profile(&mut self, id: &str) {
        let Some(profile) = self.profile(id) else { return };
        for binding in self.state.bindings.clone().iter().filter(|b| b.profile_id == profile.id) {
            if !self.unbind(&binding.id) {
                return;
            }
        }
        if let Err(e) = self.keys.delete(id).and_then(|_| oidc::logout(&profile, &self.tokens)) {
            self.error = Some(e.to_string());
            return;
        }
        self.sso_expired.remove(id);
        if let Some(global) = self.state.global_binding.clone().filter(|g| g.profile_id == profile.id) {
            if let Some(binder) = &self.binder {
                let _ = binder.revert_global(&global);
            }
            self.state.global_binding = None;
        }
        self.state.profiles.retain(|p| p.id != id);
        self.budgets.remove(id);
        self.persist();
    }

    /// Signs out locally: removes the tokens (the session at the IdP stays).
    pub fn sso_logout(&mut self, id: &str) {
        let Some(profile) = self.profile(id).filter(|p| p.is_sso()) else { return };
        self.sso_expired.remove(id);
        if let Err(e) = oidc::logout(&profile, &self.tokens) {
            self.error = Some(e.to_string());
        }
    }

    /// Access token of an SSO profile for gateway calls; flags the profile when the session expired.
    pub fn sso_token(&mut self, profile: &Profile) -> Result<String, String> {
        match oidc::access_token(profile, &self.tokens, oidc::unix_now()) {
            Ok(token) => Ok(token),
            Err(e) => {
                if matches!(e, keyzapper_core::errors::Error::SessionExpired(_)) {
                    self.sso_expired.insert(profile.id.clone());
                }
                Err(e.to_string())
            }
        }
    }

    /// The key for copying, or None with an error set.
    pub fn key_for_copy(&mut self, id: &str) -> Option<(String, String)> {
        if !self.config.allow_key_export {
            self.error = Some(l!("Das Kopieren von Keys ist laut Firmenrichtlinie (AllowKeyExport) deaktiviert."));
            return None;
        }
        let profile = self.profile(id).filter(|p| !p.is_sso())?;
        match self.keys.read(id) {
            Ok(key) => Some((key, profile.name)),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    /// Profiles that have a key, with endpoint and key, for gateway calls outside the lock.
    pub fn keyed_profiles(&self) -> Vec<(String, String, String)> {
        let keys = self.keys.all().unwrap_or_default();
        self.state.profiles.iter().filter_map(|p| Some((p.id.clone(), p.endpoint.clone(), keys.get(&p.id)?.clone()))).collect()
    }

    // MARK: Bindings

    pub fn bind(&mut self, folder: &str, profile_id: &str) {
        let Some(profile) = self.profile(profile_id) else { return };
        let root = settings::settings_root(folder);
        let previous = self.state.bindings.iter().find(|b| b.path == root).cloned();
        self.apply(&profile, &root, previous.as_ref(), None);
    }

    pub fn reapply(&mut self, binding_id: &str) {
        let Some(binding) = self.binding(binding_id) else { return };
        let Some(profile) = self.profile(&binding.profile_id) else { return };
        self.apply(&profile, &binding.path, Some(&binding), None);
    }

    /// Projects whose Budget-Killer may burn this profile's key: pooled bindings of other profiles on the same endpoint.
    pub fn budget_killers(&self, profile: &Profile) -> Vec<String> {
        if self.is_disabled() || profile.is_sso() {
            return vec![];
        }
        self.state
            .bindings
            .iter()
            .filter(|b| {
                b.is_pooled()
                    && b.profile_id != profile.id
                    && self.state.profile(&b.profile_id).is_some_and(|p| helper::same_endpoint(&p.endpoint, &profile.endpoint))
            })
            .map(|b| folder_name(&b.path))
            .collect()
    }

    /// Hidden Budget-Killer: switches a project between its own key and the pool of all keys on the same endpoint.
    pub fn toggle_pool(&mut self, binding_id: &str) {
        let Some(binding) = self.binding(binding_id) else { return };
        let Some(profile) = self.profile(&binding.profile_id).filter(|p| !p.is_sso()) else { return };
        let pooled = !binding.is_pooled();
        if !self.apply(&profile, &binding.path, Some(&binding), Some(pooled)) {
            return;
        }
        let folder = folder_name(&binding.path);
        self.notice = Some(if pooled {
            l!("Budget-Killer für „%@“ aktiv: Ist das Budget von „%@“ verbraucht, verbrennt Claude die Keys der anderen Profile am selben Gateway. Laufende Claude-Sitzungen bitte neu starten.", folder, profile.name)
        } else {
            l!("Budget-Killer für „%@“ aus: Claude nutzt nur noch den Key von „%@“. Laufende Claude-Sitzungen bitte neu starten.", folder, profile.name)
        });
    }

    pub fn unbind(&mut self, binding_id: &str) -> bool {
        let Some(binding) = self.binding(binding_id) else { return false };
        let Some(binder) = self.binder.clone() else { return false };
        let mcp_kept = match mcp::revert(&binding.path, &binding.mcp_managed, binding.mcp_git_exclude_entry.as_deref()) {
            Ok(kept) => kept,
            Err(e) => {
                self.error = Some(e.to_string());
                return false;
            }
        };
        match binder.revert(&binding) {
            Ok(mut kept) => {
                kept.extend(mcp_kept.into_iter().map(|n| format!("mcpServers.{n}")));
                self.state.bindings.retain(|b| b.id != binding.id);
                self.statuses.remove(&binding.id);
                self.persist();
                self.notice = Some(if kept.is_empty() {
                    l!("Zuordnung für „%@“ entfernt. Laufende Claude-Sitzungen dort bitte neu starten.", folder_name(&binding.path))
                } else {
                    l!("Zuordnung entfernt. Außerhalb der App geänderte Einträge wurden beibehalten: %@", kept.join(", "))
                });
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    // MARK: Agent instruction files

    /// Profile, folder and credential for the repository analysis of an assigned project.
    pub fn analysis_access(&mut self, binding_id: &str) -> Result<(Profile, String, String), String> {
        let binding = self.binding(binding_id).ok_or_else(|| l!("Unbekannte Zuordnung."))?;
        let profile = self.profile(&binding.profile_id).ok_or_else(|| l!("Unbekanntes Profil: %@", binding.profile_id))?;
        if !self.config.is_endpoint_allowed(&profile.endpoint) {
            return Err(self.not_allowed_message(&profile.endpoint));
        }
        let key = if profile.is_sso() { self.sso_token(&profile)? } else { self.keys.read(&profile.id).map_err(|e| e.to_string())? };
        Ok((profile, binding.path, key))
    }

    /// Writes the reviewed `AGENTS.md` into the assigned folder.
    pub fn write_agents_file(&mut self, binding_id: &str, content: &str) -> bool {
        let Some(binding) = self.binding(binding_id) else { return false };
        if !Path::new(&binding.path).is_dir() {
            self.error = Some(l!("Ordner nicht gefunden: %@", abbreviate_home(&binding.path)));
            return false;
        }
        let target = Path::new(&binding.path).join(agents::FILE_NAME);
        let shown = abbreviate_home(&target.to_string_lossy());
        match keyzapper_core::paths::write_atomic(&target, format!("{}\n", content.trim_end()).as_bytes(), false) {
            Ok(()) => {
                self.notice = Some(l!("%@ gespeichert. Neue Agenten-Sitzungen im Projekt nutzen die Datei.", shown));
                true
            }
            Err(e) => {
                self.error = Some(l!("%@ konnte nicht gespeichert werden: %@", shown, e));
                false
            }
        }
    }

    fn apply(&mut self, profile: &Profile, folder: &str, previous: Option<&WorkspaceBinding>, pooled: Option<bool>) -> bool {
        let Some(binder) = self.binder.clone() else { return false };
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return false;
        }
        let pooled = pooled.unwrap_or_else(|| previous.is_some_and(|p| p.is_pooled()));
        match binder.apply(profile, folder, previous, pooled) {
            Ok(result) => {
                let binding_id = result.binding.id.clone();
                let has_mcp = !result.binding.mcp_server_ids.is_empty() || !result.binding.mcp_managed.is_empty();
                match self.state.bindings.iter_mut().find(|b| b.id == result.binding.id) {
                    Some(existing) => *existing = result.binding,
                    None => self.state.bindings.push(result.binding),
                }
                self.persist();
                if has_mcp {
                    self.apply_mcp(&binding_id, true);
                }
                self.notice = Some(if result.changed {
                    l!("„%@“ verwendet jetzt Profil „%@“. Laufende Claude-Sitzungen in diesem Projekt bitte neu starten.", folder_name(folder), profile.name)
                } else {
                    l!("„%@“ ist bereits eingerichtet – keine Änderungen.", folder_name(folder))
                });
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    fn persist(&mut self) -> bool {
        if self.metadata_unreadable {
            self.error = Some(l!("Metadaten konnten nicht gelesen werden; Änderungen werden nicht gespeichert."));
            return false;
        }
        if let Err(e) = self.metadata.save(&self.state) {
            self.error = Some(e.to_string());
            return false;
        }
        self.write_backup();
        true
    }

    // MARK: MCP servers

    fn mcp_server(&self, id: &str) -> Option<McpServer> {
        self.state.mcp_server(id).cloned()
    }

    pub fn save_mcp_server(&mut self, input: McpServerInput) -> bool {
        let name = input.name.trim().to_string();
        let existing = input.id.as_deref().and_then(parse_id).and_then(|id| self.mcp_server(&id));
        if !McpServer::is_valid_name(&name) {
            self.error = Some(l!("Der Servername darf nur Buchstaben, Ziffern, „-“, „_“ und „.“ enthalten."));
            return false;
        }
        if self.state.mcp_servers.iter().any(|s| s.name == name && existing.as_ref().is_none_or(|e| e.id != s.id)) {
            self.error = Some(l!("Es gibt bereits einen MCP-Server namens „%@“.", name));
            return false;
        }
        let (env, invalid_env) = McpServer::parse_pairs(&input.env_text, '=');
        let (headers, invalid_headers) = McpServer::parse_pairs(&input.headers_text, ':');
        if !invalid_env.is_empty() || !invalid_headers.is_empty() {
            return false;
        }
        let mut server = existing.unwrap_or_else(|| McpServer::new(new_id(), name.clone()));
        server.name = name;
        server.transport = input.transport;
        server.command = String::new();
        server.args = vec![];
        server.env = BTreeMap::new();
        server.url = String::new();
        server.headers = BTreeMap::new();
        server.oauth_client_id = String::new();
        server.oauth_callback_port = None;
        server.oauth_scopes = String::new();
        match input.transport {
            McpTransport::Stdio => {
                let command = input.command.trim().to_string();
                if command.is_empty() {
                    return false;
                }
                server.command = command;
                server.args = McpServer::parse_args(&input.args_text);
                server.env = env;
            }
            McpTransport::Http | McpTransport::Sse => {
                let Some(url) = valid_endpoint(&input.url) else {
                    self.error = Some(l!("Bitte eine vollständige https-URL angeben (http nur für localhost)."));
                    return false;
                };
                server.url = url;
                server.headers = headers;
                server.oauth_client_id = input.oauth_client_id.trim().to_string();
                server.oauth_scopes = input.oauth_scopes.split_whitespace().collect::<Vec<_>>().join(" ");
                let port = input.oauth_callback_port.trim();
                if !port.is_empty() {
                    match port.parse::<u16>() {
                        Ok(port) if port > 0 => server.oauth_callback_port = Some(port),
                        _ => {
                            self.error = Some(l!("Der Callback-Port muss eine Zahl zwischen 1 und 65535 sein."));
                            return false;
                        }
                    }
                }
            }
        }
        let old = self.mcp_server(&server.id);
        match self.state.mcp_servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server.clone(),
            None => self.state.mcp_servers.push(server.clone()),
        }
        if !self.persist() {
            return false;
        }
        self.notice = Some(l!("MCP-Server „%@“ gespeichert.", server.name));
        if old.is_some_and(|o| o != server) {
            for binding in self.state.bindings.clone().iter().filter(|b| b.mcp_server_ids.contains(&server.id)) {
                self.apply_mcp(&binding.id, false);
            }
        }
        true
    }

    pub fn delete_mcp_server(&mut self, id: &str) {
        let Some(server) = self.mcp_server(id) else { return };
        for binding in self.state.bindings.clone().iter().filter(|b| b.mcp_server_ids.contains(&server.id)) {
            let ids = binding.mcp_server_ids.iter().filter(|i| **i != server.id).cloned().collect();
            if !self.set_project_mcp_servers(&binding.id, ids) {
                return;
            }
        }
        self.state.mcp_servers.retain(|s| s.id != server.id);
        self.persist();
        self.notice = Some(l!("MCP-Server „%@“ gelöscht.", server.name));
    }

    /// Chooses the servers a project uses and writes them; on failure the previous choice is kept.
    pub fn set_project_mcp_servers(&mut self, binding_id: &str, ids: Vec<String>) -> bool {
        let ids: Vec<String> = ids.into_iter().filter(|id| self.state.mcp_server(id).is_some()).collect();
        let Some(binding) = self.state.bindings.iter_mut().find(|b| b.id == binding_id) else { return false };
        let previous = std::mem::replace(&mut binding.mcp_server_ids, ids);
        if self.apply_mcp(binding_id, false) {
            return true;
        }
        if let Some(binding) = self.state.bindings.iter_mut().find(|b| b.id == binding_id) {
            binding.mcp_server_ids = previous;
        }
        false
    }

    /// Writes the project's MCP servers into its `.mcp.json`; `quiet` keeps the current notice.
    fn apply_mcp(&mut self, binding_id: &str, quiet: bool) -> bool {
        let Some(binding) = self.binding(binding_id) else { return false };
        let servers: Vec<McpServer> = binding.mcp_server_ids.iter().filter_map(|id| self.mcp_server(id)).collect();
        match mcp::apply(&binding.path, &servers, &binding.mcp_managed, binding.mcp_git_exclude_entry.as_deref()) {
            Ok(result) => {
                let names: Vec<String> = result.managed.keys().cloned().collect();
                if let Some(b) = self.state.bindings.iter_mut().find(|b| b.id == binding_id) {
                    b.mcp_managed = result.managed;
                    b.mcp_git_exclude_entry = result.git_exclude_entry;
                }
                self.persist();
                if !quiet {
                    let folder = folder_name(&binding.path);
                    self.notice = Some(if names.is_empty() {
                        l!("MCP-Server für „%@“ entfernt. Laufende Claude-Sitzungen dort bitte neu starten.", folder)
                    } else if result.changed {
                        l!("„%@“ nutzt jetzt die MCP-Server %@. Laufende Claude-Sitzungen dort bitte neu starten.", folder, names.join(", "))
                    } else {
                        l!("MCP-Server für „%@“ sind bereits eingerichtet – keine Änderungen.", folder)
                    });
                }
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    /// Takes over MCP servers from a backup (new ones added, known ones updated).
    fn merge_mcp_servers(&mut self, servers: &[McpServer]) {
        for server in servers {
            match self.state.mcp_servers.iter_mut().find(|s| s.id == server.id) {
                Some(existing) => *existing = server.clone(),
                None => self.state.mcp_servers.push(server.clone()),
            }
        }
    }

    // MARK: OneDrive backup (profiles and bindings only, never keys)

    fn write_backup(&mut self) {
        let Some(dir) = self.backup_dir.clone() else { return };
        if self.available_backup.is_some() {
            return;
        }
        match BackupStore::new(dir).write(&self.state) {
            Ok(()) => self.backup_problem = None,
            Err(e) => self.backup_problem = Some(l!("OneDrive-Backup fehlgeschlagen: %@", e)),
        }
    }

    pub fn restore_from_backup(&mut self) {
        let Some(backup) = self.available_backup.take() else { return };
        for profile in &backup.profiles {
            if self.state.profile(&profile.id).is_none() {
                self.state.profiles.push(profile.clone());
            }
        }
        self.merge_mcp_servers(&backup.mcp_servers);
        if !self.persist() {
            return;
        }
        let mut restored = 0;
        for binding in &backup.bindings {
            let Some(profile) = self.profile(&binding.profile_id) else { continue };
            if Path::new(&binding.path).exists() && self.apply(&profile, &binding.path, Some(binding), None) {
                restored += 1;
            }
        }
        let skipped = backup.bindings.len() - restored;
        let mut notice = l!("Backup wiederhergestellt: %@ Profil(e), %@ Projekt(e)", backup.profiles.len(), restored);
        if skipped > 0 {
            notice += &l!(", %@ übersprungen (Ordner fehlt oder Konflikt)", skipped);
        }
        notice += &l!(". Keys werden nicht gesichert – bitte je Profil neu eintragen.");
        self.notice = Some(notice);
    }

    pub fn discard_backup(&mut self) {
        self.available_backup = None;
        self.write_backup();
    }

    // MARK: Encrypted backup file (keys and settings)

    /// Profiles, assignments and (unless IT forbids it) the keys.
    pub fn backup_payload(&self) -> BackupPayload {
        let keys = if self.config.allow_key_export {
            let all = self.keys.all().unwrap_or_default();
            self.state.profiles.iter().filter(|p| !p.is_sso()).filter_map(|p| Some((p.id.clone(), all.get(&p.id)?.clone()))).collect()
        } else {
            BTreeMap::new()
        };
        BackupPayload {
            created_at: keyzapper_core::time::iso8601_utc(),
            app_version: self.app_version.clone(),
            state: self.state.clone(),
            keys,
        }
    }

    /// Restores profiles and keys from an encrypted backup and re-assigns projects whose folders exist here.
    pub fn import_payload(&mut self, payload: BackupPayload) -> bool {
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return false;
        }
        let mut skipped = 0;
        let mut changed_profiles = Vec::new();
        for profile in &payload.state.profiles {
            if !self.config.is_endpoint_allowed(&profile.endpoint) {
                skipped += 1;
                continue;
            }
            let managed = self.config.is_managed(&profile.id);
            match self.state.profiles.iter_mut().find(|p| p.id == profile.id) {
                Some(existing) => {
                    if !managed && existing != profile {
                        *existing = profile.clone();
                        changed_profiles.push(profile.id.clone());
                    }
                }
                None => self.state.profiles.push(profile.clone()),
            }
        }
        self.merge_mcp_servers(&payload.state.mcp_servers);
        if !self.persist() {
            return false;
        }
        let mut key_count = 0;
        for (id, key) in &payload.keys {
            let Some(id) = parse_id(id) else { continue };
            if self.state.profile(&id).is_some() && self.keys.write(&id, key).is_ok() {
                key_count += 1;
                self.keychain_profiles.retain(|p| *p != id);
            }
        }
        let mut project_count = 0;
        for binding in &payload.state.bindings {
            let profile = self.profile(&binding.profile_id);
            let Some(profile) = profile.filter(|_| Path::new(&binding.path).exists()) else {
                skipped += 1;
                continue;
            };
            let previous = self.state.bindings.iter().find(|b| b.path == binding.path).cloned().unwrap_or_else(|| binding.clone());
            if self.apply(&profile, &binding.path, Some(&previous), None) {
                project_count += 1;
            } else {
                skipped += 1;
            }
        }
        // Existing assignments of profiles the backup changed must pick up the new endpoint and models too.
        let imported: Vec<&String> = payload.state.bindings.iter().map(|b| &b.path).collect();
        for binding in self.state.bindings.clone() {
            if changed_profiles.contains(&binding.profile_id) && !imported.contains(&&binding.path) {
                if let Some(profile) = self.profile(&binding.profile_id) {
                    self.apply(&profile, &binding.path, Some(&binding), None);
                }
            }
        }
        if let Some(global) = self.state.global_binding.clone().filter(|g| changed_profiles.contains(&g.profile_id)) {
            if let Some(profile) = self.profile(&global.profile_id) {
                self.apply_global(&profile, Some(&global));
            }
        }
        let mut notice = l!(
            "Backup importiert: %@ Profil(e), %@ Key(s), %@ Projekt(e).",
            payload.state.profiles.len(),
            key_count,
            project_count
        );
        if skipped > 0 {
            notice += &l!(" %@ Eintrag/Einträge übersprungen (Ordner fehlt, Konflikt oder nicht freigegebener Endpunkt).", skipped);
        }
        self.notice = Some(notice);
        true
    }

    // MARK: ~/.claude/settings.json (default profile, repair) and deactivation

    /// Sets (or with None removes) the default profile in `~/.claude/settings.json` for folders without assignment.
    pub fn set_global_profile(&mut self, profile_id: Option<String>) {
        let Some(binder) = self.binder.clone() else { return };
        if self.is_disabled() {
            self.error = Some(l!("KeyZapper ist deaktiviert. Bitte zuerst wieder aktivieren."));
            return;
        }
        if let Some(profile) = profile_id.and_then(|id| self.profile(&id)) {
            let backup = if self.state.global_binding.is_none() { binder.backup_user_settings().ok().flatten() } else { None };
            let previous = self.state.global_binding.clone();
            if self.apply_global(&profile, previous.as_ref()) {
                let mut notice =
                    l!("Standardprofil „%@“ in ~/.claude/settings.json eingetragen. Es gilt für alle Ordner ohne eigene Zuordnung.", profile.name);
                if let Some(backup) = backup {
                    notice += &l!(" Sicherung: %@", file_name(&backup));
                }
                self.notice = Some(notice);
            }
        } else if let Some(global) = self.state.global_binding.clone() {
            match binder.revert_global(&global) {
                Ok(_) => {
                    self.state.global_binding = None;
                    self.persist();
                    self.notice = Some(l!("Standardprofil aus ~/.claude/settings.json entfernt."));
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
    }

    fn apply_global(&mut self, profile: &Profile, previous: Option<&WorkspaceBinding>) -> bool {
        let Some(binder) = self.binder.clone() else { return false };
        match binder.apply_global(profile, previous) {
            Ok(result) => {
                self.state.global_binding = Some(result.binding);
                self.persist();
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }

    /// Backs up and cleans `~/.claude/settings.json` (valid JSON, no plaintext keys) or creates it.
    pub fn repair_user_settings(&mut self) {
        let Some(binder) = self.binder.clone() else { return };
        match binder.repair_user_settings() {
            Ok(backup) => {
                if !self.is_disabled() {
                    if let Some(global) = self.state.global_binding.clone() {
                        if let Some(profile) = self.profile(&global.profile_id) {
                            self.apply_global(&profile, Some(&global));
                        }
                    }
                }
                let mut notice = l!("~/.claude/settings.json ist jetzt gültig und frei von Klartext-Keys.");
                if let Some(backup) = backup {
                    notice += &l!(" Sicherung: %@", file_name(&backup));
                }
                self.notice = Some(notice);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    /// Deactivation removes KeyZapper's values from all projects and the user settings but keeps the assignments,
    /// so activating writes them again. Claude Code meanwhile uses its normal login everywhere.
    pub fn set_disabled(&mut self, disable: bool) {
        let Some(binder) = self.binder.clone() else { return };
        if disable == self.is_disabled() {
            return;
        }
        if disable {
            let failed: Vec<String> =
                self.state.bindings.iter().filter(|b| binder.revert(b).is_err()).map(|b| folder_name(&b.path)).collect();
            if let Some(global) = &self.state.global_binding {
                let _ = binder.revert_global(global);
            }
            self.state.disabled = Some(true);
            self.persist();
            self.notice = Some(l!("KeyZapper ist deaktiviert. Claude Code nutzt jetzt überall die normale Anmeldung; laufende Sitzungen bitte neu starten."));
            if !failed.is_empty() {
                self.error = Some(l!("Diese Projekte konnten nicht zurückgesetzt werden: %@", failed.join(", ")));
            }
        } else {
            self.state.disabled = None;
            self.persist();
            for binding in self.state.bindings.clone() {
                if let Some(profile) = self.profile(&binding.profile_id) {
                    self.apply(&profile, &binding.path, Some(&binding), None);
                }
            }
            if let Some(global) = self.state.global_binding.clone() {
                if let Some(profile) = self.profile(&global.profile_id) {
                    self.apply_global(&profile, Some(&global));
                }
            }
            self.notice = Some(l!("KeyZapper ist wieder aktiv. Laufende Claude-Sitzungen bitte neu starten."));
        }
    }

    // MARK: Keys from the macOS keychain (KeyZapper ≤ 1.x)

    /// Profile IDs without a key in the file, for the (prompt-free) keychain presence check.
    pub fn profiles_without_key(&self) -> Vec<String> {
        let keys = self.keys.all().unwrap_or_default();
        self.state.profiles.iter().filter(|p| !keys.contains_key(&p.id)).map(|p| p.id.clone()).collect()
    }

    /// Moves the keys over; macOS may ask once per key.
    pub fn migrate_keychain(&mut self) {
        let mut moved = 0;
        for id in self.keychain_profiles.clone() {
            if let Some(key) = legacy_keychain::take_key(&id) {
                if self.keys.write(&id, &key).is_ok() {
                    moved += 1;
                    self.keychain_profiles.retain(|p| *p != id);
                }
            }
        }
        if self.keychain_profiles.is_empty() {
            self.notice = Some(l!("%@ Key(s) aus dem Schlüsselbund übernommen. KeyZapper nutzt den Schlüsselbund nicht mehr.", moved));
        } else {
            self.error = Some(l!("%@ Key(s) konnten nicht aus dem Schlüsselbund gelesen werden. Bitte über „Key hinterlegen“ neu eintragen.", self.keychain_profiles.len()));
        }
    }

    // MARK: View for the web UI

    pub fn view(&self) -> View {
        let keys = self.keys.all().unwrap_or_default();
        let tokens = self.tokens.all().unwrap_or_default();
        let user_settings_path = self.binder.as_ref().map(|b| b.user_settings_path.clone()).unwrap_or_else(settings::default_user_settings_path);
        View {
            platform: if cfg!(windows) { "windows" } else { "macos" },
            language: keyzapper_core::i18n::language(),
            app_version: self.app_version.clone(),
            disabled: self.is_disabled(),
            profiles: self
                .state
                .profiles
                .iter()
                .map(|p| {
                    let key = keys.get(&p.id).filter(|k| !k.is_empty());
                    let session = tokens.get(&p.id).filter(|t| !t.refresh_token.is_empty());
                    ProfileView {
                        sso: p.is_sso(),
                        oidc_issuer: p.oidc_issuer.clone(),
                        oidc_client_id: p.oidc_client_id.clone(),
                        oidc_scope: p.oidc_scope.clone(),
                        oidc_token_type: p.oidc_token_type,
                        sso_state: p.is_sso().then_some(if self.sso_expired.contains(&p.id) {
                            "expired"
                        } else if session.is_some() {
                            "loggedIn"
                        } else {
                            "loggedOut"
                        }),
                        sso_expires_at: session.and_then(|t| t.expires_at),
                        sso_logging_in: self.sso_logging_in.contains(&p.id),
                        id: p.id.clone(),
                        name: p.name.clone(),
                        endpoint: p.endpoint.clone(),
                        model_alias: p.model_alias.clone(),
                        opus_model: p.opus_model.clone(),
                        sonnet_model: p.sonnet_model.clone(),
                        haiku_model: p.haiku_model.clone(),
                        environment_text: Profile::format_environment(p.environment.as_ref()),
                        managed: self.config.is_managed(&p.id),
                        has_key: key.is_some(),
                        key_hint: key.map(|k| masked_hint(k)),
                        budget: key.filter(|_| !p.is_sso()).and(self.budgets.get(&p.id)).map(|b| BudgetView {
                            spend: b.spend,
                            max_budget: b.max_budget,
                            remaining: b.max_budget.map(|_| b.remaining()),
                            reset_at: b.reset_at.clone(),
                        }),
                        killers: self.budget_killers(p),
                        office: p.office.as_ref().map(|o| OfficeView {
                            enabled: o.enabled,
                            allowed_hosts: o.allowed_hosts.iter().map(|h| h.as_str().to_string()).collect(),
                            users_text: o.allowed_users.join("\n"),
                            headers_text: McpServer::format_pairs(&o.inference_headers, ": "),
                            mcp_server_ids: o.mcp_server_ids.clone(),
                            ttl_minutes: o.bootstrap_ttl_seconds.map(|s| (s / 60).to_string()).unwrap_or_default(),
                            gateway_auth_header: o.gateway_auth_header.as_str().to_string(),
                        }),
                    }
                })
                .collect(),
            bindings: self
                .state
                .bindings
                .iter()
                .map(|b| BindingView {
                    id: b.id.clone(),
                    path: b.path.clone(),
                    display_path: abbreviate_home(&b.path),
                    name: folder_name(&b.path),
                    profile_id: b.profile_id.clone(),
                    pooled: b.is_pooled(),
                    mcp_server_ids: b.mcp_server_ids.clone(),
                    mcp_names: b.mcp_server_ids.iter().filter_map(|id| self.state.mcp_server(id)).map(|s| s.name.clone()).collect(),
                    mcp_active: self.mcp_active.get(&b.id).copied().unwrap_or(true),
                    status: self.statuses.get(&b.id).map(|s| StatusView {
                        health: HealthView::from(&s.health),
                        conflicts: s.conflicts.iter().map(|c| ConflictView { blocking: c.blocking, message: c.message.clone() }).collect(),
                    }),
                })
                .collect(),
            mcp_servers: self
                .state
                .mcp_servers
                .iter()
                .map(|s| McpServerView {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    transport: s.transport,
                    command: s.command.clone(),
                    args_text: s.args.join("\n"),
                    env_text: McpServer::format_pairs(&s.env, "="),
                    url: s.url.clone(),
                    headers_text: McpServer::format_pairs(&s.headers, ": "),
                    oauth_client_id: s.oauth_client_id.clone(),
                    oauth_callback_port: s.oauth_callback_port.map(|p| p.to_string()).unwrap_or_default(),
                    oauth_scopes: s.oauth_scopes.clone(),
                    used_by: self.state.bindings.iter().filter(|b| b.mcp_server_ids.contains(&s.id)).map(|b| folder_name(&b.path)).collect(),
                })
                .collect(),
            global_profile_id: self.state.global_binding.as_ref().map(|g| g.profile_id.clone()),
            global_health: self.global_health.as_ref().map(HealthView::from),
            global_findings: self.global_findings.clone(),
            user_settings_exists: Path::new(&user_settings_path).exists(),
            user_settings_path: abbreviate_home(&user_settings_path),
            user_settings_full_path: user_settings_path,
            available_backup: self.available_backup.as_ref().map(|b| BackupView { profiles: b.profiles.len(), projects: b.bindings.len() }),
            backup_problem: self.backup_problem.clone(),
            outdated_clis: self.outdated_clis.clone(),
            minimum_cli_version: self.minimum_cli_version(),
            available_update: self.available_update.as_ref().map(|u| UpdateView {
                version: u.version.clone(),
                prerelease: u.prerelease,
                page_url: u.page_url.clone(),
            }),
            installing_update: self.installing_update,
            whats_new: self.whats_new.clone(),
            keychain_keys: self.keychain_profiles.len(),
            notice: self.notice.clone(),
            error: self.error.clone(),
            allow_key_export: self.config.allow_key_export,
            update_check_enabled: self.config.update_check_enabled,
            allowed_hosts: self.config.allowed_gateway_hosts.clone(),
            default_endpoint: self.config.default_endpoint.clone(),
            default_model_alias: self.config.default_model_alias.clone(),
            project_url: format!("https://github.com/{}", keyzapper_core::update::REPOSITORY),
            minimum_password_length: keyzapper_core::backup::MINIMUM_PASSWORD_LENGTH,
            office_deployment: self.state.office_deployment.as_ref().map(|d| OfficeDeploymentView {
                tenant_id: d.tenant_id.clone(),
                audience: d.audience.clone(),
                bootstrap_url: d.bootstrap_url.clone(),
            }),
            office_default_audience: office::ANTHROPIC_OFFICE_APP_ID,
            office_default_ttl_minutes: office::DEFAULT_TTL_SECONDS / 60,
            office_profiles: self.state.profiles.iter().filter(|p| p.is_office_enabled()).count(),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `keyzapper-helper` next to the app executable (`KeyZapper.app/Contents/MacOS`, the install folder on Windows).
pub fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = format!("{}{}", helper::EXECUTABLE_NAME, std::env::consts::EXE_SUFFIX);
    let path = exe.parent()?.join(name);
    path.is_file().then_some(path)
}

/// Project status, the default profile and the settings check; computed outside the lock.
pub fn status_snapshot(state: &AppState, binder: &SettingsBinder) -> (HashMap<String, BindingStatus>, HashMap<String, bool>, Option<BindingHealth>, Vec<Finding>) {
    let global_values = state.global_binding.as_ref().map(|g| g.managed_values.clone()).unwrap_or_default();
    let mut statuses = HashMap::new();
    let mut mcp_active = HashMap::new();
    let mut global_health = None;
    for binding in &state.bindings {
        mcp_active.insert(binding.id.clone(), mcp::is_active(&binding.path, &binding.mcp_managed));
    }
    if !state.is_disabled() {
        for binding in &state.bindings {
            if let Some(profile) = state.profile(&binding.profile_id) {
                statuses.insert(binding.id.clone(), binder.inspect(binding, profile, &global_values));
            }
        }
        if let Some(global) = &state.global_binding {
            if let Some(profile) = state.profile(&global.profile_id) {
                global_health = Some(binder.inspect_global(global, profile));
            }
        }
    }
    let findings = keyzapper_core::audit::check(&binder.user_settings_path, &global_values);
    (statuses, mcp_active, global_health, findings)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    platform: &'static str,
    language: &'static str,
    app_version: Option<String>,
    disabled: bool,
    profiles: Vec<ProfileView>,
    bindings: Vec<BindingView>,
    mcp_servers: Vec<McpServerView>,
    global_profile_id: Option<String>,
    global_health: Option<HealthView>,
    global_findings: Vec<Finding>,
    user_settings_path: String,
    user_settings_full_path: String,
    user_settings_exists: bool,
    available_backup: Option<BackupView>,
    backup_problem: Option<String>,
    outdated_clis: Vec<String>,
    minimum_cli_version: String,
    available_update: Option<UpdateView>,
    installing_update: bool,
    whats_new: Vec<whats_new::Release>,
    keychain_keys: usize,
    notice: Option<String>,
    error: Option<String>,
    allow_key_export: bool,
    update_check_enabled: bool,
    allowed_hosts: Vec<String>,
    default_endpoint: Option<String>,
    default_model_alias: Option<String>,
    project_url: String,
    minimum_password_length: usize,
    office_deployment: Option<OfficeDeploymentView>,
    office_default_audience: &'static str,
    office_default_ttl_minutes: u64,
    /// Profiles released for Claude for Microsoft 365 (enables the export).
    office_profiles: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OfficeDeploymentView {
    tenant_id: String,
    audience: String,
    bootstrap_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OfficeView {
    enabled: bool,
    allowed_hosts: Vec<String>,
    users_text: String,
    headers_text: String,
    mcp_server_ids: Vec<String>,
    ttl_minutes: String,
    gateway_auth_header: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileView {
    id: String,
    sso: bool,
    oidc_issuer: Option<String>,
    oidc_client_id: Option<String>,
    oidc_scope: Option<String>,
    oidc_token_type: OidcTokenType,
    /// `loggedIn`, `loggedOut` or `expired` for SSO profiles.
    sso_state: Option<&'static str>,
    sso_expires_at: Option<i64>,
    sso_logging_in: bool,
    name: String,
    endpoint: String,
    model_alias: String,
    opus_model: Option<String>,
    sonnet_model: Option<String>,
    haiku_model: Option<String>,
    environment_text: String,
    managed: bool,
    has_key: bool,
    key_hint: Option<String>,
    budget: Option<BudgetView>,
    killers: Vec<String>,
    /// Claude for Microsoft 365 section, None when never configured.
    office: Option<OfficeView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BudgetView {
    spend: f64,
    max_budget: Option<f64>,
    remaining: Option<f64>,
    reset_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BindingView {
    id: String,
    path: String,
    display_path: String,
    name: String,
    profile_id: String,
    pooled: bool,
    mcp_server_ids: Vec<String>,
    mcp_names: Vec<String>,
    /// False if `.mcp.json` no longer contains what the app wrote.
    mcp_active: bool,
    status: Option<StatusView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpServerView {
    id: String,
    name: String,
    transport: McpTransport,
    command: String,
    args_text: String,
    env_text: String,
    url: String,
    headers_text: String,
    oauth_client_id: String,
    oauth_callback_port: String,
    oauth_scopes: String,
    /// Names of the projects using this server.
    used_by: Vec<String>,
}

#[derive(Serialize)]
struct StatusView {
    health: HealthView,
    conflicts: Vec<ConflictView>,
}

#[derive(Serialize)]
struct ConflictView {
    blocking: bool,
    message: String,
}

#[derive(Serialize)]
struct HealthView {
    /// `active`, `notApplied`, `drifted` or `folderMissing`.
    state: &'static str,
    keys: Vec<String>,
}

impl From<&BindingHealth> for HealthView {
    fn from(health: &BindingHealth) -> Self {
        match health {
            BindingHealth::Active => HealthView { state: "active", keys: vec![] },
            BindingHealth::NotApplied => HealthView { state: "notApplied", keys: vec![] },
            BindingHealth::Drifted(keys) => HealthView { state: "drifted", keys: keys.clone() },
            BindingHealth::FolderMissing => HealthView { state: "folderMissing", keys: vec![] },
        }
    }
}

#[derive(Serialize)]
struct BackupView {
    profiles: usize,
    projects: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateView {
    version: String,
    prerelease: bool,
    page_url: String,
}
