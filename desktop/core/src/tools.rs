//! Installs and detects the Claude tools KeyZapper can set up: Claude Code (CLI), its VS Code extension and IntelliJ
//! plugin, and Claude Desktop. Everything comes from Anthropic's official sources: the native installer
//! (`claude.ai/install.sh`, `install.ps1`), the VS Code Marketplace, the JetBrains Marketplace and the Claude Desktop
//! download (`.dmg` on macOS, `.msix` on Windows). Nothing needs administrator rights.

use crate::{cli, gateway, git, l, paths, update};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub const VSCODE_EXTENSION: &str = "anthropic.claude-code";
pub const JETBRAINS_PLUGIN: &str = "com.anthropic.code.plugin";
/// Folder the JetBrains plugin is installed in (`<config>/plugins/claude-code-jetbrains-plugin`).
const JETBRAINS_PLUGIN_DIR: &str = "claude-code-jetbrains-plugin";
/// Developer ID team of Anthropic PBC; the downloaded Claude.app must be signed by it.
#[cfg(target_os = "macos")]
const ANTHROPIC_TEAM_ID: &str = "Q6L2SF6YDW";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    Cli,
    VsCode,
    Intellij,
    Desktop,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub tool: Tool,
    /// Installed version (empty if unknown); None if not installed.
    pub version: Option<String>,
    /// False if the IDE the extension or plugin belongs to was not found.
    pub host_found: bool,
    /// Where the CLI is installed, for the IDE's "Claude command" setting.
    pub path: Option<String>,
}

/// State of all tools; runs `--version` and (Windows) PowerShell, so call it off the main thread.
pub fn status() -> Vec<ToolStatus> {
    let cli = cli::installed();
    vec![
        ToolStatus { tool: Tool::Cli, version: cli.as_ref().map(|(_, v)| v.clone()), host_found: true, path: cli.map(|(p, _)| p) },
        ToolStatus { tool: Tool::VsCode, version: vscode_extension_version(), host_found: vscode_cli().is_some(), path: None },
        ToolStatus { tool: Tool::Intellij, version: jetbrains_plugin_version(), host_found: !intellij_launchers().is_empty(), path: None },
        ToolStatus { tool: Tool::Desktop, version: desktop_version(), host_found: true, path: None },
    ]
}

pub fn install(tool: Tool) -> Result<(), String> {
    match tool {
        Tool::Cli => install_cli(),
        Tool::VsCode => {
            let code = vscode_cli().ok_or_else(|| l!("Visual Studio Code wurde nicht gefunden."))?;
            run(&code.to_string_lossy(), &["--install-extension", VSCODE_EXTENSION], &[]).map(|_| ())
        }
        Tool::Intellij => install_jetbrains_plugin(),
        Tool::Desktop => install_desktop(),
    }
}

/// Runs a program without console window; on failure the last lines of its output are the error.
fn run(program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let output = git::command(program)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| l!("%@ konnte nicht gestartet werden: %@", program, e))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stderr.trim().is_empty() { stdout.as_str() } else { &stderr };
    let tail: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = tail[tail.len().saturating_sub(3)..].join(" ");
    Err(if tail.is_empty() { l!("Fehlgeschlagen (Exit-Code %@).", output.status.code().unwrap_or(-1)) } else { tail })
}

// MARK: Claude Code CLI

/// Anthropic's native installer: `~/.local/bin/claude`, updates itself.
fn install_cli() -> Result<(), String> {
    if cfg!(windows) {
        run("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "irm https://claude.ai/install.ps1 | iex"], &[])
    } else {
        run("/bin/bash", &["-c", "set -o pipefail; curl -fsSL https://claude.ai/install.sh | bash"], &[])
    }
    .map(|_| ())?;
    cli::installed().map(|_| ()).ok_or_else(|| l!("Der Installer ist durchgelaufen, aber claude wurde nicht gefunden."))
}

// MARK: VS Code

fn vscode_cli() -> Option<PathBuf> {
    let home = paths::home_dir();
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        let mut list = vec![];
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            list.push(PathBuf::from(local).join(r"Programs\Microsoft VS Code\bin\code.cmd"));
        }
        if let Some(programs) = std::env::var_os("ProgramFiles") {
            list.push(PathBuf::from(programs).join(r"Microsoft VS Code\bin\code.cmd"));
        }
        list
    } else {
        let bundle = "Visual Studio Code.app/Contents/Resources/app/bin/code";
        vec![Path::new("/Applications").join(bundle), home.join("Applications").join(bundle), "/usr/local/bin/code".into(), "/opt/homebrew/bin/code".into()]
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// Newest `~/.vscode/extensions/anthropic.claude-code-<version>[-<platform>]` folder.
fn vscode_extension_version() -> Option<String> {
    let prefix = format!("{VSCODE_EXTENSION}-");
    newest(entries(&paths::home_dir().join(".vscode").join("extensions")).filter_map(|name| {
        Some(name.strip_prefix(&prefix)?.split('-').next()?.to_string())
    }))
}

// MARK: IntelliJ IDEA

/// Launchers of the installed IntelliJ IDEA editions (also those from the Toolbox app).
fn intellij_launchers() -> Vec<PathBuf> {
    let home = paths::home_dir();
    if cfg!(windows) {
        let bases = [std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("JetBrains")), std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs"))];
        bases.into_iter().flatten().flat_map(|base| {
            entries(&base).filter(|n| n.starts_with("IntelliJ IDEA")).map(move |n| base.join(n).join(r"bin\idea64.exe")).collect::<Vec<_>>()
        }).filter(|p| p.is_file()).collect()
    } else {
        [PathBuf::from("/Applications"), home.join("Applications")].into_iter().flat_map(|base| {
            entries(&base).filter(|n| n.starts_with("IntelliJ IDEA") && n.ends_with(".app")).map(move |n| base.join(n).join("Contents/MacOS/idea")).collect::<Vec<_>>()
        }).filter(|p| p.is_file()).collect()
    }
}

/// IntelliJ's configuration folders (`IntelliJIdea2025.2`, `IdeaIC2024.3`, …), each with its `plugins` folder.
fn intellij_config_dirs() -> Vec<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| paths::home_dir().join(r"AppData\Roaming")).join("JetBrains")
    } else {
        paths::home_dir().join("Library/Application Support/JetBrains")
    };
    entries(&base).filter(|n| n.starts_with("IntelliJIdea") || n.starts_with("IdeaIC")).map(|n| base.join(n)).collect()
}

/// Newest `plugins/claude-code-jetbrains-plugin/lib/claude-code-jetbrains-plugin-<version>.jar`.
fn jetbrains_plugin_version() -> Option<String> {
    let prefix = format!("{JETBRAINS_PLUGIN_DIR}-");
    newest(intellij_config_dirs().into_iter().flat_map(|dir| {
        let lib = dir.join("plugins").join(JETBRAINS_PLUGIN_DIR).join("lib");
        let prefix = prefix.clone();
        entries(&lib)
            .filter_map(move |name| name.strip_prefix(&prefix)?.strip_suffix(".jar").filter(|v| !v.contains("searchableOptions")).map(str::to_string))
            .collect::<Vec<_>>()
    }))
}

/// JetBrains' `installPlugins` command for every IntelliJ found; it fails while that IntelliJ is running.
fn install_jetbrains_plugin() -> Result<(), String> {
    let launchers = intellij_launchers();
    if launchers.is_empty() {
        return Err(l!("IntelliJ IDEA wurde nicht gefunden."));
    }
    let results: Vec<Result<String, String>> = launchers.iter().map(|l| run(&l.to_string_lossy(), &["installPlugins", JETBRAINS_PLUGIN], &[])).collect();
    if results.iter().any(Result::is_ok) {
        return Ok(());
    }
    Err(l!("Plugin konnte nicht installiert werden. Bitte IntelliJ IDEA beenden und erneut versuchen. (%@)", results.into_iter().filter_map(Result::err).collect::<Vec<_>>().join("; ")))
}

// MARK: Claude Desktop

#[cfg(target_os = "macos")]
fn desktop_app() -> Option<PathBuf> {
    [PathBuf::from("/Applications"), paths::home_dir().join("Applications")].into_iter().map(|d| d.join("Claude.app")).find(|p| p.is_dir())
}

/// Claude Desktop is installed (the same check the tools page uses).
pub fn desktop_installed() -> bool {
    desktop_version().is_some()
}

fn desktop_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let app = desktop_app()?;
        let info = app.join("Contents/Info");
        return Some(run("/usr/bin/defaults", &["read", &info.to_string_lossy(), "CFBundleShortVersionString"], &[]).map(|v| v.trim().to_string()).unwrap_or_default());
    }
    #[cfg(windows)]
    {
        let script = "(Get-AppxPackage -Name Claude | Select-Object -First 1).Version";
        let version = run("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", script], &[]).map(|v| v.trim().to_string()).unwrap_or_default();
        if !version.is_empty() {
            return Some(version);
        }
        // The former .exe installer (without Cowork) installs per user here.
        let legacy = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"AnthropicClaude\claude.exe"));
        return legacy.filter(|p| p.is_file()).map(|_| String::new());
    }
    #[allow(unreachable_code)]
    None
}

/// Downloads into a fresh temporary folder (streamed, the installers are several hundred MB).
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn download(url: &str, file_name: &str) -> Result<PathBuf, String> {
    let mut response = gateway::agent(Duration::from_secs(1800))
        .get(url)
        .header("User-Agent", "KeyZapper")
        .call()
        .map_err(|e| l!("Download fehlgeschlagen: %@", e))?;
    if response.status().as_u16() != 200 {
        return Err(l!("Download fehlgeschlagen (HTTP %@).", response.status().as_u16()));
    }
    let dir = std::env::temp_dir().join(format!("KeyZapper-Claude-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(file_name);
    let mut file = std::fs::File::create(&target).map_err(|e| e.to_string())?;
    let mut body = response.body_mut().with_config().limit(8 * 1024 * 1024 * 1024).reader();
    std::io::copy(&mut body, &mut file).map_err(|e| l!("Download fehlgeschlagen: %@", e))?;
    Ok(target)
}

/// Mounts the `.dmg`, checks that Claude.app is signed by Anthropic and copies it to /Applications
/// (or ~/Applications without write access there).
#[cfg(target_os = "macos")]
fn install_desktop() -> Result<(), String> {
    if desktop_app().is_some() {
        return Ok(());
    }
    let dmg = download("https://claude.ai/api/desktop/darwin/universal/dmg/latest/redirect", "Claude.dmg")?;
    let dir = dmg.parent().map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    let mount = dir.join("mount");
    let mount_text = mount.to_string_lossy().into_owned();
    let result = (|| {
        run("/usr/bin/hdiutil", &["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", &mount_text, &dmg.to_string_lossy()], &[])?;
        let source = mount.join("Claude.app");
        let requirement = format!("=anchor apple generic and certificate leaf[subject.OU] = \"{ANTHROPIC_TEAM_ID}\"");
        run("/usr/bin/codesign", &["--verify", "--deep", "--strict", "-R", &requirement, &source.to_string_lossy()], &[])
            .map_err(|_| l!("Claude.app ist nicht von Anthropic signiert. Installation abgebrochen."))?;
        let system = Path::new("/Applications");
        let target_dir = if is_writable(system) { system.to_path_buf() } else { paths::home_dir().join("Applications") };
        std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
        run("/usr/bin/ditto", &[&source.to_string_lossy(), &target_dir.join("Claude.app").to_string_lossy()], &[]).map(|_| ())
    })();
    let _ = run("/usr/bin/hdiutil", &["detach", "-quiet", &mount_text], &[]);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(target_os = "macos")]
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".keyzapper-{}", uuid::Uuid::new_v4()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Installs the `.msix` for the current user (`Add-AppxPackage` checks Anthropic's signature).
#[cfg(windows)]
fn install_desktop() -> Result<(), String> {
    let arch = if std::env::consts::ARCH == "aarch64" { "arm64" } else { "x64" };
    let msix = download(&format!("https://claude.ai/api/desktop/win32/{arch}/msix/latest/redirect"), "Claude.msix")?;
    let path = msix.to_string_lossy().into_owned();
    let result = run(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", "Add-AppxPackage -Path $env:KEYZAPPER_PACKAGE -ErrorAction Stop"],
        &[("KEYZAPPER_PACKAGE", &path)],
    );
    if let Some(dir) = msix.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
    result.map(|_| ())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install_desktop() -> Result<(), String> {
    Err(l!("Auf diesem System nicht unterstützt."))
}

// MARK: Helpers

/// Names of the entries of `dir` (none if it does not exist).
fn entries(dir: &Path) -> impl Iterator<Item = String> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned())
}

fn newest(versions: impl Iterator<Item = String>) -> Option<String> {
    versions.max_by(|a, b| update::compare_versions(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_compares_versions_numerically() {
        let versions = ["2.1.9", "2.1.300", "2.1.29"].map(String::from);
        assert_eq!(newest(versions.into_iter()).as_deref(), Some("2.1.300"));
        assert_eq!(newest(std::iter::empty()), None);
    }

    #[test]
    fn tools_serialize_in_camel_case() {
        assert_eq!(serde_json::to_string(&Tool::VsCode).unwrap(), "\"vsCode\"");
        assert_eq!(serde_json::from_str::<Tool>("\"intellij\"").unwrap(), Tool::Intellij);
    }
}
