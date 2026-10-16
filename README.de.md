[English](README.md) · **Deutsch**

<img src="Resources/AppIcon.svg" width="96" alt="KeyZapper-Icon">

# KeyZapper

Desktop-App für macOS und Windows, die freigegebene, projektbezogene LiteLLM-Keys in einer lokalen Key-Datei verwaltet und lokalen Projektordnern zuordnet.
Claude Code (VS Code, IntelliJ, CLI) verwendet danach in jedem Projekt automatisch den passenden Key. Mehrere gleichzeitig
offene Projekte arbeiten unabhängig voneinander.

**Einmal zuordnen, danach nie wieder Keys wechseln.** Claude Code holt sich den passenden Key über `keyzapper-helper`
selbst, auch wenn KeyZapper geschlossen ist. Die App erklärt das unter „So funktioniert’s“.

![KeyZapper-Übersicht mit zwei Key-Profilen, einem SSO-Profil und zugeordneten Projekten](docs/screenshots/keyzapper-overview.de.png)

<sub>Screenshot mit ausgedachten Beispieldaten.</sub>

```
Claude Code ──apiKeyHelper──▶ keyzapper-helper ──▶ lokale Key-Datei
     │
     └──── Anfragen mit Projekt-Key ────▶ LiteLLM ──▶ Amazon Bedrock
```

## Warum KeyZapper

LLM-Kosten müssen projektbezogen erfasst und in Projektbudgets gehalten werden. Dafür stellt eine eigens entwickelte
Middleware vor LiteLLM API-Keys pro Projekt aus. Jeder Key trägt das Budget und die freigegebenen Modelle seines
Projekts, sodass jede Anfrage mit diesem Key auf das Projekt gebucht wird. KeyZapper sorgt dafür, dass Claude Code immer
den Key des Projekts verwendet, in dem gerade gearbeitet wird. So landen die Kosten ohne manuelles Key-Wechseln auf dem
richtigen Projekt.

## Funktionen

* Profile mit Name, LiteLLM-Endpunkt (https; einfaches http nur für ein Gateway auf localhost), optionalem Modellalias und Key. Der Key liegt ausschließlich in der lokalen Key-Datei `keys.json` im App-Datenordner (keine Schlüsselbund- oder Anmeldeinformationsverwaltungs-Abfragen).
* Projektordner einem Profil zuordnen. Die App ergänzt nur `.claude/settings.local.json` und schließt die Datei per `.git/info/exclude` von Git aus.
* Status je Projekt: aktiv, abweichend, Ordner fehlt, Key fehlt, Konflikte mit anderen Einstellungen.
* MCP-Server (stdio, HTTP oder SSE) einmal definieren und je Projekt auswählen. Die App schreibt sie in die `.mcp.json` des Repositorys (von Git ausgeschlossen) und schaltet sie in `.claude/settings.local.json` frei, damit Claude Code sie dort automatisch lädt. Remote-Server mit OAuth (z. B. Weave-MCP über Entra ID oder Keycloak) bekommen Client-ID, Callback-Port und Scopes; den Browser-Login übernimmt Claude Code.
* Verbindungstest gegen LiteLLM (ungültiger oder gesperrter Key, Rate-Limit, Budget, Gateway nicht erreichbar).
* Verbleibendes Budget je Key laut LiteLLM (`/key/info`): Rest vom Limit und nächster Reset, bei Keys ohne Limit die bisherigen Ausgaben.
* Rücknahme entfernt nur die Einträge, die die App selbst gesetzt hat.
* Übersicht mit allen Profilen, maskierten Keys und zugeordneten Projekten. Alles lässt sich direkt ändern; Keys lassen sich kopieren und ersetzen.
* Update-Prüfung gegen die GitHub-Releases mit Installation per Klick; die Version steht in der App und unter „Über KeyZapper“.
* Oberfläche auf Deutsch und Englisch (folgt der Systemsprache).
* Verschlüsseltes Backup und Wiederherstellen von Profilen, Zuordnungen und Keys (`.kzbackup`, passwortgeschützt).
* Steuerbar per Intune: vorgegebene Profile, Gateway-Allowlist, Standardwerte, CLI-Mindestversion, OneDrive-Backup, Update-Prüfung.

## Voraussetzungen

* macOS 14 oder neuer oder Windows 10 oder neuer (mit WebView2-Runtime)
* Claude Code ≥ 2.1.288 (VS-Code-Erweiterung bzw. `claude`-CLI für IntelliJ). Ältere Versionen lesen die Projektzuordnung
  nur beim Start direkt im Projektordner; die App warnt in dem Fall.

## Nutzung

1. **Profil anlegen:** Name, LiteLLM-Endpunkt, Key und optional die Modellnamen des Gateways für Opus, Sonnet und Haiku. *Modelle vom Gateway laden* listet die Modelle, die der Key nutzen darf (LiteLLM `/v1/models`), und belegt die Stufen vor. Von der IT vorgegebene Profile sind bereits da; dort nur den Key eintragen.
2. **Projekt zuordnen:** Ordner wählen und Profil auswählen. Bei Git-Repositories gilt die Zuordnung für das ganze Repository
   samt Unterordnern und Worktrees. Claude Code liest die Projekteinstellungen nur am Root des Haupt-Checkouts.
3. **Claude-Sitzung im Projekt neu starten**, in VS Code eine neue Unterhaltung bzw. das Fenster neu laden.

Die Übersicht zeigt jedes Profil mit Endpunkt, Modell und dem zugewiesenen Key (maskiert, z. B. `••••7f3a`) samt allen
zugeordneten Projekten. Pro Projekt lassen sich das Profil wechseln, die Einstellungen neu schreiben und die Zuordnung entfernen.

**Key kopieren:** Der Key landet als „vertraulich“ markiert in der Zwischenablage, damit Clipboard-Manager ihn nicht speichern.
Nach 60 Sekunden wird er wieder entfernt, sofern inzwischen nichts anderes kopiert wurde.

**Keywechsel:** „Ändern“ ersetzt den Key; das wirkt sofort für neue Sitzungen. Laufende Sitzungen übernehmen den Key nach Ablauf des
Helper-Caches (Standard 5 min), nach einem HTTP 401 oder nach einem Neustart.

**Fehlt ein Key** oder ist er gesperrt, schlagen die Anfragen fehl. Claude Code weicht nie auf andere Zugangsdaten aus.

## SSO-Profile

Profile können sich über einen statischen LiteLLM-Key (Typ Key) oder per OIDC-Anmeldung (Typ SSO) authentifizieren.
SSO erfordert einen OIDC-validierenden Proxy vor LiteLLM, der das Token (standardmäßig das Access-Token) prüft und den virtuellen Key in die Anfragen injiziert.

**SSO-Felder:** Issuer (https-URL), Client-ID (öffentlicher Client, kein Secret), Scope (Standard `openid profile offline_access`),
Token-Art (Standard *Access-Token*, oder *ID-Token* für Gateways, die das OIDC-ID-Token als Bearer-Token erwarten).
Für Microsoft Entra ID mit Access-Token Scope `api://<app-id>/.default offline_access` verwenden; ein Refresh-Token ist erforderlich.
Mit ID-Token muss der Scope `openid` (bei Entra ID auch `profile`, sonst fehlt der Claim `oid`) und `offline_access` enthalten;
der Ablauf kommt aus dem `exp`-Claim des Tokens.

**Anmeldung:** *Anmelden* öffnet den System-Browser für Authorization-Code-Flow mit PKCE. Die Redirect-URI ist eine Loopback-Adresse
`http://127.0.0.1:<zufälliger Port>/callback`. Die Redirect-URI registrieren:

* **Microsoft Entra ID:** App-Registrierung › Mobile und Desktop-Anwendungen › Redirect-URI: `http://127.0.0.1`
  (Entra ignoriert den Port bei Loopback-Umleitungen).
* **Keycloak:** Öffentlicher Client › Standard flow › PKCE S256 › Valid redirect URI: `http://127.0.0.1/*`.

**Tokens:** Refresh- und Access-Tokens werden lokal in `sso-tokens.json` im App-Datensatz-Ordner gespeichert (nur Besitzer, `0600`),
nie in Backups oder per „Key kopieren". Der Helper gibt ein gecachtes Access-Token zurück oder aktualisiert es einmal,
serialisiert sicher über parallele Sitzungen. Exit-Code 67 bedeutet abgelaufene Sitzung; in KeyZapper neu anmelden.

**Einstellungen:** `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` im Umgebungsbereich des Profils auf einen Wert unter der Token-Lebensdauer
setzen (z. B. 900000 für 60–90 Min. Tokens). Für lange Streams über ein Gateway auch `API_TIMEOUT_MS`,
`CLAUDE_STREAM_IDLE_TIMEOUT_MS`, `CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS` setzen.

**Einschränkungen:** SSO-Profile unterstützen keinen Budget-Killer-Pool, keine Budget-Anzeige und kein Key-Kopieren.
Modell-Laden und Verbindungstest verwenden das Access-Token.

## Claude for Microsoft 365

Dieselben Profile können Claude for Excel, Word und PowerPoint bedienen (Outlook optional). Das Office-Add-in kann
`keyzapper-helper` nicht aufrufen; es holt seine Konfiguration mit dem Microsoft-Entra-Token des Benutzers von einem
HTTPS-Bootstrap-Endpunkt, wie Anthropics Drittanbieter-Deployment es vorsieht. KeyZapper liefert diesen Endpunkt als
separaten Dienst `keyzapper-office-bootstrap` (`desktop/office-bootstrap`): Er prüft das Entra-Token (Signatur, Issuer,
Audience, Tenant, `oid`), ordnet den Benutzer einem freigegebenen Profil zu und antwortet mit Gateway-URL, einem
kurzlebigen Gateway-Token (LiteLLM Virtual Key), Inference-Headern und den Remote-MCP-Servern des Profils.

Office ist pro Profil aus, bis ein Administrator es im Profil-Editor freigibt (*Claude for Microsoft 365*: erlaubte
Office-Anwendungen, Entra-Objekt-IDs, Inference-Header, MCP-Server, Gültigkeit). *Backup › Office-Bootstrap-Konfiguration
exportieren …* schreibt die Deployment-Datei für den Dienst; sie enthält keine Keys. Keys vergibt der Dienst selbst.
Einrichtung, Serverkonfiguration, Manifest-Werte (`entra_sso=1`, `bootstrap_url`) und Fehlersuche:
[docs/office-bootstrap.md](docs/office-bootstrap.md) (englisch).

## So funktioniert es

**Einrichtung (einmalig):** Der Key geht per stdin an den Helper und landet nur in der Key-Datei `keys.json`. Ins Projekt schreibt die App nur Verweise.

![Einrichtung: KeyZapper speichert den Key über keyzapper-helper und schreibt settings.local.json in Repo A und Repo B](docs/diagrams/keyzapper-flow-setup.de.svg)

**Laufzeit (jede Claude-Anfrage):** Claude Code holt den Key pro Projekt selbst über den Helper, auch wenn die App geschlossen ist.

![Laufzeit: Claude Code in Repo A und Repo B ruft keyzapper-helper mit der eigenen Profil-ID auf und sendet Key A bzw. Key B an LiteLLM](docs/diagrams/keyzapper-flow-runtime.de.svg)

Die Gesamtansicht mit Erläuterungen liegt unter [docs/diagrams/keyzapper-flow.de.html](docs/diagrams/keyzapper-flow.de.html).

Die App schreibt in `<Projekt>/.claude/settings.local.json`:

| Schlüssel | Wert |
|---|---|
| `apiKeyHelper` | `'/Applications/KeyZapper.app/Contents/MacOS/keyzapper-helper' credential --profile <UUID>` unter macOS, unter Windows der gequotete Pfad von `keyzapper-helper.exe` im Installationsordner (Key- und SSO-Profile) |
| `env.ANTHROPIC_BASE_URL` | LiteLLM-Endpunkt des Profils |
| `env.ANTHROPIC_DEFAULT_OPUS_MODEL`, `…_SONNET_MODEL`, `…_HAIKU_MODEL`, `env.ANTHROPIC_MODEL` | Modellnamen des Gateways aus dem Profil (falls gesetzt), z. B. `eu.anthropic.claude-sonnet-5-…` |
| weitere `env.*` | Zusätzliche Umgebungsvariablen des Profils, z. B. `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` für Bedrock über LiteLLM |
| `env.ANTHROPIC_API_KEY`, `env.ANTHROPIC_AUTH_TOKEN` | `""`. Das neutralisiert geerbte Werte aus IDE oder Shell, die sonst gemischte Auth-Header erzeugen. |

Bestehende Einstellungen bleiben erhalten. Änderungen sind atomar und idempotent: Wiederholtes Einrichten ändert nichts.

Konflikte, die das Einrichten verhindern:
* Verwaltete Claude-Einstellungen, die `apiKeyHelper` oder `ANTHROPIC_*` setzen
* `CLAUDE_CODE_USE_BEDROCK`, `_VERTEX` oder `_FOUNDRY` in einer Einstellungsebene
* fremde Werte für dieselben Schlüssel in `settings.local.json`
* eine eingecheckte `settings.local.json`

**Helper-Schnittstelle:** `keyzapper-helper credential|store|status|delete --profile <UUID>`. Der Key fließt nur über stdin und stdout,
nie über Argumente oder Logs.

| Exit-Code | Bedeutung |
|---|---|
| 0 | ok |
| 64 | Aufruf |
| 65 | unbekanntes Profil |
| 66 | Key fehlt |
| 67 | Sitzung abgelaufen (nur SSO; neu anmelden) |
| 70 | intern |
| 77 | Key-Speicher (`keys.json`) nicht lesbar oder nicht beschreibbar |
| 78 | Metadaten defekt, Endpunkt nicht https (http nur für localhost) oder nicht in `AllowedGatewayHosts` |

Metadaten ohne Keys liegen in `state.json` im App-Datenordner, die Keys daneben in `keys.json`: `~/Library/Application Support/KeyZapper` unter macOS (nur für den Besitzer), `%LOCALAPPDATA%\KeyZapper` unter Windows (Benutzerprofil-Ordner). Keys aus der früheren Schlüsselbund-Version bietet die App unter macOS einmalig zur Übernahme an (macOS fragt dabei ggf. je Key um Erlaubnis).

## Verteilung über Microsoft Intune

Jeder Git-Tag `X.Y.Z` baut per CI die Pakete `KeyZapper-X.Y.Z.pkg` (macOS) und `KeyZapper-X.Y.Z.msi` (Windows) und hängt beide an das GitHub-Release (siehe [Release](#release)).

**macOS: App anlegen:** *Apps › Alle Apps › Erstellen › macOS-App (PKG)*. Das ist der nicht verwaltete PKG-Typ, der auch
unsignierte Pakete annimmt; Voraussetzung ist der Intune Management Agent ≥ 2308.006.
* Mindest-OS: macOS 14
* Erkennungsregeln › Enthaltene Apps: `io.github.bl0rb.keyzapper` mit der Paketversion, „App-Version ignorieren“ = Nein

**Windows: App anlegen:** *Apps › Alle Apps › Erstellen › Branchen-App (Line-of-business)*, die `.msi` hochladen (Installation pro Computer nach Program Files).

**Signatur:** Die Keys liegen nicht im Schlüsselbund, die Signatur beeinflusst den Key-Zugriff daher nicht. Für macOS signieren
`SIGN_IDENTITY` und `INSTALLER_IDENTITY` (`desktop/scripts/build-pkg.sh`) mit einer Developer ID; ohne sie ist der Build ad-hoc-signiert.

**Deinstallation:** Intune kennt für PKG-Apps keine Deinstallationszuweisung. Vorher in der App „Zuordnung entfernen“
wählen, dann `/Applications/KeyZapper.app` löschen. Unter Windows die MSI über Intune oder *Apps & Features* deinstallieren.

### Konfiguration

**macOS:** *Geräte › Konfiguration › Erstellen › macOS › Vorlagen › Einstellungsdatei (Preference file)*, Domäne `io.github.bl0rb.keyzapper`.
Vorlage: [docs/intune/io.github.bl0rb.keyzapper.plist](docs/intune/io.github.bl0rb.keyzapper.plist). Sie enthält nur
Schlüssel-Wert-Paare ohne `<plist>`/`<dict>`-Rahmen, wie Intune es verlangt.

**Windows:** Dieselben Schlüssel werden als Registrywerte unter `HKLM\SOFTWARE\Policies\KeyZapper` (Computer-Richtlinie hat Vorrang) bzw. `HKCU\SOFTWARE\Policies\KeyZapper` gelesen:
Zeichenfolgen als `REG_SZ`, Listen als `REG_MULTI_SZ`, Schalter als `REG_DWORD` 0/1, `ManagedProfiles` als JSON-Array in einem `REG_SZ`.
Vorlage: [docs/intune/keyzapper-windows.reg](docs/intune/keyzapper-windows.reg) (z. B. per PowerShell-Skript oder Einstellungskatalog verteilen).

**Wichtig:** Für Benutzer, die SSO-Profile per verwalteten Einstellungen erhalten, nicht `apiKeyHelper`, `env.ANTHROPIC_BASE_URL`,
`ANTHROPIC_API_KEY` oder `ANTHROPIC_AUTH_TOKEN` in `managed-settings.json` (oder `managed-settings.d/*.json`) setzen.
Verwaltete Einstellungen haben Vorrang vor Projekteinstellungen; KeyZapper bricht das Binding bei Konflikt ab.

| Schlüssel | Typ | Wirkung |
|---|---|---|
| `ManagedProfiles` | Array von Dicts: `Name`, `Endpoint`, optional `OpusModel`, `SonnetModel`, `HaikuModel`, `ModelAlias`, `Environment` (Dict), `ID`, `Type` (`apiKey` Standard oder `oidc`), `OIDCIssuer`, `OIDCClientID`, `OIDCScope`, `OIDCTokenType` | Profile werden automatisch angelegt und sind nicht editier- oder löschbar; Entwickler tragen nur den Key ein (oder melden sich für SSO an). Ohne `ID` wird die Profil-ID aus `Name` abgeleitet, eine Umbenennung erzeugt also ein neues Profil. Bei SSO (`oidc`) sind `OIDCIssuer` und `OIDCClientID` erforderlich; `OIDCScope` Standard `openid profile offline_access`; `OIDCTokenType` ist `access` (Standard) oder `id`, andere Werte verwerfen das Profil. |
| `AllowedGatewayHosts` | Array von Strings (`host` oder `*.domain`) | Profile nur für diese Hosts. Der Helper gibt Keys für andere Hosts nicht heraus (Exit 78). Gilt nur für den Gateway-Host; der IdP-Host muss nicht gelistet sein. Leer bedeutet keine Einschränkung. |
| `DefaultEndpoint`, `DefaultModelAlias` | String | Vorbelegung beim Anlegen eigener Profile |
| `MinimumClaudeCodeVersion` | String | Schwelle für die Warnung vor veralteten Claude-CLIs (Standard 2.1.288) |
| `OneDriveBackup` | Bool | Backup von Profilen und Zuordnungen nach OneDrive (siehe unten) |
| `BackupDirectory` | String, `~` erlaubt | Expliziter Backup-Ordner; aktiviert das Backup auch ohne `OneDriveBackup` |
| `UpdateCheckEnabled` | Bool (Standard `true`) | In-App-Update-Prüfung. Bei Verteilung über Intune auf `false` setzen, sonst kann Intune eine selbst installierte neuere Version wieder überschreiben. |
| `AllowKeyExport` | Bool (Standard `true`) | Ob Keys die App verlassen dürfen. `false`: kein Kopieren in die Zwischenablage; Backups enthalten nur Profile und Zuordnungen. |

Lokal testen (Benutzerebene; per Intune verwaltete Werte haben Vorrang):

```bash
defaults write io.github.bl0rb.keyzapper AllowedGatewayHosts -array litellm.firma.example
```

### OneDrive-Backup

Die App sichert Profile und Projektzuordnungen nach `~/Library/CloudStorage/OneDrive-<Firma>/KeyZapper/keyzapper-backup.json` (macOS) bzw. in den Ordner `KeyZapper` im OneDrive-Ordner (Windows).
Ein Geschäftskonto hat Vorrang vor „OneDrive-Personal“. **Keys werden nie gesichert.** Auf einem neuen Rechner bietet die App
an, das Backup wiederherzustellen oder zu verwerfen. Zuordnungen werden nur für vorhandene Ordner übernommen, die Keys
trägt man neu ein. Solange ein gefundenes Backup nicht wiederhergestellt oder verworfen wurde, wird es nicht überschrieben.

## Prüfung der Claude-Einstellungen und Standardprofil

*Claude-Einstellungen prüfen* (Schild-Symbol in der Symbolleiste) prüft `~/.claude/settings.json` auf Probleme, die Claude
Code stillschweigend hinnimmt: ungültiges JSON (geprüft mit demselben strengen `JSON.parse` wie Claude Code, z. B. Kommas
nach dem letzten Eintrag), Klartext-Keys, einen Endpunkt ohne Key, Nicht-Text-Werte in `env`, Provider-Schalter an
LiteLLM vorbei, Modell-Variablen und feste Modell-IDs statt Alias. Probleme zeigt jederzeit ein Banner an.

* **Reparieren / Erzeugen** macht die Datei gültig, entfernt Klartext-Keys und wandelt `env`-Werte in Text um. Eine
  fehlende Datei wird angelegt. Vor jeder Änderung sichert KeyZapper sie als `settings.json.keyzapper-<Zeit>.bak`.
* **Standardprofil** trägt ein Profil in `~/.claude/settings.json` ein. Claude Code nutzt es in allen Ordnern ohne eigene
  Zuordnung statt eines Klartext-Keys oder der normalen Anmeldung. Zugeordnete Projekte behalten ihr eigenes Profil.

## KeyZapper deaktivieren

Der Schalter in der Symbolleiste deaktiviert KeyZapper: Die App entfernt ihre Einträge aus allen Projekten und aus
`~/.claude/settings.json`, Claude Code nutzt dann überall die normale Anmeldung. Die Zuordnungen bleiben gespeichert und
werden beim Aktivieren wieder geschrieben.

## Verschlüsseltes Backup und Wiederherstellen

*Backup › Backup exportieren …* (Symbolleiste oder Menü *Ablage*) schreibt Profile, Projektzuordnungen und Keys in eine
`.kzbackup`-Datei, verschlüsselt mit einem Passwort von mindestens 12 Zeichen. *Backup importieren …* stellt sie auf
demselben oder einem anderen Rechner wieder her: Profile und Keys mit gleicher ID werden überschrieben, Projekte werden
zugeordnet, sofern ihr Ordner existiert. Das Passwort wird nirgends gespeichert; ohne Passwort ist das Backup nicht
wiederherstellbar.

Verschlüsselung: AES-256-GCM mit einem Schlüssel aus PBKDF2-HMAC-SHA256 (600.000 Runden, zufälliges Salt). Die
Dateiparameter sind authentifiziert, ein falsches Passwort oder eine veränderte Datei wird erkannt. Die Datei wird mit
den Rechten `0600` gespeichert. Mit `AllowKeyExport = false` enthält das Backup keine Keys. Anders als das automatische
OneDrive-Backup (nie Keys) entsteht dieses Backup manuell und bei Bedarf.

## Updates und Version

Beim Start prüft die App das neueste [GitHub-Release](https://github.com/bl0rb/ClaudeKeyZapper/releases); manuell geht es über
*KeyZapper › Nach Updates suchen …* oder die Versionszeile unten in der App. Ist eine neuere Version verfügbar, lädt
„Installieren“ den Installer (`.pkg` unter macOS, `.msi` unter Windows) herunter, prüft die von GitHub veröffentlichte SHA-256-Prüfsumme und die minisign-Signatur des Releases (`<Installer>.minisig`) gegen den
in die App eingebauten öffentlichen Schlüssel ([desktop/update-key.pub](desktop/update-key.pub)) und öffnet dann den System-Installer.
Ohne Prüfsumme oder gültige Signatur wird das Update abgelehnt; der signierte Dateiname muss passen, damit ein älteres signiertes Paket
nicht als neueres durchgeht.
Dafür sind Administratorrechte nötig. Pakete werden nur von `github.com` geladen. Die installierte Version steht unten in der
App und unter *KeyZapper › Über KeyZapper*.

Unter Intune schaltet `UpdateCheckEnabled = false` die Prüfung ab. Updates kommen dann über ein neues Paket in Intune.

## Bekannte Einschränkungen

* Ein Profil pro Git-Repository; Unterordner und Worktrees lassen sich nicht abweichend zuordnen.
* Startet Claude außerhalb des zugeordneten Bereichs (z. B. in einem Unterordner eines Nicht-Git-Ordners), nutzt es die
  Standard-Anmeldung des Entwicklers.
* Die IntelliJ-Integration ist noch nicht im Pilot geprüft.

## Entwicklung

```
desktop/core        Rust-Bibliothek keyzapper-core: Datenmodell, Metadaten, Key-Speicher, Helper-Logik, Claude-Einstellungen, Intune-Konfiguration, Backup
desktop/src-tauri   Tauri-App (macOS, Windows) und keyzapper-helper (apiKeyHelper für Claude Code)
desktop/ui          Web-Oberfläche (HTML, JS, CSS)
desktop/locales     Englische Übersetzungen (deutsche Quelltexte)
desktop/scripts     build-pkg.sh (macOS), build-msi.sh (Windows), l10n_check.py
```

Tests:

```bash
cd desktop && cargo test -p keyzapper-core
```

App im Entwicklungsmodus starten (benötigt Rust und Node.js):

```bash
cd desktop && npm ci && npm run dev
```

Installer lokal bauen (`desktop/dist/KeyZapper-<version>.pkg` unter macOS, `.msi` unter Windows in Git Bash):

```bash
cd desktop && VERSION=1.0.0 scripts/build-pkg.sh
```

```bash
cd desktop && VERSION=1.0.0 scripts/build-msi.sh
```

## Release

Ein Tag im Format `X.Y.Z` startet [.github/workflows/release.yml](.github/workflows/release.yml). Der Workflow führt die
Tests aus, baut `KeyZapper-X.Y.Z.pkg` (macOS) und `KeyZapper-X.Y.Z.msi` (Windows), signiert beide mit minisign und veröffentlicht sie
mit ihren `.minisig`-Dateien in einem GitHub-Release. Zum Signieren braucht es das Repository-Secret `MINISIGN_SECRET_KEY` (der geheime
Schlüssel zu `desktop/update-key.pub`); ohne Secret bricht der Workflow ab, statt unsignierte Installer zu veröffentlichen.
Ein Tag wie `X.Y.Z-beta.N` erzeugt ein Pre-Release, das nur installierten Betas und über „Beta-Version suchen“ angeboten wird.

```bash
git tag 1.0.0
```

```bash
git push origin 1.0.0
```

## Lizenz

[MIT](LICENSE) © 2026 bl0rb
