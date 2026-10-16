use keyzapper_core::backup::{self, BackupError, BackupPayload};
use keyzapper_core::errors::Error;
use keyzapper_core::helper::{ExitCode, HelperCommand};
use keyzapper_core::keys::KeyStore;
use keyzapper_core::managed::ManagedConfig;
use keyzapper_core::metadata::MetadataStore;
use keyzapper_core::models::{AppState, AuthType, Profile, WorkspaceBinding};
use keyzapper_core::tokens::{TokenSet, TokenStore};
use keyzapper_core::paths::canonical_path;
use keyzapper_core::settings::{self, BindingHealth, SettingsBinder, GIT_EXCLUDE_LINE};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const ALPHA: &str = "6F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9";
const BETA: &str = "0A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9";

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kz-{}", uuid::Uuid::new_v4())).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    canonical_path(&dir.to_string_lossy())
}

fn sample() -> Profile {
    let mut p = Profile::new(ALPHA.into(), "Alpha".into(), "https://litellm.example.test/".into(), "claude-alpha".into());
    p.opus_model = Some("team-opus".into());
    p
}

fn other() -> Profile {
    Profile::new(BETA.into(), "Beta".into(), "https://litellm-b.example.test".into(), String::new())
}

fn binder() -> SettingsBinder {
    SettingsBinder {
        helper_path: "/Applications/Project AI'Switch.app/Contents/MacOS/keyzapper-helper".into(),
        managed_settings_paths: vec![],
        user_settings_path: "/nonexistent/settings.json".into(),
    }
}

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn write(path: &str, value: Value) {
    std::fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args).current_dir(dir).output().unwrap();
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_init(dir: &str) {
    assert!(git(dir, &["init", "-q"]).is_some() && git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]).is_some());
}

fn local(root: &str) -> String {
    format!("{root}/.claude/settings.local.json")
}

#[test]
fn applies_idempotently_and_preserves_other_settings() {
    let root = temp_dir("proj with space");
    write(&local(&root), json!({"permissions": {"allow": ["Bash(ls)"]}, "env": {"FOO": "bar"}}));
    let first = binder().apply(&sample(), &root, None, false).unwrap();
    assert!(first.changed);
    let settings = read(&local(&root));
    assert_eq!(settings["permissions"]["allow"], json!(["Bash(ls)"]));
    assert_eq!(settings["env"]["FOO"], "bar");
    assert_eq!(settings["env"]["ANTHROPIC_BASE_URL"], "https://litellm.example.test");
    assert_eq!(settings["env"]["ANTHROPIC_MODEL"], "claude-alpha");
    assert_eq!(settings["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "team-opus");
    assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "");
    assert!(settings["apiKeyHelper"].as_str().unwrap().ends_with(&format!("credential --profile {ALPHA}")));

    let bytes = std::fs::read(local(&root)).unwrap();
    let second = binder().apply(&sample(), &root, Some(&first.binding), false).unwrap();
    assert!(!second.changed);
    assert_eq!(second.binding, first.binding);
    assert_eq!(std::fs::read(local(&root)).unwrap(), bytes);
    assert_eq!(binder().inspect(&first.binding, &sample(), &BTreeMap::new()).health, BindingHealth::Active);
}

#[cfg(unix)]
#[test]
fn helper_command_survives_spaces_and_quotes_in_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("helper dir's");
    let helper = format!("{dir}/keyzapper-helper");
    std::fs::write(&helper, "#!/bin/sh\nprintf 'ok:%s' \"$3\"\n").unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let b = SettingsBinder { helper_path: helper, ..binder() };
    let command = b.desired_values(&sample(), false)["apiKeyHelper"].clone();
    let out = Command::new("/bin/sh").arg("-c").arg(&command).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("ok:{ALPHA}"));
}

/// Claude Code runs `apiKeyHelper` through `cmd /d /s /c` on Windows.
#[cfg(windows)]
#[test]
fn helper_command_survives_spaces_in_path_under_cmd() {
    use std::os::windows::process::CommandExt;
    let dir = temp_dir("helper dir");
    let helper = format!("{dir}\\keyzapper-helper.cmd");
    std::fs::write(&helper, "@echo ok:%3\r\n").unwrap();
    let b = SettingsBinder { helper_path: helper, ..binder() };
    let command = b.desired_values(&sample(), false)["apiKeyHelper"].clone();
    let out = Command::new("cmd.exe").raw_arg(format!("/d /s /c \"{command}\"")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("ok:{ALPHA}"));
}

#[test]
fn toggles_pool_mode_and_removes_its_cache_setting_again() {
    let root = temp_dir("proj");
    let pooled = binder().apply(&sample(), &root, None, true).unwrap();
    assert!(pooled.binding.is_pooled());
    assert!(read(&local(&root))["apiKeyHelper"].as_str().unwrap().ends_with(&format!("pool --profile {ALPHA}")));
    let plain = binder().apply(&sample(), &root, Some(&pooled.binding), false).unwrap();
    assert_eq!(plain.binding.pooled, None);
    assert!(read(&local(&root))["env"].get("CLAUDE_CODE_API_KEY_HELPER_TTL_MS").is_none());
    assert_eq!(binder().inspect(&plain.binding, &sample(), &BTreeMap::new()).health, BindingHealth::Active);
}

#[test]
fn foreign_values_are_conflicts_and_left_untouched() {
    let root = temp_dir("proj");
    write(&local(&root), json!({"apiKeyHelper": "/usr/local/bin/other", "env": {"ANTHROPIC_API_KEY": "sk-user"}}));
    let before = std::fs::read(local(&root)).unwrap();
    match binder().apply(&sample(), &root, None, false) {
        Err(Error::SettingsConflicts(conflicts)) => {
            assert_eq!(conflicts.len(), 2);
            assert!(!conflicts.iter().any(|c| c.message.contains("sk-user")));
        }
        other => panic!("expected conflicts, got {:?}", other.err()),
    }
    assert_eq!(std::fs::read(local(&root)).unwrap(), before);
}

#[test]
fn switching_profile_replaces_only_own_values() {
    let root = temp_dir("proj");
    let a = binder().apply(&sample(), &root, None, false).unwrap();
    let b = binder().apply(&other(), &root, Some(&a.binding), false).unwrap();
    let env = &read(&local(&root))["env"];
    assert_eq!(env["ANTHROPIC_BASE_URL"], "https://litellm-b.example.test");
    assert!(env.get("ANTHROPIC_MODEL").is_none());
    assert_eq!(b.binding.id, a.binding.id);
    assert_eq!(b.binding.profile_id, BETA);
}

#[test]
fn user_edited_value_blocks_reapply_and_survives_revert() {
    let root = temp_dir("proj");
    let a = binder().apply(&sample(), &root, None, false).unwrap();
    let mut settings = read(&local(&root));
    settings["env"]["ANTHROPIC_MODEL"] = json!("user-choice");
    settings["model"] = json!("opus");
    write(&local(&root), settings);
    assert_eq!(
        binder().inspect(&a.binding, &sample(), &BTreeMap::new()).health,
        BindingHealth::Drifted(vec!["env.ANTHROPIC_MODEL".into()])
    );
    assert!(binder().apply(&sample(), &root, None, false).is_err());
    assert_eq!(binder().revert(&a.binding).unwrap(), vec!["env.ANTHROPIC_MODEL".to_string()]);
    let after = read(&local(&root));
    assert!(after.get("apiKeyHelper").is_none());
    assert_eq!(after["model"], "opus");
    assert_eq!(after["env"], json!({"ANTHROPIC_MODEL": "user-choice"}));
}

#[test]
fn revert_deletes_file_created_only_for_the_binding() {
    let root = temp_dir("proj");
    let a = binder().apply(&sample(), &root, None, false).unwrap();
    assert!(binder().revert(&a.binding).unwrap().is_empty());
    assert!(!Path::new(&local(&root)).exists());
    assert!(!Path::new(&format!("{root}/.claude")).exists());
    assert_eq!(binder().inspect(&a.binding, &sample(), &BTreeMap::new()).health, BindingHealth::NotApplied);
}

#[test]
fn invalid_json_is_not_overwritten_and_missing_folder_is_reported() {
    let root = temp_dir("proj");
    std::fs::create_dir_all(format!("{root}/.claude")).unwrap();
    std::fs::write(local(&root), "{ broken").unwrap();
    assert!(binder().apply(&sample(), &root, None, false).is_err());
    assert_eq!(std::fs::read_to_string(local(&root)).unwrap(), "{ broken");

    let gone = format!("{root}/gone");
    let binding = WorkspaceBinding::new(gone.clone(), ALPHA.into());
    assert_eq!(binder().inspect(&binding, &sample(), &BTreeMap::new()).health, BindingHealth::FolderMissing);
    assert_eq!(binder().apply(&sample(), &gone, None, false).err(), Some(Error::FolderNotFound(canonical_path(&gone))));
}

#[test]
fn trailing_comma_is_reported_and_repaired_by_reapply() {
    let root = temp_dir("proj");
    let applied = binder().apply(&sample(), &root, None, false).unwrap();
    let text = std::fs::read_to_string(local(&root)).unwrap();
    std::fs::write(local(&root), text.replacen("\"apiKeyHelper\"", "\"model\": \"opus\",\n  \"apiKeyHelper\"", 1).replace("\n}", ",\n}")).unwrap();
    assert!(settings::is_unparsable_json(&local(&root)));
    let status = binder().inspect(&applied.binding, &sample(), &BTreeMap::new());
    assert!(status.conflicts.iter().any(|c| c.blocking));
    let again = binder().apply(&sample(), &root, Some(&applied.binding), false).unwrap();
    assert!(again.changed);
    assert!(!settings::is_unparsable_json(&local(&root)));
    assert_eq!(read(&local(&root))["model"], "opus");
}

#[test]
fn managed_settings_and_provider_switches_block() {
    let root = temp_dir("proj");
    let managed = format!("{}/managed-settings.json", temp_dir("managed"));
    write(&managed, json!({"apiKeyHelper": "/opt/corp/helper"}));
    let user = format!("{}/settings.json", temp_dir("user"));
    write(&user, json!({"env": {"CLAUDE_CODE_USE_BEDROCK": "1", "ANTHROPIC_API_KEY": "x"}}));
    let strict = SettingsBinder { helper_path: "/h".into(), managed_settings_paths: vec![managed], user_settings_path: user };
    let conflicts = strict.environment_conflicts(&root, &BTreeMap::new());
    assert_eq!(conflicts.iter().filter(|c| c.blocking).count(), 2);
    assert_eq!(conflicts.iter().filter(|c| !c.blocking).count(), 1);
    assert!(strict.apply(&sample(), &root, None, false).is_err());
    assert!(!Path::new(&local(&root)).exists());
}

#[test]
fn global_profile_is_written_reverted_and_repair_removes_plaintext_keys() {
    let user = format!("{}/settings.json", temp_dir("home"));
    std::fs::write(&user, r#"{"model": "opus", "env": {"ANTHROPIC_API_KEY": "sk-plain", "DEBUG": 1, }, }"#).unwrap();
    let b = SettingsBinder { user_settings_path: user.clone(), ..binder() };
    let backup = b.repair_user_settings().unwrap().unwrap();
    assert!(backup.exists());
    let repaired = read(&user);
    assert!(repaired["env"].get("ANTHROPIC_API_KEY").is_none());
    assert_eq!(repaired["env"]["DEBUG"], "1");
    assert!(repaired["$schema"].is_string());

    let global = b.apply_global(&sample(), None).unwrap();
    assert_eq!(b.inspect_global(&global.binding, &sample()), BindingHealth::Active);
    assert!(b.revert_global(&global.binding).unwrap().is_empty());
    let after = read(&user);
    assert!(after.get("apiKeyHelper").is_none());
    assert_eq!(after["model"], "opus");
}

#[test]
fn subfolders_and_worktrees_resolve_to_main_root_and_git_exclude_is_managed() {
    let repo = temp_dir("repo");
    git_init(&repo);
    std::fs::create_dir_all(format!("{repo}/sub/deep")).unwrap();
    let worktree = canonical_path(&format!("{}/repo wt", Path::new(&repo).parent().unwrap().display()));
    assert!(git(&repo, &["worktree", "add", "-q", "--detach", &worktree]).is_some());
    assert_eq!(settings::settings_root(&format!("{repo}/sub/deep")), repo);
    assert_eq!(settings::settings_root(&worktree), repo);
    assert!(matches!(binder().apply(&sample(), &format!("{repo}/sub"), None, false), Err(Error::NotSettingsRoot { .. })));

    let first = binder().apply(&sample(), &repo, None, false).unwrap();
    assert_eq!(first.binding.git_exclude_entry.as_deref(), Some(GIT_EXCLUDE_LINE));
    assert!(git(&repo, &["check-ignore", "-q", ".claude/settings.local.json"]).is_some());
    assert_eq!(git(&repo, &["status", "--porcelain"]).as_deref(), Some(""));
    let second = binder().apply(&sample(), &repo, Some(&first.binding), false).unwrap();
    let exclude = std::fs::read_to_string(format!("{repo}/.git/info/exclude")).unwrap();
    assert_eq!(exclude.matches(GIT_EXCLUDE_LINE).count(), 1);
    binder().revert(&second.binding).unwrap();
    assert!(!std::fs::read_to_string(format!("{repo}/.git/info/exclude")).unwrap().contains(GIT_EXCLUDE_LINE));
}

#[test]
fn tracked_local_settings_block() {
    let repo = temp_dir("repo");
    git_init(&repo);
    write(&local(&repo), json!({"model": "opus"}));
    assert!(git(&repo, &["add", "-f", ".claude/settings.local.json"]).is_some() && git(&repo, &["commit", "-qm", "add"]).is_some());
    assert!(binder().apply(&sample(), &repo, None, false).is_err());
}

#[test]
fn metadata_round_trip_rejects_newer_schema_and_reads_macos_files() {
    let dir = temp_dir("meta");
    let store = MetadataStore::new(Path::new(&dir).join("sub/state.json"), true);
    assert_eq!(store.load().unwrap(), AppState::default());
    let state = AppState { profiles: vec![sample()], bindings: vec![WorkspaceBinding::new("/x".into(), ALPHA.into())], ..AppState::default() };
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), state);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(store.path()).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(MetadataStore::decode(br#"{"schemaVersion":99,"profiles":[],"bindings":[]}"#), Err(Error::UnsupportedSchemaVersion(99)));
    assert!(MetadataStore::decode(b"nope").is_err());

    // Written by the macOS app (lowercase ID accepted, normalised).
    let swift = br#"{"bindings":[{"id":"11111111-2222-4333-8444-555555555555","managedValues":{},"path":"\/p","profileID":"6f1e2d3c-4b5a-4978-8695-a4b3c2d1e0f9"}],
        "profiles":[{"credential":{"account":"6F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9","service":"KeyZapper.LiteLLM"},"endpoint":"https:\/\/litellm.example.test","id":"6F1E2D3C-4B5A-4978-8695-A4B3C2D1E0F9","modelAlias":"","name":"Alpha"}],
        "schemaVersion":1}"#;
    let decoded = MetadataStore::decode(swift).unwrap();
    assert_eq!(decoded.bindings[0].profile_id, ALPHA);
    assert_eq!(decoded.bindings[0].path, "/p");
}

fn helper(dir: &str, budgets: BTreeMap<String, f64>) -> HelperCommand {
    HelperCommand {
        metadata: MetadataStore::new(Path::new(dir).join("state.json"), true),
        store: KeyStore { file: Path::new(dir).join("keys.json") },
        tokens: TokenStore { file: Path::new(dir).join("sso-tokens.json") },
        config: ManagedConfig::default(),
        remaining_budget: Box::new(move |_, key| budgets.get(key).copied()),
    }
}

fn args(command: &str, id: &str) -> Vec<String> {
    vec![command.into(), "--profile".into(), id.into()]
}

#[test]
fn helper_returns_only_requested_key_and_never_falls_back() {
    let dir = temp_dir("helper");
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sample(), other()], ..AppState::default() }).unwrap();
    assert_eq!(h.run(&args("store", ALPHA), || "sk-alpha\n".into()).exit_code, ExitCode::Ok);
    assert_eq!(h.run(&args("store", BETA), || "sk-beta".into()).exit_code, ExitCode::Ok);
    let out = h.run(&args("credential", &ALPHA.to_lowercase()), String::new);
    assert_eq!((out.exit_code, out.stdout.as_str()), (ExitCode::Ok, "sk-alpha"));
    assert_eq!(h.run(&args("credential", BETA), String::new).stdout, "sk-beta");
    assert_eq!(h.run(&args("store", ALPHA), || " \n".into()).exit_code, ExitCode::MissingCredential);
    assert_eq!(h.run(&args("status", ALPHA), String::new).exit_code, ExitCode::Ok);
    assert_eq!(h.run(&args("delete", ALPHA), String::new).exit_code, ExitCode::Ok);
    let missing = h.run(&args("credential", ALPHA), String::new);
    assert_eq!(missing.exit_code, ExitCode::MissingCredential);
    assert!(missing.stdout.is_empty() && !missing.stderr.contains("sk-"));
    assert_eq!(h.run(&args("credential", "6F1E2D3C-0000-4978-8695-A4B3C2D1E0F9"), String::new).exit_code, ExitCode::UnknownProfile);
    assert_eq!(h.run(&args("credential", "not-a-uuid"), String::new).exit_code, ExitCode::UnknownProfile);
    assert_eq!(h.run(&["credential".into()], String::new).exit_code, ExitCode::Usage);
    assert_eq!(h.run(&args("export", BETA), String::new).exit_code, ExitCode::Usage);
}

#[test]
fn helper_respects_gateway_allowlist() {
    let dir = temp_dir("helper");
    let mut h = helper(&dir, BTreeMap::new());
    h.metadata.save(&AppState { profiles: vec![sample()], ..AppState::default() }).unwrap();
    h.store.write(ALPHA, "sk-alpha").unwrap();
    h.config.allowed_gateway_hosts = vec!["litellm.firma.example".into()];
    assert_eq!(h.run(&args("credential", ALPHA), String::new).exit_code, ExitCode::ConfigError);
}

#[test]
fn pool_falls_back_to_key_with_most_budget_on_same_endpoint() {
    let dir = temp_dir("pool");
    let gamma = Profile::new("1A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9".into(), "Gamma".into(), "https://litellm.example.test".into(), String::new());
    let delta = Profile::new("2A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9".into(), "Delta".into(), "https://litellm.example.test/".into(), String::new());
    let left = |alpha: f64| BTreeMap::from([("sk-alpha".into(), alpha), ("sk-beta".into(), 100.0), ("sk-gamma".into(), 5.0), ("sk-delta".into(), 8.0)]);
    let setup = |h: &HelperCommand| {
        h.metadata.save(&AppState { profiles: vec![sample(), other(), gamma.clone(), delta.clone()], ..AppState::default() }).unwrap();
        for (id, key) in [(ALPHA, "sk-alpha"), (BETA, "sk-beta"), (gamma.id.as_str(), "sk-gamma"), (delta.id.as_str(), "sk-delta")] {
            h.store.write(id, key).unwrap();
        }
    };
    let h = helper(&dir, left(1.0));
    setup(&h);
    assert_eq!(h.run(&args("pool", ALPHA), String::new).stdout, "sk-alpha");
    let h = helper(&dir, left(0.0));
    assert_eq!(h.run(&args("pool", ALPHA), String::new).stdout, "sk-delta");
}

fn payload() -> BackupPayload {
    BackupPayload {
        created_at: "2026-09-21T14:13:20Z".into(),
        app_version: Some("1.0.3".into()),
        state: AppState { profiles: vec![sample()], bindings: vec![WorkspaceBinding::new("/p".into(), ALPHA.into())], ..AppState::default() },
        keys: BTreeMap::from([(ALPHA.into(), "sk-secret-alpha".into())]),
    }
}

#[test]
fn backup_round_trip_and_rejections() {
    let password = "correct horse battery";
    let payload = payload();
    let data = backup::seal(&payload, password, 100_000).unwrap();
    assert_eq!(backup::open(&data, password).unwrap(), payload);
    let text = String::from_utf8_lossy(&data);
    assert!(!text.contains("sk-secret") && !text.contains("Alpha"));
    assert_eq!(backup::open(&data, "wrong password!!"), Err(BackupError::WrongPasswordOrCorrupt));
    let tampered = text.replace("100000", "100001");
    assert_eq!(backup::open(tampered.as_bytes(), password), Err(BackupError::WrongPasswordOrCorrupt));
    let hostile = text.replace("100000", "999999999");
    assert_eq!(backup::open(hostile.as_bytes(), password), Err(BackupError::Corrupt));
    assert_eq!(backup::seal(&payload, "short", 100_000), Err(BackupError::PasswordTooShort));
    assert_eq!(backup::open(b"{}", password), Err(BackupError::NotABackup));
}

/// `tests/fixtures/swift-backup.kzbackup` was written by the macOS app (KeyZapper 1.0.7) with the password
/// "correct horse battery" and the payload above.
#[test]
fn opens_backups_made_by_the_macos_app() {
    let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/swift-backup.kzbackup")).unwrap();
    let opened = backup::open(&data, "correct horse battery").unwrap();
    assert_eq!(opened.keys, payload().keys);
    assert_eq!(opened.state.profiles[0].id, ALPHA);
    assert_eq!(opened.created_at, "2026-09-21T14:13:20Z");
}

fn sso() -> Profile {
    let mut p = Profile::new(BETA.into(), "SSO".into(), "https://gw.example.test".into(), String::new());
    p.auth_type = AuthType::Oidc;
    p.oidc_issuer = Some("http://127.0.0.1:1".into());
    p.oidc_client_id = Some("client".into());
    p
}

fn far_future() -> i64 {
    keyzapper_core::oidc::unix_now() + 3600
}

#[test]
fn profiles_without_auth_type_load_as_api_key_and_sso_round_trips() {
    let old: Profile = serde_json::from_value(json!({"id": ALPHA, "name": "A", "endpoint": "https://x.example"})).unwrap();
    assert_eq!(old.auth_type, AuthType::ApiKey);
    assert!(!old.is_sso());
    assert!(serde_json::to_value(&old).unwrap().get("oidcIssuer").is_none());
    let value = serde_json::to_value(sso()).unwrap();
    assert_eq!(value["authType"], "oidc");
    assert_eq!(value["oidcClientId"], "client");
    assert_eq!(serde_json::from_value::<Profile>(value).unwrap(), sso());
}

#[test]
fn token_store_round_trips_and_stays_out_of_keys_file() {
    let dir = temp_dir("tokens");
    let tokens = TokenStore { file: Path::new(&dir).join("sso-tokens.json") };
    assert!(!tokens.exists(BETA).unwrap());
    tokens.write(BETA, &TokenSet { refresh_token: "R".into(), access_token: Some("A".into()), expires_at: Some(5), token_type: Default::default() }).unwrap();
    assert!(tokens.exists(BETA).unwrap());
    assert_eq!(tokens.read(BETA).unwrap().unwrap().access_token.as_deref(), Some("A"));
    assert!(!Path::new(&dir).join("keys.json").exists());
    tokens.delete(BETA).unwrap();
    assert!(tokens.read(BETA).unwrap().is_none());
}

#[test]
fn sso_helper_exit_codes() {
    let dir = temp_dir("sso-helper");
    let mut state = AppState::default();
    state.profiles = vec![sample(), sso()];
    let h = helper(&dir, BTreeMap::new());
    h.metadata.save(&state).unwrap();
    // No session yet.
    assert_eq!(h.run(&args("status", BETA), String::new).exit_code, ExitCode::SessionExpired);
    assert_eq!(h.run(&args("credential", BETA), String::new).exit_code, ExitCode::SessionExpired);
    assert_eq!(ExitCode::SessionExpired as i32, 67);
    // Cached, still valid access token.
    h.tokens.write(BETA, &TokenSet { refresh_token: "R".into(), access_token: Some("jwt-1".into()), expires_at: Some(far_future()), token_type: Default::default() }).unwrap();
    let out = h.run(&args("credential", BETA), String::new);
    assert_eq!((out.exit_code, out.stdout.as_str()), (ExitCode::Ok, "jwt-1"));
    assert_eq!(h.run(&args("status", BETA), String::new).exit_code, ExitCode::Ok);
    // pool and store are refused.
    assert_eq!(h.run(&args("pool", BETA), String::new).exit_code, ExitCode::ConfigError);
    assert_eq!(h.run(&args("store", BETA), || "sk-x".into()).exit_code, ExitCode::Usage);
    assert!(!h.store.exists(BETA).unwrap());
    // Gateway allowlist still applies to the endpoint.
    let mut restricted = helper(&dir, BTreeMap::new());
    restricted.config.allowed_gateway_hosts = vec!["other.example".into()];
    assert_eq!(restricted.run(&args("credential", BETA), String::new).exit_code, ExitCode::ConfigError);
    // delete removes the tokens.
    assert_eq!(h.run(&args("delete", BETA), String::new).exit_code, ExitCode::Ok);
    assert_eq!(h.run(&args("status", BETA), String::new).exit_code, ExitCode::SessionExpired);
}

#[test]
fn pool_skips_sso_profiles() {
    let dir = temp_dir("sso-pool");
    let mut sso_profile = sso();
    sso_profile.endpoint = sample().endpoint;
    let mut state = AppState::default();
    state.profiles = vec![sample(), sso_profile];
    let h = helper(&dir, BTreeMap::from([("sk-own".to_string(), 0.0), ("sk-sso".to_string(), 50.0)]));
    h.metadata.save(&state).unwrap();
    h.store.write(ALPHA, "sk-own").unwrap();
    h.store.write(BETA, "sk-sso").unwrap(); // stray key on an SSO profile must not be pooled
    assert_eq!(h.run(&args("pool", ALPHA), String::new).stdout, "sk-own");
}

#[test]
fn sso_profiles_are_never_bound_with_pool() {
    let root = temp_dir("sso-proj");
    let result = binder().apply(&sso(), &root, None, true).unwrap();
    assert_eq!(result.binding.pooled, None);
    let helper_cmd = read(&local(&root))["apiKeyHelper"].as_str().unwrap().to_string();
    assert!(helper_cmd.ends_with(&format!("credential --profile {BETA}")));
    assert!(read(&local(&root))["env"].get("CLAUDE_CODE_API_KEY_HELPER_TTL_MS").is_none());
}

#[test]
fn backup_keeps_sso_metadata_but_never_tokens() {
    let dir = temp_dir("sso-backup");
    let tokens = TokenStore { file: Path::new(&dir).join("sso-tokens.json") };
    tokens.write(BETA, &TokenSet { refresh_token: "refresh-secret".into(), access_token: Some("access-secret".into()), expires_at: Some(1), token_type: Default::default() }).unwrap();
    let mut payload = payload();
    payload.state.profiles.push(sso());
    let plain = serde_json::to_string(&payload).unwrap();
    assert!(!plain.contains("refresh-secret") && !plain.contains("access-secret"));
    let opened = backup::open(&backup::seal(&payload, "correct horse battery", 100_000).unwrap(), "correct horse battery").unwrap();
    let restored = opened.state.profile(BETA).unwrap();
    assert_eq!((restored.auth_type, restored.oidc_issuer.as_deref(), restored.oidc_client_id.as_deref()), (AuthType::Oidc, Some("http://127.0.0.1:1"), Some("client")));
    assert!(!opened.keys.contains_key(BETA));
}

// MARK: Agent instruction files

#[test]
fn agents_scan_counts_languages_and_skips_dependencies() {
    use keyzapper_core::agents;
    let root = temp_dir("repo");
    for (file, text) in [
        ("src/main.rs", "fn main() {}"),
        ("src/lib.rs", ""),
        ("ui/app.ts", ""),
        ("Cargo.toml", "[package]\nname = \"demo\""),
        ("README.md", "# Demo\nA demo project."),
        ("CLAUDE.md", "Run cargo test."),
        ("node_modules/left-pad/index.js", ""),
        ("target/debug/build.rs", ""),
    ] {
        let path = Path::new(&root).join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let scan = agents::scan(&root);
    assert!(!scan.git);
    assert_eq!(scan.file_count, 6);
    assert_eq!(scan.languages, vec![("Rust".to_string(), 2), ("TypeScript".to_string(), 1)]);
    assert_eq!(scan.manifests, vec!["Cargo.toml"]);
    assert_eq!(scan.agent_files, vec!["CLAUDE.md"]);
    assert!(!scan.has_agents_file);

    let context = agents::context(&scan);
    assert!(context.contains("src/ (2 files)"));
    assert!(context.contains("=== Cargo.toml ===\n[package]"));
    assert!(context.contains("=== Existing CLAUDE.md ===\nRun cargo test."));
    assert!(!context.contains("node_modules"));
}

#[test]
fn agents_reply_is_parsed_into_proposal() {
    use keyzapper_core::agents;
    let reply = "<assessment>\nA Rust CLI.\n</assessment>\n<recommendations>\ntask | model | tier | reason\n|---|---|---|---|\n\
- Default | `team-sonnet` | Sonnet | Everyday work\n| Search | team-haiku | haiku | Cheap |\nnot a row\n</recommendations>\n\
<agents_md>\n```markdown\n# AGENTS.md\n\n- Run `cargo test`.\n```\n</agents_md>";
    let proposal = agents::parse_reply(reply).unwrap();
    assert_eq!(proposal.assessment, "A Rust CLI.");
    assert_eq!(proposal.content, "# AGENTS.md\n\n- Run `cargo test`.");
    let rows: Vec<(&str, &str, &str)> = proposal.recommendations.iter().map(|r| (r.task.as_str(), r.model.as_str(), r.tier.as_str())).collect();
    assert_eq!(rows, vec![("Default", "team-sonnet", "sonnet"), ("Search", "team-haiku", "haiku")]);
    assert_eq!(agents::parse_reply("<assessment>No file</assessment>"), None);
}

#[test]
fn agents_analysis_model_prefers_choice_then_profile_then_gateway() {
    use keyzapper_core::agents::analysis_model;
    let profile = sample();
    assert_eq!(analysis_model(" chosen ", &profile, &[]).as_deref(), Some("chosen"));
    assert_eq!(analysis_model("", &profile, &[]).as_deref(), Some("team-opus"));
    let bare = other();
    let available = vec!["claude-haiku-5-5".to_string(), "claude-sonnet-5-5".to_string()];
    assert_eq!(analysis_model("", &bare, &available).as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(analysis_model("", &bare, &[]), None);
}

#[test]
fn agents_prompt_lists_goals_and_models() {
    use keyzapper_core::agents::{prompt, Goal, Options};
    let options = Options { goals: vec![Goal::Cost, Goal::Security], language: "de".into(), model: String::new(), notes: "Use pnpm.".into() };
    let text = prompt("CONTEXT", &options, &sample(), &["team-opus".to_string(), "team-haiku".to_string()]);
    assert!(text.contains("<repository>\nCONTEXT\n</repository>"));
    assert!(text.contains("- Cost efficiency:") && text.contains("- Security:") && !text.contains("- Speed:"));
    assert!(text.contains("- opus: team-opus\n- default (ANTHROPIC_MODEL): claude-alpha"));
    assert!(text.contains("on the gateway: team-haiku\n"));
    assert!(text.contains("Use pnpm.") && text.contains("AGENTS.md in German"));
}

/// Answers one HTTP request with `status` and `body`; returns the endpoint and the raw request it received.
fn one_shot_server(status: &str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            request.extend_from_slice(&buffer[..n]);
            let text = String::from_utf8_lossy(&request);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        write!(stream, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (endpoint, handle)
}

#[test]
fn gateway_complete_posts_messages_and_reads_text() {
    use keyzapper_core::gateway::complete;
    use std::time::Duration;
    let (endpoint, server) = one_shot_server("200 OK",
        r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Hello"}],"stop_reason":"end_turn","usage":{"input_tokens":12,"output_tokens":3}}"#);
    let reply = complete(&format!("{endpoint}/"), "sk-test", "team-opus", "SYSTEM", "PROMPT", 100, Duration::from_secs(5)).unwrap();
    assert_eq!((reply.text.as_str(), reply.input_tokens, reply.output_tokens), ("Hello", 12, 3));
    let request = server.join().unwrap().to_lowercase();
    assert!(request.starts_with("post /v1/messages http/1.1"));
    assert!(request.contains("x-api-key: sk-test") && request.contains("authorization: bearer sk-test") && request.contains("anthropic-version: 2023-06-01"));
    assert!(request.contains(r#""model":"team-opus""#) && request.contains(r#""system":"system""#));

    let (endpoint, server) = one_shot_server("400 Bad Request", r#"{"error":{"message":"Invalid model name"}}"#);
    let error = complete(&endpoint, "sk-test", "nope", "S", "P", 100, Duration::from_secs(5)).err().unwrap();
    server.join().unwrap();
    assert!(error.contains("400") && error.contains("Invalid model name"), "{error}");
}

#[test]
fn whats_new_lists_releases_since_the_previous_version() {
    use keyzapper_core::whats_new::since;
    let versions = |previous: Option<&str>, current: &str| since(previous, current).into_iter().map(|r| r.version).collect::<Vec<_>>();
    assert_eq!(versions(Some("1.3.0"), "1.4.0"), vec!["1.4.0"]);
    assert_eq!(versions(Some("1.2.0"), "1.4.0"), vec!["1.4.0", "1.3.0"]);
    assert_eq!(versions(None, "1.4.0"), vec!["1.4.0", "1.3.0"]);
    assert_eq!(versions(Some("1.3.0"), "1.4.0-beta.1"), vec!["1.4.0"]);
    // Betas of one release share its notes: nothing new between them or from the last beta to the release.
    assert!(versions(Some("1.4.0-beta.1"), "1.4.0-beta.2").is_empty());
    assert!(versions(Some("1.4.0-beta.2"), "1.4.0").is_empty());
    assert!(versions(Some("1.4.0"), "1.4.1").is_empty());
    assert!(since(Some("1.3.0"), "1.4.0")[0].items.iter().all(|i| !i.is_empty()));
}

fn mcp_file(root: &str) -> String {
    format!("{root}/.mcp.json")
}

#[test]
fn mcp_servers_are_written_enabled_and_reverted_without_touching_foreign_entries() {
    use keyzapper_core::mcp;
    use keyzapper_core::models::{McpServer, McpTransport};
    let repo = temp_dir("repo");
    git_init(&repo);
    write(&mcp_file(&repo), json!({"mcpServers": {"team": {"type": "http", "url": "https://team.example.test/mcp"}}}));
    write(&local(&repo), json!({"enabledMcpjsonServers": ["team"]}));
    let mut github = McpServer::new(ALPHA.into(), "github".into());
    github.command = "npx".into();
    github.args = vec!["-y".into(), "server-github".into()];
    github.env = BTreeMap::from([("GITHUB_TOKEN".to_string(), "${GITHUB_TOKEN}".to_string())]);
    let mut docs = McpServer::new(BETA.into(), "docs".into());
    docs.transport = McpTransport::Http;
    docs.url = "https://docs.example.test/mcp".into();
    docs.headers = BTreeMap::from([("Authorization".to_string(), "Bearer ${DOCS_TOKEN}".to_string())]);
    docs.oauth_client_id = "weave-claude".into();
    docs.oauth_callback_port = Some(8976);
    docs.oauth_scopes = "mcp.read".into();

    let first = mcp::apply(&repo, &[github.clone(), docs.clone()], &BTreeMap::new(), None).unwrap();
    assert!(first.changed);
    assert_eq!(first.git_exclude_entry.as_deref(), Some(mcp::GIT_EXCLUDE_LINE));
    let file = read(&mcp_file(&repo));
    assert_eq!(file["mcpServers"]["github"], json!({"type": "stdio", "command": "npx", "args": ["-y", "server-github"], "env": {"GITHUB_TOKEN": "${GITHUB_TOKEN}"}}));
    assert_eq!(file["mcpServers"]["docs"], json!({"type": "http", "url": "https://docs.example.test/mcp", "headers": {"Authorization": "Bearer ${DOCS_TOKEN}"},
        "oauth": {"clientId": "weave-claude", "callbackPort": 8976, "scopes": "mcp.read"}}));
    assert_eq!(file["mcpServers"]["team"]["url"], "https://team.example.test/mcp");
    assert_eq!(read(&local(&repo))["enabledMcpjsonServers"], json!(["team", "docs", "github"]));
    assert!(git(&repo, &["check-ignore", "-q", ".mcp.json"]).is_some());
    assert!(!git(&repo, &["status", "--porcelain"]).unwrap().contains(".mcp.json"));
    assert!(mcp::is_active(&repo, &first.managed));

    let second = mcp::apply(&repo, &[github.clone(), docs.clone()], &first.managed, first.git_exclude_entry.as_deref()).unwrap();
    assert!(!second.changed);
    assert_eq!(second.managed, first.managed);

    // Deselecting a server removes only its entry.
    let third = mcp::apply(&repo, &[github.clone()], &second.managed, second.git_exclude_entry.as_deref()).unwrap();
    assert!(third.changed);
    let file = read(&mcp_file(&repo));
    assert!(file["mcpServers"].get("docs").is_none() && file["mcpServers"].get("team").is_some());
    assert_eq!(read(&local(&repo))["enabledMcpjsonServers"], json!(["team", "github"]));

    // A foreign definition under the same name blocks; revert keeps it and reports it.
    write(&mcp_file(&repo), json!({"mcpServers": {"github": {"type": "http", "url": "https://other.example.test"}}}));
    assert!(matches!(mcp::apply(&repo, &[github.clone()], &BTreeMap::new(), None), Err(Error::SettingsConflicts(_))));
    assert!(!mcp::is_active(&repo, &third.managed));
    let kept = mcp::revert(&repo, &third.managed, third.git_exclude_entry.as_deref()).unwrap();
    assert_eq!(kept, vec!["github".to_string()]);
    assert_eq!(read(&mcp_file(&repo))["mcpServers"]["github"]["url"], "https://other.example.test");
    assert_eq!(read(&local(&repo))["enabledMcpjsonServers"], json!(["team"]));
    assert!(!std::fs::read_to_string(format!("{repo}/.git/info/exclude")).unwrap().contains(mcp::GIT_EXCLUDE_LINE));

    // State round trip keeps servers and assignments.
    let mut binding = WorkspaceBinding::new(repo.clone(), ALPHA.into());
    binding.mcp_server_ids = vec![github.id.clone()];
    binding.mcp_managed = third.managed.clone();
    let state = AppState { profiles: vec![sample()], bindings: vec![binding], mcp_servers: vec![github, docs], ..AppState::default() };
    assert_eq!(MetadataStore::decode(&serde_json::to_vec(&state).unwrap()).unwrap(), state);
}

#[test]
fn mcp_files_created_for_servers_vanish_on_revert_and_tracked_mcp_json_blocks() {
    use keyzapper_core::mcp;
    use keyzapper_core::models::McpServer;
    let repo = temp_dir("repo");
    git_init(&repo);
    let mut server = McpServer::new(ALPHA.into(), "fs".into());
    server.command = "mcp-fs".into();
    let first = mcp::apply(&repo, &[server.clone()], &BTreeMap::new(), None).unwrap();
    assert!(Path::new(&mcp_file(&repo)).exists() && Path::new(&local(&repo)).exists());
    assert!(mcp::revert(&repo, &first.managed, first.git_exclude_entry.as_deref()).unwrap().is_empty());
    assert!(!Path::new(&mcp_file(&repo)).exists() && !Path::new(&local(&repo)).exists());
    assert!(Path::new(&repo).is_dir());

    write(&mcp_file(&repo), json!({"mcpServers": {}}));
    assert!(git(&repo, &["add", ".mcp.json"]).is_some() && git(&repo, &["commit", "-qm", "mcp"]).is_some());
    assert!(matches!(mcp::apply(&repo, &[server], &BTreeMap::new(), None), Err(Error::SettingsConflicts(_))));
}

// MARK: Claude for Microsoft 365

fn office_profile() -> Profile {
    use keyzapper_core::office::OfficeSettings;
    let mut p = sample();
    p.office = Some(OfficeSettings { enabled: true, allowed_users: vec!["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()], ..OfficeSettings::default() });
    p
}

/// Office settings travel in metadata and backups like any profile field; profiles without them are byte-for-byte
/// what KeyZapper wrote before, and an older app that drops the field gets it back from the mirror.
#[test]
fn office_settings_round_trip_and_stay_backward_compatible() {
    let dir = temp_dir("office-meta");
    let store = MetadataStore::new(Path::new(&dir).join("state.json"), true);
    let state = AppState { profiles: vec![office_profile(), other()], ..AppState::default() };
    store.save(&state).unwrap();
    let text = std::fs::read_to_string(Path::new(&dir).join("state.json")).unwrap();
    assert!(text.contains("\"office\"") && text.contains("\"allowedUsers\""));
    let loaded = store.load().unwrap();
    assert_eq!(loaded.profiles, state.profiles);
    assert!(loaded.profiles[0].is_office_enabled() && !loaded.profiles[1].is_office_enabled());
    assert!(serde_json::to_value(&loaded.profiles[1]).unwrap().get("office").is_none());

    // An older app rewrites state.json without `office`; the full mirror restores it on the next load.
    let mut downgraded: Value = serde_json::from_str(&text).unwrap();
    downgraded["profiles"][0].as_object_mut().unwrap().remove("office");
    std::fs::write(Path::new(&dir).join("state.json"), downgraded.to_string()).unwrap();
    assert_eq!(store.load().unwrap().profiles[0].office, office_profile().office);

    // The helper still answers with the key only; Office settings change nothing for Claude Code.
    let keys = KeyStore { file: Path::new(&dir).join("keys.json") };
    keys.write(ALPHA, "sk-alpha").unwrap();
    let h = HelperCommand::new(store, keys, ManagedConfig::default());
    let out = h.run(&args("credential", ALPHA), String::new);
    assert_eq!((out.exit_code, out.stdout.as_str()), (ExitCode::Ok, "sk-alpha"));

    let payload = BackupPayload { state: AppState { profiles: vec![office_profile()], ..AppState::default() }, ..payload() };
    let data = backup::seal(&payload, "correct horse battery", 100_000).unwrap();
    assert_eq!(backup::open(&data, "correct horse battery").unwrap(), payload);
    let legacy: BackupPayload = serde_json::from_value(json!({"createdAt": "2026-01-01T00:00:00Z", "state": {"schemaVersion": 1, "profiles": [{"id": ALPHA, "name": "A", "endpoint": "https://x.example"}]}})).unwrap();
    assert!(legacy.state.profiles[0].office.is_none() && legacy.state.office_deployment.is_none());
}

#[test]
fn litellm_virtual_key_provider_generates_short_lived_keys() {
    use keyzapper_core::office::{CredentialError, EntraIdentity, LiteLlmVirtualKeyProvider, OfficeCredentialProvider};
    let identity = EntraIdentity { oid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(), tenant_id: "t".into(), scopes: vec![] };
    let (endpoint, server) = one_shot_server("200 OK", r#"{"key":"sk-virtual-123","expires":"2026-10-16T12:00:00Z"}"#);
    let mut profile = office_profile();
    profile.endpoint = format!("{endpoint}/");
    let provider = LiteLlmVirtualKeyProvider::new("sk-management".into());
    let before = keyzapper_core::oidc::unix_now();
    let credential = provider.credential_for(&profile, &identity, 300).unwrap();
    assert_eq!(credential.token, "sk-virtual-123");
    assert!(credential.expires_at.unwrap() >= before + 300);
    let request = server.join().unwrap();
    let lower = request.to_lowercase();
    assert!(lower.starts_with("post /key/generate http/1.1"), "{request}");
    assert!(lower.contains("authorization: bearer sk-management"));
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["duration"], "300s");
    assert_eq!(body["models"], json!(["claude-alpha", "team-opus"]));
    assert_eq!(body["metadata"]["keyzapper_profile"], ALPHA);
    assert_eq!(body["metadata"]["keyzapper_source"], "office");
    assert!(!body.to_string().contains("eeeeeeeeeeee"), "oid must not reach the gateway in clear text");
    assert!(!format!("{credential:?}").contains("sk-virtual"));

    let (endpoint, server) = one_shot_server("401 Unauthorized", r#"{"error":"bad key"}"#);
    profile.endpoint = endpoint;
    let error = provider.credential_for(&profile, &identity, 300).err().unwrap();
    server.join().unwrap();
    assert!(matches!(error, CredentialError::NotConfigured(_)), "{error}");
    assert!(!error.to_string().contains("sk-"));

    let (endpoint, server) = one_shot_server("500 Internal Server Error", "{}");
    profile.endpoint = endpoint;
    let error = provider.credential_for(&profile, &identity, 300).err().unwrap();
    server.join().unwrap();
    assert!(matches!(error, CredentialError::Unavailable(_)), "{error}");
}
