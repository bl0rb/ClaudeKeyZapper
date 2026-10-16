use crate::errors::{Error, Result};
use crate::models::AppState;
use crate::{json, l, paths};
use std::path::{Path, PathBuf};

/// Versioned JSON metadata (profiles and bindings), same format as the macOS app. Contains no secrets.
/// Written only by the app; the helper reads it to validate profile IDs.
#[derive(Clone, Debug)]
pub struct MetadataStore {
    pub file: PathBuf,
    /// Full copy next to the state file. Older app versions rewrite `state.json` without fields they do not know;
    /// on load those fields are restored from this mirror, so a downgrade does not lose them.
    mirror: Option<PathBuf>,
}

impl Default for MetadataStore {
    fn default() -> Self {
        Self::new(paths::data_dir().join("state.json"), true)
    }
}

impl MetadataStore {
    pub fn new(file: PathBuf, mirror: bool) -> Self {
        let mirror = mirror.then(|| file.with_extension("full.json"));
        Self { file, mirror }
    }

    pub fn load(&self) -> Result<AppState> {
        let data = match std::fs::read(&self.file) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AppState::default()),
            Err(_) => return Err(Error::CorruptMetadata(l!("%@ ist nicht lesbar", self.file.display()))),
        };
        let state = Self::decode(&data)?;
        let mirror = self.mirror.as_ref().and_then(|m| std::fs::read(m).ok()).and_then(|d| Self::decode(&d).ok());
        Ok(match mirror {
            Some(mirror) => restoring_dropped_fields(state, &mirror),
            None => state,
        })
    }

    pub fn save(&self, state: &AppState) -> Result<()> {
        if let Some(dir) = self.file.parent() {
            paths::create_private_dir(dir)?;
        }
        let mut current = state.clone();
        current.schema_version = AppState::CURRENT_SCHEMA_VERSION;
        let text = json::to_pretty_sorted(&serde_json::to_value(&current).map_err(|e| Error::Io(e.to_string()))?);
        for file in std::iter::once(&self.file).chain(self.mirror.as_ref()) {
            paths::write_atomic(file, text.as_bytes(), true)?;
        }
        Ok(())
    }

    /// Decodes any known schema version and migrates it to the current one.
    pub fn decode(data: &[u8]) -> Result<AppState> {
        let value: serde_json::Value =
            serde_json::from_slice(data).map_err(|_| Error::CorruptMetadata(l!("schemaVersion fehlt")))?;
        let version = value.get("schemaVersion").and_then(|v| v.as_u64()).ok_or_else(|| Error::CorruptMetadata(l!("schemaVersion fehlt")))?;
        match version {
            1 => serde_json::from_value::<AppState>(value)
                .map(AppState::normalized)
                .map_err(|e| Error::CorruptMetadata(e.to_string())),
            v => Err(Error::UnsupportedSchemaVersion(v as u32)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }
}

/// Copies fields an older app version dropped (model tiers, environment, global profile, deactivation).
pub fn restoring_dropped_fields(state: AppState, mirror: &AppState) -> AppState {
    let mut result = state;
    for profile in result.profiles.iter_mut() {
        let saved = mirror.profile(&profile.id).filter(|s| s.endpoint == profile.endpoint);
        if profile.office.is_none() {
            profile.office = saved.and_then(|s| s.office.clone());
        }
        if profile.opus_model.is_some() || profile.sonnet_model.is_some() || profile.haiku_model.is_some() || profile.environment.is_some() {
            continue;
        }
        if let Some(saved) = saved {
            profile.opus_model = saved.opus_model.clone();
            profile.sonnet_model = saved.sonnet_model.clone();
            profile.haiku_model = saved.haiku_model.clone();
            profile.environment = saved.environment.clone();
        }
    }
    if result.office_deployment.is_none() {
        result.office_deployment = mirror.office_deployment.clone();
    }
    if result.global_binding.is_none() {
        if let Some(global) = mirror.global_binding.as_ref().filter(|g| result.profile(&g.profile_id).is_some()) {
            result.global_binding = Some(global.clone());
        }
    }
    if result.disabled.is_none() {
        result.disabled = mirror.disabled;
    }
    if result.mcp_servers.is_empty() {
        result.mcp_servers = mirror.mcp_servers.clone();
        for binding in result.bindings.iter_mut().filter(|b| b.mcp_server_ids.is_empty()) {
            if let Some(saved) = mirror.bindings.iter().find(|s| s.path == binding.path) {
                binding.mcp_server_ids = saved.mcp_server_ids.clone();
                binding.mcp_managed = saved.mcp_managed.clone();
                binding.mcp_git_exclude_entry = saved.mcp_git_exclude_entry.clone();
            }
        }
    }
    result
}
