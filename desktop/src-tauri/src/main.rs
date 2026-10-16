// No console window next to the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;

use keyzapper_core::errors::Error;
use keyzapper_core::gateway::CheckResult;
use keyzapper_core::{agents, backup, clipboard, gateway, l, legacy_keychain, oidc, update};
use model::{McpServerInput, Model, OfficeDeploymentInput, ProfileInput, View};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

struct Shared(Mutex<Model>);

fn with_model<T>(app: &AppHandle, f: impl FnOnce(&mut Model) -> T) -> T {
    let shared = app.state::<Shared>();
    let mut model = shared.0.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut model)
}

fn view(app: &AppHandle) -> View {
    with_model(app, |m| m.view())
}

/// Tells the window to fetch a new view.
fn changed(app: &AppHandle) {
    let _ = app.emit("changed", ());
}

/// Runs blocking work (files, git, network, dialogs) off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tauri::async_runtime::spawn_blocking(f).await.expect("background task panicked")
}

// MARK: Background refresh

/// Recomputes project status and the settings check off the lock; the newest run wins.
fn refresh(app: &AppHandle) {
    let Some((generation, state, binder)) = with_model(app, |m| {
        m.refresh_generation += 1;
        Some((m.refresh_generation, m.state.clone(), m.binder.clone()?))
    }) else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let (statuses, mcp_active, global_health, findings) = model::status_snapshot(&state, &binder);
        with_model(&app, |m| {
            if m.refresh_generation == generation {
                m.statuses = statuses;
                m.mcp_active = mcp_active;
                m.global_health = global_health;
                m.global_findings = findings;
            }
        });
        changed(&app);
    });
}

/// Loads the budget of every stored key from the gateway (LiteLLM `/key/info`).
fn refresh_budgets(app: &AppHandle, only: Option<&str>) {
    let profiles = with_model(app, |m| m.keyed_profiles());
    for (id, endpoint, key) in profiles.into_iter().filter(|p| only.is_none_or(|o| o == p.0)) {
        let app = app.clone();
        std::thread::spawn(move || {
            let budget = gateway::budget(&endpoint, &key, Duration::from_secs(10)).ok();
            with_model(&app, |m| match budget {
                Some(b) => {
                    m.budgets.insert(id, b);
                }
                None => {
                    m.budgets.remove(&id);
                }
            });
            changed(&app);
        });
    }
}

/// Stable releases only, unless `beta` was asked for or a beta is installed (testers get the next beta too).
fn check_updates(app: &AppHandle, user_initiated: bool, beta: bool) {
    let (enabled, version) = with_model(app, |m| (m.config.update_check_enabled, m.app_version.clone()));
    let notice = |text: String| {
        if user_initiated {
            with_model(app, |m| m.notice = Some(text));
        }
    };
    if !enabled {
        return notice(l!("Updates werden von der IT verteilt; die Update-Prüfung ist abgeschaltet."));
    }
    let Some(version) = version else { return notice(l!("Entwicklungsversion – keine Update-Prüfung.")) };
    match update::latest_release(beta || update::is_prerelease(&version)) {
        Ok(release) if update::is_newer(&release.version, &version) => with_model(app, |m| m.available_update = Some(release)),
        Ok(_) => notice(l!("KeyZapper %@ ist aktuell.", version)),
        Err(e) => {
            if user_initiated {
                with_model(app, |m| m.error = Some(l!("Update-Prüfung fehlgeschlagen: %@", e)));
            }
        }
    }
    changed(app);
}

/// Start-up work that may take a while: status, budgets, updates, CLI versions, keys left in the keychain.
fn start_background(app: &AppHandle) {
    refresh(app);
    refresh_budgets(app, None);
    let handle = app.clone();
    std::thread::spawn(move || check_updates(&handle, false, false));
    let handle = app.clone();
    std::thread::spawn(move || {
        let minimum = with_model(&handle, |m| m.minimum_cli_version());
        let outdated: Vec<String> = keyzapper_core::cli::outdated_installations(&minimum)
            .into_iter()
            .map(|(path, version)| format!("{} {version}", keyzapper_core::paths::abbreviate_home(&path)))
            .collect();
        let candidates = with_model(&handle, |m| {
            m.outdated_clis = outdated;
            m.profiles_without_key()
        });
        let in_keychain: Vec<String> = candidates.into_iter().filter(|id| legacy_keychain::has_key(id)).collect();
        with_model(&handle, |m| m.keychain_profiles = in_keychain);
        changed(&handle);
    });
}

// MARK: Commands

#[tauri::command]
fn translations() -> HashMap<String, String> {
    keyzapper_core::i18n::english().clone()
}

#[tauri::command]
fn get_view(app: AppHandle) -> View {
    view(&app)
}

#[tauri::command]
async fn refresh_all(app: AppHandle) -> View {
    blocking(move || {
        refresh(&app);
        refresh_budgets(&app, None);
        view(&app)
    })
    .await
}

#[tauri::command]
fn dismiss_notice(app: AppHandle) -> View {
    with_model(&app, |m| m.notice = None);
    view(&app)
}

#[tauri::command]
fn dismiss_error(app: AppHandle) -> View {
    with_model(&app, |m| m.error = None);
    view(&app)
}

/// Runs a model action off the main thread, then refreshes the status.
async fn act(app: AppHandle, action: impl FnOnce(&mut Model) + Send + 'static) -> View {
    blocking(move || {
        with_model(&app, action);
        refresh(&app);
        view(&app)
    })
    .await
}

#[tauri::command]
async fn save_profile(app: AppHandle, input: ProfileInput) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.save_profile_input(input));
        refresh(&app);
        refresh_budgets(&app, None);
        (ok, view(&app))
    })
    .await
}

#[tauri::command]
async fn store_key(app: AppHandle, profile_id: String, key: String) -> View {
    blocking(move || {
        with_model(&app, |m| m.store_key(&profile_id, &key));
        refresh(&app);
        refresh_budgets(&app, Some(&profile_id));
        view(&app)
    })
    .await
}

#[tauri::command]
async fn delete_profile(app: AppHandle, profile_id: String) -> View {
    act(app, move |m| m.delete_profile(&profile_id)).await
}

/// Copies the key marked as confidential and clears it after 60 s if unchanged.
#[tauri::command]
async fn copy_key(app: AppHandle, profile_id: String) -> View {
    blocking(move || {
        if let Some((key, name)) = with_model(&app, |m| m.key_for_copy(&profile_id)) {
            match clipboard::copy_concealed(&key) {
                Some(marker) => {
                    with_model(&app, |m| m.notice = Some(l!("Key von „%@“ kopiert. Er wird nach 60 Sekunden aus der Zwischenablage entfernt.", name)));
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(60));
                        clipboard::clear_if_unchanged(marker);
                    });
                }
                None => with_model(&app, |m| m.error = Some(l!("Die Zwischenablage ist nicht verfügbar."))),
            }
        }
        view(&app)
    })
    .await
}

#[derive(serde::Serialize)]
struct CheckOutcome {
    success: bool,
    message: String,
}

#[tauri::command]
async fn test_connection(app: AppHandle, profile_id: String) -> Result<CheckOutcome, String> {
    blocking(move || {
        let (endpoint, models, key, sso) = with_model(&app, |m| {
            let profile = m.state.profile(&profile_id).cloned().ok_or_else(|| l!("Unbekanntes Profil: %@", profile_id))?;
            let key = if profile.is_sso() { m.sso_token(&profile) } else { m.keys.read(&profile_id).map_err(|e| e.to_string()) };
            Ok::<_, String>((profile.endpoint.clone(), profile.configured_models(), key, profile.is_sso()))
        })?;
        let key = match key {
            Ok(key) => key,
            Err(message) if sso => return Ok(CheckOutcome { success: false, message }),
            Err(message) => return Err(message),
        };
        let result = gateway::run(&endpoint, &key, &models);
        if sso && result == CheckResult::Unauthorized {
            with_model(&app, |m| m.sso_expired.insert(profile_id.clone()));
            return Ok(CheckOutcome { success: false, message: l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden.") });
        }
        refresh_budgets(&app, Some(&profile_id));
        Ok(CheckOutcome { success: result.is_success(), message: result.message() })
    })
    .await
}

/// Lists the gateway's model names for a key: the one typed in the editor, else the stored key of the profile.
#[tauri::command]
async fn available_models(app: AppHandle, endpoint: String, typed_key: String, profile_id: Option<String>) -> Result<Vec<String>, String> {
    blocking(move || {
        let endpoint = model::valid_endpoint(&endpoint).ok_or_else(|| l!("Bitte eine vollständige https-URL angeben (http nur für localhost)."))?;
        let sso = with_model(&app, |m| profile_id.as_deref().and_then(|id| m.state.profile(id)).is_some_and(|p| p.is_sso()));
        let key = with_model(&app, |m| {
            if !m.config.is_endpoint_allowed(&endpoint) {
                return Err(l!(
                    "Der Endpunkt %@ ist laut Firmenrichtlinie nicht freigegeben. Erlaubt: %@",
                    keyzapper_core::managed::host_of(&endpoint).unwrap_or_default(),
                    m.config.allowed_gateway_hosts.join(", ")
                ));
            }
            match (typed_key.trim(), profile_id) {
                ("", Some(id)) if m.state.profile(&id).is_some_and(|p| p.is_sso()) => {
                    let profile = m.state.profile(&id).cloned().unwrap();
                    m.sso_token(&profile).map_err(|_| l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden."))
                }
                ("", Some(id)) => m.keys.read(&id).map_err(|e| e.to_string()),
                ("", None) => Err(l!("Für den Modellabruf wird ein Key benötigt.")),
                (typed, _) => Ok(typed.to_string()),
            }
        })?;
        match gateway::models(&endpoint, &key) {
            Ok(models) if models.is_empty() => Err(l!("Das Gateway hat keine Modelle für diesen Key gemeldet.")),
            Ok(models) => Ok(models),
            Err(CheckResult::Unauthorized) if sso => Err(l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden.")),
            Err(failure) => Err(failure.message()),
        }
    })
    .await
}

/// Browser sign-in (OIDC authorization code + PKCE); blocks up to `LOGIN_TIMEOUT` on a background thread.
#[tauri::command]
async fn sso_login(app: AppHandle, profile_id: String) -> View {
    blocking(move || {
        let Some((profile, tokens)) = with_model(&app, |m| {
            let profile = m.state.profile(&profile_id).filter(|p| p.is_sso()).cloned()?;
            m.sso_logging_in.insert(profile_id.clone()).then(|| (profile, m.tokens.clone()))
        }) else {
            return view(&app);
        };
        changed(&app);
        let opener = app.clone();
        let result = oidc::login(
            &profile,
            &tokens,
            |url| opener.opener().open_url(url, None::<&str>).map_err(|e| Error::Oidc(e.to_string())),
            oidc::LOGIN_TIMEOUT,
        );
        with_model(&app, |m| {
            m.sso_logging_in.remove(&profile_id);
            match result {
                Ok(()) => {
                    m.sso_expired.remove(&profile_id);
                    m.notice = Some(l!("Angemeldet bei „%@“.", profile.name));
                }
                Err(e) => m.error = Some(e.to_string()),
            }
        });
        refresh(&app);
        view(&app)
    })
    .await
}

#[tauri::command]
async fn sso_logout(app: AppHandle, profile_id: String) -> View {
    act(app, move |m| {
        m.sso_logout(&profile_id);
        m.notice = Some(l!("Abgemeldet."));
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SuggestedModels {
    opus: Option<String>,
    sonnet: Option<String>,
    haiku: Option<String>,
}

#[tauri::command]
fn suggest_models(models: Vec<String>) -> SuggestedModels {
    let (opus, sonnet, haiku) = keyzapper_core::models::suggest_models(&models);
    SuggestedModels { opus, sonnet, haiku }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PickedFolder {
    folder: String,
    root: String,
    existing_profile_id: Option<String>,
}

#[tauri::command]
async fn pick_project_folder(app: AppHandle) -> Option<PickedFolder> {
    blocking(move || {
        let folder = app.dialog().file().set_title(l!("Projektordner")).blocking_pick_folder()?.into_path().ok()?;
        let folder = folder.to_string_lossy().into_owned();
        let root = keyzapper_core::settings::settings_root(&folder);
        let existing_profile_id = with_model(&app, |m| m.state.bindings.iter().find(|b| b.path == root).map(|b| b.profile_id.clone()));
        Some(PickedFolder { folder: keyzapper_core::paths::canonical_path(&folder), root, existing_profile_id })
    })
    .await
}

#[tauri::command]
async fn bind(app: AppHandle, folder: String, profile_id: String) -> View {
    act(app, move |m| m.bind(&folder, &profile_id)).await
}

#[tauri::command]
async fn reapply(app: AppHandle, binding_id: String) -> View {
    act(app, move |m| m.reapply(&binding_id)).await
}

#[tauri::command]
async fn toggle_pool(app: AppHandle, binding_id: String) -> View {
    act(app, move |m| m.toggle_pool(&binding_id)).await
}

#[tauri::command]
async fn unbind(app: AppHandle, binding_id: String) -> View {
    act(app, move |m| {
        m.unbind(&binding_id);
    })
    .await
}

// MARK: Agent instruction files

#[tauri::command]
async fn scan_repository(app: AppHandle, binding_id: String) -> Result<agents::RepoScan, String> {
    blocking(move || {
        let path = with_model(&app, |m| m.state.bindings.iter().find(|b| b.id == binding_id).map(|b| b.path.clone()))
            .ok_or_else(|| l!("Unbekannte Zuordnung."))?;
        if !std::path::Path::new(&path).is_dir() {
            return Err(l!("Ordner nicht gefunden: %@", keyzapper_core::paths::abbreviate_home(&path)));
        }
        Ok(agents::scan(&path))
    })
    .await
}

/// Asks the profile's gateway model for an assessment, model advice and an `AGENTS.md`; nothing is written yet.
#[tauri::command]
async fn generate_agents_file(app: AppHandle, binding_id: String, options: agents::Options) -> Result<agents::Proposal, String> {
    blocking(move || {
        let (profile, path, key) = with_model(&app, |m| m.analysis_access(&binding_id))?;
        let available = match gateway::models(&profile.endpoint, &key) {
            Ok(models) => models,
            Err(CheckResult::Unauthorized) if profile.is_sso() => {
                with_model(&app, |m| m.sso_expired.insert(profile.id.clone()));
                return Err(l!("Kein aktiver Zugang – Sitzung abgelaufen, bitte neu anmelden."));
            }
            Err(failure @ (CheckResult::Unauthorized | CheckResult::BudgetExceeded | CheckResult::Unreachable(_))) => return Err(failure.message()),
            // Some gateways do not list models; the analysis call can still work.
            Err(_) => vec![],
        };
        let model = agents::analysis_model(&options.model, &profile, &available)
            .ok_or_else(|| l!("Kein Modell für die Analyse gefunden. Bitte im Profil Modelle eintragen."))?;
        let prompt = agents::prompt(&agents::context(&agents::scan(&path)), &options, &profile, &available);
        let reply = gateway::complete(&profile.endpoint, &key, &model, agents::SYSTEM_PROMPT, &prompt, 16_000, Duration::from_secs(300));
        refresh_budgets(&app, Some(&profile.id));
        let reply = reply?;
        let proposal = agents::parse_reply(&reply.text).ok_or_else(|| l!("Die Antwort des Modells enthielt keine AGENTS.md. Bitte erneut versuchen."))?;
        Ok(agents::Proposal { model, input_tokens: reply.input_tokens, output_tokens: reply.output_tokens, ..proposal })
    })
    .await
}

#[tauri::command]
async fn write_agents_file(app: AppHandle, binding_id: String, content: String) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.write_agents_file(&binding_id, &content));
        (ok, view(&app))
    })
    .await
}

// MARK: MCP servers

#[tauri::command]
async fn save_mcp_server(app: AppHandle, input: McpServerInput) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.save_mcp_server(input));
        refresh(&app);
        (ok, view(&app))
    })
    .await
}

#[tauri::command]
async fn delete_mcp_server(app: AppHandle, server_id: String) -> View {
    act(app, move |m| m.delete_mcp_server(&server_id)).await
}

#[tauri::command]
async fn set_project_mcp_servers(app: AppHandle, binding_id: String, server_ids: Vec<String>) -> View {
    act(app, move |m| {
        m.set_project_mcp_servers(&binding_id, server_ids);
    })
    .await
}

#[tauri::command]
async fn set_global_profile(app: AppHandle, profile_id: Option<String>) -> View {
    act(app, move |m| m.set_global_profile(profile_id)).await
}

#[tauri::command]
async fn repair_user_settings(app: AppHandle) -> View {
    act(app, |m| m.repair_user_settings()).await
}

#[tauri::command]
async fn set_disabled(app: AppHandle, disabled: bool) -> View {
    act(app, move |m| m.set_disabled(disabled)).await
}

#[tauri::command]
async fn restore_from_backup(app: AppHandle) -> View {
    act(app, |m| m.restore_from_backup()).await
}

#[tauri::command]
async fn discard_backup(app: AppHandle) -> View {
    act(app, |m| m.discard_backup()).await
}

#[tauri::command]
async fn migrate_keychain(app: AppHandle) -> View {
    blocking(move || {
        with_model(&app, |m| m.migrate_keychain());
        refresh(&app);
        refresh_budgets(&app, None);
        view(&app)
    })
    .await
}

/// Asks where to save, then writes the password-encrypted `.kzbackup` file. False if cancelled or failed.
#[tauri::command]
async fn export_backup(app: AppHandle, password: String) -> bool {
    blocking(move || {
        let name = format!("KeyZapper-Backup-{}.{}", keyzapper_core::time::today(), backup::FILE_EXTENSION);
        let Some(target) = app
            .dialog()
            .file()
            .add_filter("KeyZapper Backup", &[backup::FILE_EXTENSION])
            .set_file_name(&name)
            .blocking_save_file()
            .and_then(|p| p.into_path().ok())
        else {
            return false;
        };
        let payload = with_model(&app, |m| m.backup_payload());
        let result = backup::seal(&payload, &password, backup::DEFAULT_ITERATIONS)
            .map_err(|e| e.to_string())
            .and_then(|data| keyzapper_core::paths::write_atomic(&target, &data, true).map_err(|e| e.to_string()));
        with_model(&app, |m| match result {
            Ok(()) => {
                m.notice = Some(l!(
                    "Verschlüsseltes Backup gespeichert: %@ Profil(e), %@ Key(s), %@ Projekt(e).",
                    payload.state.profiles.len(),
                    payload.keys.len(),
                    payload.state.bindings.len()
                ));
                true
            }
            Err(e) => {
                m.error = Some(e);
                false
            }
        })
    })
    .await
}

#[tauri::command]
async fn save_office_deployment(app: AppHandle, input: OfficeDeploymentInput) -> (bool, View) {
    blocking(move || {
        let ok = with_model(&app, |m| m.save_office_deployment(input));
        (ok, view(&app))
    })
    .await
}

/// Writes the deployment file for `keyzapper-office-bootstrap`: profiles released for Office, their remote MCP
/// servers and user assignments. Never keys or tokens.
#[tauri::command]
async fn export_office_config(app: AppHandle) -> bool {
    blocking(move || {
        let json = match with_model(&app, |m| m.office_deployment_json()) {
            Ok(json) => json,
            Err(e) => {
                with_model(&app, |m| m.error = Some(e));
                return false;
            }
        };
        let name = format!("keyzapper-office-deployment-{}.json", keyzapper_core::time::today());
        let Some(target) = app.dialog().file().add_filter("JSON", &["json"]).set_file_name(&name).blocking_save_file().and_then(|p| p.into_path().ok()) else {
            return false;
        };
        let result = keyzapper_core::paths::write_atomic(&target, json.as_bytes(), true).map_err(|e| e.to_string());
        with_model(&app, |m| match result {
            Ok(()) => {
                m.notice = Some(l!("Office-Bootstrap-Konfiguration gespeichert (ohne Keys): %@", target.display()));
                true
            }
            Err(e) => {
                m.error = Some(e);
                false
            }
        })
    })
    .await
}

#[tauri::command]
async fn pick_backup_file(app: AppHandle) -> Option<String> {
    blocking(move || {
        app.dialog()
            .file()
            .add_filter("KeyZapper Backup", &[backup::FILE_EXTENSION])
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn import_backup(app: AppHandle, path: String, password: String) -> bool {
    blocking(move || {
        let opened = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|data| backup::open(&data, &password).map_err(|e| e.to_string()));
        let ok = with_model(&app, |m| match opened {
            Ok(payload) => m.import_payload(payload),
            Err(e) => {
                m.error = Some(e);
                false
            }
        });
        refresh(&app);
        refresh_budgets(&app, None);
        ok
    })
    .await
}

#[tauri::command]
async fn check_for_updates(app: AppHandle, beta: bool) -> View {
    blocking(move || {
        check_updates(&app, true, beta);
        view(&app)
    })
    .await
}

/// Downloads and verifies the installer and installs it; KeyZapper then starts again as the new version.
/// If installing in place fails, the package is opened in the system installer instead.
#[tauri::command]
async fn install_update(app: AppHandle) -> View {
    blocking(move || {
        let Some(release) = with_model(&app, |m| {
            if m.installing_update {
                return None;
            }
            m.installing_update = true;
            m.available_update.clone()
        }) else {
            return view(&app);
        };
        changed(&app);
        let path = match update::download_package(&release) {
            Ok(path) => path,
            Err(e) => {
                with_model(&app, |m| {
                    m.installing_update = false;
                    m.error = Some(l!("Update konnte nicht geladen werden: %@", e));
                });
                return view(&app);
            }
        };
        match update::install(&path, &release.version) {
            // Windows: the installer runs on and starts the new version itself; the app must not hold its files.
            Ok(()) if cfg!(windows) => app.exit(0),
            Ok(()) => app.restart(),
            Err(update::UpdateError::Cancelled) => with_model(&app, |m| m.notice = Some(update::UpdateError::Cancelled.to_string())),
            Err(e) => {
                let opened = app.opener().open_path(path.to_string_lossy(), None::<&str>);
                with_model(&app, |m| match opened {
                    Ok(()) => {
                        m.notice = Some(l!("Installer für KeyZapper %@ geöffnet. Nach der Installation KeyZapper neu starten.", release.version));
                        m.available_update = None;
                    }
                    Err(_) => m.error = Some(e.to_string()),
                });
            }
        }
        with_model(&app, |m| m.installing_update = false);
        view(&app)
    })
    .await
}

#[tauri::command]
fn dismiss_whats_new(app: AppHandle) -> View {
    with_model(&app, |m| m.dismiss_whats_new());
    view(&app)
}

#[tauri::command]
fn reveal(app: AppHandle, path: String) {
    let _ = app.opener().reveal_item_in_dir(path);
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) {
    if url.starts_with("https://") {
        let _ = app.opener().open_url(url, None::<&str>);
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let version = (!cfg!(debug_assertions)).then(|| app.package_info().version.to_string());
            app.manage(Shared(Mutex::new(Model::load(version))));
            start_background(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            translations,
            get_view,
            refresh_all,
            dismiss_notice,
            dismiss_error,
            save_profile,
            store_key,
            delete_profile,
            copy_key,
            sso_login,
            sso_logout,
            test_connection,
            available_models,
            suggest_models,
            pick_project_folder,
            bind,
            reapply,
            toggle_pool,
            unbind,
            scan_repository,
            generate_agents_file,
            write_agents_file,
            save_mcp_server,
            delete_mcp_server,
            set_project_mcp_servers,
            set_global_profile,
            repair_user_settings,
            set_disabled,
            restore_from_backup,
            discard_backup,
            migrate_keychain,
            export_backup,
            save_office_deployment,
            export_office_config,
            pick_backup_file,
            import_backup,
            check_for_updates,
            install_update,
            dismiss_whats_new,
            reveal,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running KeyZapper");
}
