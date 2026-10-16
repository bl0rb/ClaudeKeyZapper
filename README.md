**English** · [Deutsch](README.de.md)

<img src="Resources/AppIcon.svg" width="96" alt="KeyZapper icon">

# KeyZapper

Desktop app for macOS and Windows that manages approved, project-specific LiteLLM keys in a local key file and assigns them to local project folders.
Claude Code (VS Code, IntelliJ, CLI) then automatically uses the matching key in every project. Several projects open at
the same time work independently of each other.

**Assign once, never switch keys again.** Claude Code fetches the matching key itself via `keyzapper-helper`,
even when KeyZapper is closed. The app explains this under “How it works”.

![KeyZapper overview with two key profiles, one SSO profile and assigned projects](docs/screenshots/keyzapper-overview.png)

<sub>Screenshot with made-up sample data.</sub>

```
Claude Code ──apiKeyHelper──▶ keyzapper-helper ──▶ local key file
     │
     └──── requests with project key ────▶ LiteLLM ──▶ Amazon Bedrock
```

## Why KeyZapper

LLM costs have to be tracked per project and kept within project budgets. For this, a dedicated middleware in front of
LiteLLM issues API keys per project. Each key carries its project's budget and allowed models, so every request made
with it is booked to that project. KeyZapper makes sure Claude Code always uses the key of the project you are working
in, so the costs land on the right project without anyone switching keys by hand.

## Features

* Profiles with name, LiteLLM endpoint (https; plain http only for a gateway on localhost), optional model alias and key. The key is stored exclusively in the local key file `keys.json` in the app data folder (no Keychain or credential-manager prompts).
* Assign project folders to a profile. The app only adds to `.claude/settings.local.json` and excludes the file from Git via `.git/info/exclude`.
* Status per project: active, differing, folder missing, key missing, conflicts with other settings.
* MCP servers (stdio, HTTP or SSE) defined once and selected per project. The app writes them into the repository's `.mcp.json` (excluded from Git) and approves them in `.claude/settings.local.json`, so Claude Code loads them there automatically. Remote servers with OAuth (e.g. Weave-MCP via Entra ID or Keycloak) take a client ID, callback port and scopes; Claude Code handles the browser sign-in.
* Test connection against LiteLLM (invalid or blocked key, rate limit, budget, gateway unreachable).
* Remaining budget per key, as reported by LiteLLM (`/key/info`): amount left of the limit and next reset, or the spend for keys without a limit.
* Removing an assignment only removes the entries the app set itself.
* Overview with all profiles, masked keys and assigned projects. Everything can be changed directly; keys can be copied and replaced.
* Update check against the GitHub releases with one-click installation; the version is shown in the app and under “About KeyZapper”.
* UI in English and German (follows the system language).
* Encrypted backup and restore of profiles, assignments and keys (`.kzbackup`, password-protected).
* Manageable via Intune: predefined profiles, gateway allowlist, defaults, minimum CLI version, OneDrive backup, update check.

## Requirements

* macOS 14 or newer or Windows 10 or newer (with the WebView2 runtime)
* Claude Code ≥ 2.1.288 (VS Code extension or the `claude` CLI for IntelliJ). Older versions only read the project assignment
  at startup directly in the project folder; the app warns in that case.

## Usage

1. **Create profile:** name, LiteLLM endpoint, key and optionally the gateway model names for Opus, Sonnet and Haiku. *Load models from gateway* lists the models the key may use (LiteLLM `/v1/models`) and fills the tiers. Profiles managed by IT are already there; just enter the key.
2. **Assign project:** choose a folder and select a profile. For Git repositories the assignment applies to the whole repository
   including subfolders and worktrees. Claude Code reads the project settings only at the root of the main checkout.
3. **Restart the Claude session in the project**; in VS Code, start a new conversation or reload the window.

The overview shows every profile with endpoint, model and the assigned key (masked, e.g. `••••7f3a`) along with all
assigned projects. For each project you can change the profile, reapply the settings and remove the assignment.

**Copy key:** The key is placed on the clipboard marked as confidential so that clipboard managers do not store it.
After 60 seconds it is removed again, unless something else has been copied in the meantime.

**Changing a key:** “Change” replaces the key; this takes effect immediately for new sessions. Running sessions pick up the key after the
helper cache expires (default 5 min), after an HTTP 401, or after a restart.

**If a key is missing** or blocked, requests fail. Claude Code never falls back to other credentials.

## SSO profiles

Profiles can authenticate via a static LiteLLM key (Key type) or via OIDC login (SSO type). SSO requires
an OIDC-verifying proxy in front of LiteLLM that validates the token (access token by default) and injects the virtual key into requests.

**SSO fields:** Issuer (https URL), Client ID (public client, no secret), Scope (default `openid profile offline_access`),
Token type (*Access token* default, or *ID token* for gateways that expect the OIDC ID token as bearer token).
For Microsoft Entra ID use scope `api://<app-id>/.default offline_access` with access tokens; a refresh token is required.
With ID tokens the scope must include `openid` (Entra ID: also `profile`, otherwise the `oid` claim is missing) and `offline_access`;
the token's expiry comes from its `exp` claim.

**Login:** *Sign In* opens the system browser for Authorization Code flow with PKCE. The redirect URI is a loopback address
`http://127.0.0.1:<random port>/callback`. To register the redirect URI:

* **Microsoft Entra ID:** App registration › Mobile and desktop applications › Redirect URI: `http://127.0.0.1`
  (Entra ignores the port for loopback redirects).
* **Keycloak:** Public client › Standard flow › PKCE S256 › Valid redirect URI: `http://127.0.0.1/*`.

**Tokens:** Refresh and access tokens are stored locally in `sso-tokens.json` in the app data folder (owner-only, `0600`),
never in backups or via "copy key". The helper returns a cached access token or refreshes it once, serialized safely across parallel
sessions. Exit code 67 means the session expired; sign in again in KeyZapper.

**Settings:** Set `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` in the profile's Environment field to a value below the token lifetime
(e.g., 900000 for 60–90 min tokens). For long streams over a gateway, also set `API_TIMEOUT_MS`,
`CLAUDE_STREAM_IDLE_TIMEOUT_MS`, `CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS`.

**Limitations:** SSO profiles do not support Budget-Killer pool, budget display, or key copying. Model loading and test connection
use the access token.

## Claude for Microsoft 365

The same profiles can serve Claude for Excel, Word and PowerPoint (Outlook optional). The Office add-in cannot call
`keyzapper-helper`; it fetches its configuration from an HTTPS bootstrap endpoint with the user's Microsoft Entra
token, as Anthropic's third-party deployment documents. KeyZapper ships that endpoint as the separate service
`keyzapper-office-bootstrap` (`desktop/office-bootstrap`), which validates the Entra token (signature, issuer, audience,
tenant, `oid`), maps the user to a released profile and answers with gateway URL, a short-lived gateway token
(LiteLLM virtual key), inference headers and the profile's remote MCP servers.

Office is off per profile until an administrator releases it in the profile editor (*Claude for Microsoft 365*: allowed
Office apps, Entra object IDs, inference headers, MCP servers, lifetime). *Backup › Export Office bootstrap
configuration…* writes the deployment file for the service; it contains no keys. Keys are issued by the service itself.
Setup, server configuration, manifest values (`entra_sso=1`, `bootstrap_url`) and troubleshooting:
[docs/office-bootstrap.md](docs/office-bootstrap.md).

## How it works

**Setup (once):** The key goes to the helper via stdin and ends up only in the key file `keys.json`. The app writes only references into the project.

![Setup: KeyZapper stores the key via keyzapper-helper and writes settings.local.json in repo A and repo B](docs/diagrams/keyzapper-flow-setup.svg)

**Runtime (every Claude request):** Claude Code fetches the key for each project itself via the helper, even when the app is closed.

![Runtime: Claude Code in repo A and repo B calls keyzapper-helper with its own profile ID and sends key A or key B to LiteLLM](docs/diagrams/keyzapper-flow-runtime.svg)

The full view with explanations is at [docs/diagrams/keyzapper-flow.html](docs/diagrams/keyzapper-flow.html).

The app writes to `<project>/.claude/settings.local.json`:

| Key | Value |
|---|---|
| `apiKeyHelper` | `'/Applications/KeyZapper.app/Contents/MacOS/keyzapper-helper' credential --profile <UUID>` on macOS, the quoted path of `keyzapper-helper.exe` in the install folder on Windows (Key and SSO profiles) |
| `env.ANTHROPIC_BASE_URL` | LiteLLM endpoint of the profile |
| `env.ANTHROPIC_DEFAULT_OPUS_MODEL`, `…_SONNET_MODEL`, `…_HAIKU_MODEL`, `env.ANTHROPIC_MODEL` | Gateway model names of the profile (if set), e.g. `eu.anthropic.claude-sonnet-5-…` |
| further `env.*` | Additional environment variables of the profile, e.g. `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` for Bedrock via LiteLLM |
| `env.ANTHROPIC_API_KEY`, `env.ANTHROPIC_AUTH_TOKEN` | `""`. This neutralizes inherited values from the IDE or shell that would otherwise produce mixed auth headers. |

Existing settings are preserved. Changes are atomic and idempotent: setting up repeatedly changes nothing.

Conflicts that prevent setup:
* Managed Claude settings that set `apiKeyHelper` or `ANTHROPIC_*`
* `CLAUDE_CODE_USE_BEDROCK`, `_VERTEX` or `_FOUNDRY` in any settings level
* foreign values for the same keys in `settings.local.json`
* a checked-in `settings.local.json`

**Helper interface:** `keyzapper-helper credential|store|status|delete --profile <UUID>`. The key flows only via stdin and stdout,
never via arguments or logs.

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 64 | usage |
| 65 | unknown profile |
| 66 | key missing |
| 67 | session expired (SSO only; sign in again) |
| 70 | internal |
| 77 | key store (`keys.json`) unreadable or not writable |
| 78 | metadata corrupt, endpoint not https (http only for localhost) or not in `AllowedGatewayHosts` |

Metadata without keys is stored in `state.json` in the app data folder, the keys in `keys.json` next to it: `~/Library/Application Support/KeyZapper` on macOS (owner-only), `%LOCALAPPDATA%\KeyZapper` on Windows (user profile folder). Keys from the former Keychain-based version are offered for a one-time move on macOS (macOS may ask for permission once per key).

## Distribution via Microsoft Intune

Every Git tag `X.Y.Z` builds the packages `KeyZapper-X.Y.Z.pkg` (macOS) and `KeyZapper-X.Y.Z.msi` (Windows) via CI and attaches both to the GitHub release (see [Release](#release)).

**macOS: create the app:** *Apps › All apps › Create › macOS app (PKG)*. This is the unmanaged PKG type, which also accepts
unsigned packages; it requires the Intune Management Agent ≥ 2308.006.
* Minimum OS: macOS 14
* Detection rules › Included apps: `io.github.bl0rb.keyzapper` with the package version, “Ignore app version” = No

**Windows: create the app:** *Apps › All apps › Create › Line-of-business app*, upload the `.msi` (per-machine install to Program Files).

**Signing:** Keys are not in the Keychain, so signing does not affect key access. For macOS, `SIGN_IDENTITY` and `INSTALLER_IDENTITY`
(`desktop/scripts/build-pkg.sh`) sign with a Developer ID; without them the build is ad-hoc signed.

**Uninstall:** Intune has no uninstall assignment for PKG apps. First choose “Remove assignment” in the app,
then delete `/Applications/KeyZapper.app`. On Windows, uninstall the MSI via Intune or *Apps & features*.

### Configuration

**macOS:** *Devices › Configuration › Create › macOS › Templates › Preference file*, Preference domain name `io.github.bl0rb.keyzapper`.
Template for the Property list file: [docs/intune/io.github.bl0rb.keyzapper.plist](docs/intune/io.github.bl0rb.keyzapper.plist). It contains only
key-value pairs without the `<plist>`/`<dict>` wrapper, as Intune requires.

**Windows:** The same keys are read as registry values under `HKLM\SOFTWARE\Policies\KeyZapper` (machine policy wins) or `HKCU\SOFTWARE\Policies\KeyZapper`:
strings as `REG_SZ`, lists as `REG_MULTI_SZ`, switches as `REG_DWORD` 0/1, `ManagedProfiles` as a JSON array in a `REG_SZ`.
Template: [docs/intune/keyzapper-windows.reg](docs/intune/keyzapper-windows.reg) (deploy e.g. via PowerShell script or Settings catalog).

**Important:** For users receiving SSO profiles via managed settings, do not set `apiKeyHelper`, `env.ANTHROPIC_BASE_URL`,
`ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` in `managed-settings.json` (or `managed-settings.d/*.json`). Managed settings
outrank project settings; KeyZapper aborts binding on conflict.

| Key | Type | Effect |
|---|---|---|
| `ManagedProfiles` | Array of dicts: `Name`, `Endpoint`, optional `OpusModel`, `SonnetModel`, `HaikuModel`, `ModelAlias`, `Environment` (dict), `ID`, `Type` (`apiKey` default or `oidc`), `OIDCIssuer`, `OIDCClientID`, `OIDCScope`, `OIDCTokenType` | Profiles are created automatically and cannot be edited or deleted; developers only enter the key (or sign in for SSO). Without `ID` the profile ID is derived from `Name`, so renaming creates a new profile. For SSO (`oidc` type), `OIDCIssuer` and `OIDCClientID` are required; `OIDCScope` defaults to `openid profile offline_access`; `OIDCTokenType` is `access` (default) or `id`, other values discard the profile. |
| `AllowedGatewayHosts` | Array of strings (`host` or `*.domain`) | Profiles only for these hosts. The helper does not release keys for other hosts (exit 78). Applies to the gateway host only; the IdP host need not be listed. Empty means no restriction. |
| `DefaultEndpoint`, `DefaultModelAlias` | String | Prefill when creating your own profiles |
| `MinimumClaudeCodeVersion` | String | Threshold for the warning about outdated Claude CLIs (default 2.1.288) |
| `OneDriveBackup` | Bool | Backup of profiles and assignments to OneDrive (see below) |
| `BackupDirectory` | String, `~` allowed | Explicit backup folder; enables the backup even without `OneDriveBackup` |
| `UpdateCheckEnabled` | Bool (default `true`) | In-app update check. When distributing via Intune, set to `false`, otherwise Intune may overwrite a newer version that was installed by the user. |
| `AllowKeyExport` | Bool (default `true`) | Whether keys may leave the key store via the app. `false`: no copying to the clipboard; backups contain profiles and assignments only. |

Test locally (user level; values managed via Intune take precedence):

```bash
defaults write io.github.bl0rb.keyzapper AllowedGatewayHosts -array litellm.firma.example
```

### OneDrive backup

The app backs up profiles and project assignments to `~/Library/CloudStorage/OneDrive-<Company>/KeyZapper/keyzapper-backup.json` (macOS) or into the `KeyZapper` folder of the OneDrive folder (Windows).
A business account takes precedence over “OneDrive-Personal”. **Keys are never backed up.** On a new computer the app offers
to restore or discard the backup (“Restore” or “Discard”). Assignments are only applied for existing folders; the keys
have to be entered again. As long as a found backup has been neither restored nor discarded, it is not overwritten.

## Claude settings check and default profile

*Check Claude settings* (shield icon in the toolbar) checks `~/.claude/settings.json` for problems Claude Code accepts
silently: invalid JSON (validated with the same strict `JSON.parse` as Claude Code, e.g. trailing commas), plaintext keys,
an endpoint without a key, non-text values in `env`, provider switches that bypass LiteLLM, model variables and fixed
model IDs instead of an alias. A banner shows problems at any time.

* **Repair / Create** makes the file valid, removes plaintext keys and turns `env` values into text. A missing file is
  created. Before every change KeyZapper saves a backup `settings.json.keyzapper-<time>.bak`.
* **Default profile** writes a profile into `~/.claude/settings.json`, so Claude Code uses it in all folders without their
  own assignment instead of a plaintext key or the normal login. Assigned projects keep their own profile.

## Deactivating KeyZapper

The switch in the toolbar deactivates KeyZapper: it removes its entries from all projects and from
`~/.claude/settings.json`, so Claude Code uses its normal login everywhere. The assignments stay saved and are written
again when activating.

## Encrypted backup and restore

*Backup › Export Backup…* (toolbar or *File* menu) writes profiles, project assignments and keys into a `.kzbackup` file,
encrypted with a password of at least 12 characters. *Import Backup…* restores them on the same or another computer:
profiles and keys with the same ID are overwritten, and projects are assigned if their folder exists. The password is
stored nowhere – without it the backup cannot be restored.

Encryption: AES-256-GCM with a key derived via PBKDF2-HMAC-SHA256 (600,000 iterations, random salt). The file
parameters are authenticated, so a wrong password or a modified file is detected. The file is saved with permissions
`0600`. With `AllowKeyExport = false` the backup contains no keys. Unlike the automatic OneDrive backup (never keys),
this backup is created manually and on demand.

## Updates and version

On startup the app checks the latest [GitHub release](https://github.com/bl0rb/ClaudeKeyZapper/releases); you can also check manually via
*KeyZapper › Check for Updates…* or the version line at the bottom of the app. If a newer version is available, “Install”
downloads the installer (`.pkg` on macOS, `.msi` on Windows), verifies the SHA-256 checksum published by GitHub and the release's
minisign signature (`<installer>.minisig`) against the public key built into the app ([desktop/update-key.pub](desktop/update-key.pub)),
then opens the system installer. Without a checksum or a valid signature the update is refused; the signed file name must match, so an
older signed package cannot pass as a newer one.
Administrator rights are required. Packages are only downloaded from `github.com`. The installed version is shown at the bottom of the
app and under *KeyZapper › About KeyZapper*.

With Intune, `UpdateCheckEnabled = false` turns the check off. Updates then arrive as a new package in Intune.

## Known limitations

* One profile per Git repository; subfolders and worktrees cannot be assigned differently.
* If Claude is started outside the assigned area (e.g. in a subfolder of a non-Git folder), it uses the
  developer's default sign-in.
* The IntelliJ integration has not yet been verified in the pilot.

## Development

```
desktop/core        Rust library keyzapper-core: data model, metadata, key store, helper logic, Claude settings, Intune configuration, backup
desktop/src-tauri   Tauri app (macOS, Windows) and keyzapper-helper (apiKeyHelper for Claude Code)
desktop/ui          Web interface (HTML, JS, CSS)
desktop/locales     English translations (German source strings)
desktop/scripts     build-pkg.sh (macOS), build-msi.sh (Windows), l10n_check.py
```

Tests:

```bash
cd desktop && cargo test -p keyzapper-core
```

Run the app in development mode (needs Rust and Node.js):

```bash
cd desktop && npm ci && npm run dev
```

Build the installer locally (`desktop/dist/KeyZapper-<version>.pkg` on macOS, `.msi` on Windows in Git Bash):

```bash
cd desktop && VERSION=1.0.0 scripts/build-pkg.sh
```

```bash
cd desktop && VERSION=1.0.0 scripts/build-msi.sh
```

## Release

A tag in the format `X.Y.Z` starts [.github/workflows/release.yml](.github/workflows/release.yml). The workflow runs the
tests, builds `KeyZapper-X.Y.Z.pkg` (macOS) and `KeyZapper-X.Y.Z.msi` (Windows), signs both with minisign and publishes them with
their `.minisig` files in one GitHub release. Signing needs the repository secret `MINISIGN_SECRET_KEY` (the secret key matching
`desktop/update-key.pub`); without it the workflow stops instead of publishing unsigned installers.
A tag like `X.Y.Z-beta.N` creates a pre-release, which is only offered to installed betas and via “Check for beta”.

```bash
git tag 1.0.0
```

```bash
git push origin 1.0.0
```

## License

[MIT](LICENSE) © 2026 bl0rb
