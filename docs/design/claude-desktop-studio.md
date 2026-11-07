# Claude-Auftrag: Studio-Design für ClaudeKeyZapper

Stand: 7. November 2026 · Umsetzungsvorgabe für dieses Repository

## Auftrag und Umfang

Setze die Desktop-Oberfläche dieses Repositories nach der gemeinsamen Designrichtung **Studio – modern & markant** um. Ziel ist eine hochwertige Produktfamilie aus Weave, OpenVizPilot, KeyZapper, OpenReleasePilot und OpenSitePilot: gleiche gestalterische Regeln, aber aufgabengerechte Ansichten pro App. Arbeite ausschließlich im unten genannten Repository und erhalte dessen vollständigen aktuellen Funktionsumfang.

Dies ist ein konkreter UI-Implementierungsauftrag, keine Aufforderung zu einem weiteren Konzeptpapier. Lies zuerst die geltenden Repository-Anweisungen, prüfe Branch und Arbeitsbaum und erhalte fremde Änderungen. Verwende das vorhandene Tauri-2-/HTML-/CSS-/JavaScript-System. Kein Frameworkwechsel, keine neue Build-Kette, kein gemeinsames Monorepo und kein neues UI-Paket. Gemeinsame Regeln werden lokal umgesetzt; keine Imports aus Geschwister-Repositories. Backend- oder native Änderungen nur, soweit sie für die konkrete Darstellung/Fenstergröße erforderlich sind.

Die Aufgabe endet mit lokal geprüfter Umsetzung und einem knappen Ergebnisbericht. Commit, Push, PR-Veröffentlichung, Release, Installation oder produktive externe Aktionen sind durch diese Anleitung nicht zusätzlich beauftragt. Bereits separat erteilte Freigaben gelten weiter.

## Visuelle Referenz und ihre Grenzen

Zusätzliche lokale Referenz: [Klickbarer Studio-Entwurf](/Users/matze/.codex/visualizations/2026/11/07/01a1b59c-f00f-78b2-99cc-17498d931bc5/desktop-studio.html). Darin die unten genannte App auswählen und beide Erscheinungsbilder sowie die passenden Ansichten ansehen. Die Datei ist ein HTML-Fragment für die Chat-Vorschau, keine startfertige Anwendung. Die Spezifikation in dieser Markdown-Datei ist auch ohne Zugriff auf die Vorschau vollständig genug zur Umsetzung.

**Übernimm die Gestaltung, nicht die Demo-Implementierung.** Insbesondere:

- Die obere App-Auswahl, „Desktop Studio“, „Designkonzept“, Beschreibungen außerhalb des Fensters und der Vorschau-Schalter Hell/Dunkel gehören nicht in die App. Keine neue App-übergreifende Navigation bauen.
- Gezeichnete macOS-Fensterknöpfe sind Vorschau-Chrome. In der App native Titelleiste/Buttons und vorhandene Drag-Regionen respektieren; keine zweite Titelleiste oder nachgebauten Traffic Lights hinzufügen.
- Beispielnamen, Fragen, Antworten, Versionen, Budgetwerte, URLs, Konten, Statuswerte, Quellen und Freigaben dürfen nicht fest in Produktansichten übernommen werden. Echte Daten und bestehende Zustandsmodelle verwenden.
- Die kleinen Mockup-Dialoge vereinfachen den Funktionsumfang. Aktuelle Felder, Optionen, Validierungen, Aktionen, Plattformunterschiede und verwaltete Vorgaben vollständig erhalten, auch wenn sie in der Vorschau fehlen.
- Keine Simulation, Timer-Erfolgsmeldung, optimistische „fertig“-Markierung oder lokale Demo-Zustandsmaschine anstelle realer Rückmeldungen übernehmen. Kein externes Icon-CDN, keine Vorschau-Hilfen wie `Tweak`/`window.openai`, keine Aufweichung von CSP/Capabilities.
- Der aktuelle Produktcode und dokumentierte Verträge bestimmen Funktionen und Berechtigungen. Die Referenz bestimmt Hierarchie, Abstände, Flächen, Typografie und Gewichtung. Bei einem Konflikt die Gestaltung an den echten Funktionsumfang anpassen.

## Gemeinsame Designvorgaben

### Flächen und Farben

Semantische CSS-Variablen verwenden und vorhandene Tokens weiterentwickeln. Die Namen unten sind Rollen, keine Pflicht zum Umbenennen bestehender Variablen. Systemdarstellung bzw. vorhandene Theme-Einstellung beibehalten; beide Themes gleichwertig gestalten. Keine zusätzliche Theme-Einstellung nur deshalb einführen, weil die Vorschau einen Umschalter enthält.

| Rolle | Hell | Dunkel |
|---|---|---|
| Hauptfläche | `#fcfcfe` | `#17191f` |
| Sidebar / zurückgesetzte Fläche | `#f3f4f8` | `#111319` |
| Karten / Eingaben / Dialoge | `#ffffff` | `#20232c` |
| Primärer Text | `#1b2032` | `#f0f2f9` |
| Sekundärer Text | `#656b7c` | `#a3aabc` |
| Trennlinien | `#e3e6ee` | `#303440` |
| Hover / neutrale Hervorhebung | `#eceef5` | `#292d39` |
| Erfolg | `#18785b` | `#6bd4a7` |
| Hinweis / Aufmerksamkeit | `#9c5d05` | `#edbf6b` |
| Fehler | `#b53b47` | `#f09096` |

App-Akzent siehe unten. Aktive Navigation und dezente Hervorhebungen erhalten etwa 10 % Akzent auf der jeweiligen Fläche. Primärbuttons nutzen den vollen Akzent, mit weißer Beschriftung im hellen Theme und sehr dunkler Beschriftung auf dem helleren Dunkelmodus-Akzent; Kontrast am tatsächlichen Hintergrund prüfen. Farben ergänzen Text und Symbole, ersetzen sie nicht. Keine großflächigen dekorativen Verläufe oder neue Markenlogos; vorhandene Produktassets weiterverwenden.

### Typografie und Geometrie

- Systemschrift: `-apple-system`, `BlinkMacSystemFont`, `Segoe UI`, `system-ui`, `sans-serif`; keine Webfont-Abhängigkeit.
- Fließtext 14 px, längere Antworten 14–15 px mit etwa 1,65 Zeilenhöhe. Sekundärtext 12 px; 11 px nur für wenig wichtige Metadaten, nicht für zentrale Aktionen oder Fehlermeldungen.
- Seitentitel 26–29 px, Gewicht etwa 650–700, dezentes negatives Letterspacing; Abschnittstitel 18 px, Elementüberschriften 14 px/600–650. Keine große Marketingüberschrift zulasten von Inhalt; Seitentitel bleiben fachlich verständlich.
- Abstandsraster 4/8/12/16/24/32 px. Inhaltsränder meist 24–28 px, auf schmalen Fenstern 16 px. Verwandte Elemente enger gruppieren als unabhängige Abschnitte.
- Buttons und Eingaben Radius 8 px; Karten 12 px; Composer und Dialoge 12–16 px. Fensterrahmen bleiben nativ. Schatten sparsam für schwebende Flächen und Dialoge, keine Schattenkachel um jeden Inhalt.
- Reguläre Controls ungefähr 36 px hoch; verständliche Beschriftungen, ausreichende Klickflächen und sichtbarer Tastaturfokus. Wichtige Aktionen nicht ausschließlich als unbeschriftete Icons anbieten.
- Eine dominante Primäraktion pro Ansicht. Nebenaktionen als neutrale Buttons oder beschriftetes Menü; destruktive Aktionen eindeutig unterscheiden. Kontextaktionen beim betroffenen Objekt halten.
- Tabellen mit ruhigen horizontalen Trennlinien und klaren Zeilen; keine unnötigen Dashboard-Kennzahlen. Lange Namen und Pfade dürfen Aktionen nicht verdrängen.

### Verhalten, Zugänglichkeit und Qualität

Alle bestehenden Zustände gestalten: Einrichtung, Anmeldung/Abmeldung, leer, lädt, erfolgreich, fehlgeschlagen, abgebrochen, gesperrt/verwaltet und veraltete/unbekannte Daten. Bei deaktivierten wichtigen Aktionen den Grund zugänglich erklären. Wesentliche Information nicht in reine Hover-Tooltips verschieben.

Native Controls, sinnvolle Labels, Fokusreihenfolge, Escape und Fokus-Rückgabe bei Dialogen erhalten. Statuswechsel angemessen ankündigen; nicht jedes gestreamte Token vorlesen. Reduzierte Bewegung respektieren. Bestehende Sprachen und Übersetzungsmechanismen vollständig mitziehen.

Inhalt bleibt bei Fensteränderungen stabil: keine verlorene Eingabe, springende Textauswahl, gestohlener Fokus, neu startende Animation oder verlorene Scrollposition durch unnötigen DOM-Neuaufbau. Gestreamte Antworten und Logausgaben dürfen die Oberfläche nicht blockieren. Desktop-Fenster bei Standard- und Mindestgröße sowie vergrößerter Schrift prüfen; nicht das Vorschau-Layout starr skalieren.

## ClaudeKeyZapper: konkrete Umsetzung

**Zielrepo:** `ClaudeKeyZapper`, aktuelle Desktop-Oberfläche in `desktop/ui/`. Produktname in der UI bleibt **KeyZapper**. **Referenzansichten:** KeyZapper → Profile, Projekte, weitere bestehende Seiten und Dialoge. **Akzent:** hell `#6551dd`, dunkel `#b2a0ff`. Vorhandenes Markenasset `Resources/AppIcon.svg` bzw. dessen bereits verwendete Ableitung weiterverwenden.

### Einstieg und betroffene Dateien

| Zuständigkeit | Datei |
|---|---|
| Aktuelles Verhalten und Regeln | [README](../../README.md), [AGENTS](../../AGENTS.md) |
| Navigation, Karten, Projekte, Dialoge | [app.js](../../desktop/ui/app.js), [index.html](../../desktop/ui/index.html) |
| Gemeinsame Darstellung | [style.css](../../desktop/ui/style.css) |
| Separates Nutzungsfenster | [usage.html](../../desktop/ui/usage.html) |
| Englische Übersetzung / Prüfer | [en.json](../../desktop/locales/en.json), [l10n_check.py](../../desktop/scripts/l10n_check.py) |
| Natives Verhalten und Tests | [Desktop-Workspace](../../desktop/Cargo.toml), `desktop/core/`, `desktop/src-tauri/` |

1. **App-Rahmen:** bestehende Sidebar mit „Profile“, „Projekte“, „MCP-Server“, „Claude-Tools“, „Claude-Einstellungen“ und „Backup“ übernehmen, einschließlich policyabhängiger Sichtbarkeit. Aktivierung, Updates und sonstige aktuelle Fußzeilenaktionen weiterhin erreichbar halten; nicht durch die vereinfachte Mockup-Fußzeile ersetzen. Breite grob 180–200 px, deutlicher aktiver Eintrag, kompakte Nebenaktionen. Auf schmalen unterstützten Fenstern keine Labels oder Aktionen abschneiden.
2. **Profile:** eine zusammengehörige Karte pro Zugang. Reihenfolge: Name und Zugangsart → echter Verbindungs-/Anmeldezustand → passende Zugangsinformationen und tatsächliches Budget/Nutzung → zugeordnete Projekte. Fehlende Budgets nicht durch 0 oder Demo-Zahlen ersetzen. Vorhandene Account-/Office-/Modell-/Reset-/Poolinformationen erhalten, wo sie zur jeweiligen Zugangsart gehören. „Neues Profil“ als Primäraktion, „Projekt zuordnen“ klar sichtbar als Nebenaktion.
3. **Projekte:** Zuordnung und Zustandsprobleme direkt an der Projektzeile zeigen. Lange Ordnerpfade sekundär darstellen und zugänglich vollständig lesbar halten. Reparatur, Zuordnung, Entfernen und bestehende Projektaktionen bleiben erreichbar. Nach einer Änderung tatsächlichen Backend-Zustand neu lesen; kein verschwundener Fehler nur wegen Klick auf „Reparieren“.
4. **Drei Zugangsarten:** LiteLLM-/Gateway-Schlüssel, SSO und Claude-Abonnement visuell gleichwertig, fachlich unterscheidbar. Keine identischen Pflichtfelder für alle drei. Bei SSO keine Schlüsselkopie oder Poolaktionen vortäuschen. Das Abonnementprofil nicht als API-Schlüsselprofil darstellen. Bestehende Prüfungen und Profil-Menüs vollständig erhalten.
5. **Weitere Seiten:** MCP-Server, Claude-Tools, Claude-Einstellungen und Backup sowie alle vorhandenen Unterdialoge mit denselben Controls, Flächen, Tabellen-/Zeilenregeln und Zustandsmustern gestalten. Der Mockup enthält dort nur repräsentative Ausschnitte; keine vorhandenen Funktionen entfernen oder durch statische „Öffnen“-Platzhalter ersetzen. Kataloge, Projektzuordnungen, Import-/Exportoptionen und Verwaltungsbeschränkungen aus dem aktuellen Code übernehmen.
6. **Dialoge:** klare Titel, korrekte Felder je Profiltyp, verständliche Validierungsfehler und eindeutige Aktion. Bestehende Lösch-/Import-/Export-/Ersetzen-Bestätigungen erhalten. Eingabe und Cursor bei Live-Aktualisierungen stabil halten. Änderungen an Profilen oder Projektbindungen müssen die vorhandenen Hinweise zum Neustart einer Claude-Sitzung weiter vermitteln.
7. **Zusätzliche Fenster / Tray:** `usage.html`, Tray-Aktionen und bestehende native Fensterbehandlung mit berücksichtigen. Gemeinsames CSS darf das Nutzungsfenster nicht versehentlich im Hauptfensterlayout darstellen.

### Unverhandelbare Funktionsgrenzen

- Credentials bleiben in vorhandenen sicheren Speichern und Rust-/Core-Abläufen. Keine echten Schlüssel in UI-Demos, Logs, Screenshots, Fixtures oder JavaScript-Persistenz. In Ansichten nur vorhandene sichere Maskierung verwenden.
- Profilzuordnung, Credential-Routing, Pool-/Budgetregeln, verwaltete Werte, erlaubte Hosts und vorhandene Sperren bleiben unverändert. Fehler bei einem Zugang dürfen keinen stillen Fallback auf einen anderen auslösen.
- German-source `t(...)` und gegebenenfalls Rust-`l!(...)` weiter nutzen; englische Übersetzungen in `desktop/locales/en.json` und `%@`-Platzhalter konsistent halten. Nicht einfach Mockup-Texte ohne Übersetzung einfügen.
- Kein echter Keychain-Umbau, Login, Profilwechsel für reale Arbeitsprojekte, Import oder Export privater Konfiguration als gewöhnlicher UI-Test. Testzustände synthetisch und isoliert anlegen.
- Benutzer-, IT- und Zugangsart-Beschränkungen dürfen durch neue Menüs, vereinfachte Dialoge oder ausgeblendete Detailbereiche nicht umgangen werden.

### Prüfen und abnehmen

Vorhandenes Setup verwenden. Vom Repository-Root:

```sh
node --input-type=module --check < desktop/ui/app.js
```

Bei geänderten Texten den bestehenden Lokalisierungscheck aus `desktop/` ausführen:

```sh
python3 scripts/l10n_check.py
```

Die App aus `desktop/` mit `npm run dev` starten. Nur wenn Core-/Rust-Verhalten betroffen ist, passende Tests aus `desktop/` verwenden, z. B. `cargo test --locked -p keyzapper-core <testname>`. Kein pauschaler Paketbau für CSS; bei notwendiger Paketvalidierung die aktuellen CI-/Build-Skripte verwenden. Kein nicht vorhandenes `npm test`-/Lint-/Typecheck-Skript erfinden.

Abnahme: alle aktuellen Navigationsbereiche und relevante Dialoge in Hell/Dunkel, Standard-/Mindestfenstergröße aus aktueller Tauri-Konfiguration, Deutsch/Englisch, lange Namen/Pfade, viele/keine Profile, alle drei Zugangsarten, fehlender Schlüssel/Ordner, abgelaufene Anmeldung, Budget unbekannt/erreicht, verwaltete Profile, policybedingt verborgene Funktionen, Tastatur/Modalfokus und Nutzungsfenster. macOS und Windows sind Zielplattformen. Native Checks nur für tatsächlich verfügbare Systeme behaupten.

## Arbeitsfolge und Fertigkriterien

1. Aktuellen UI-Zustand und native Fenstergrenzen anhand der genannten Dateien erfassen. Die Referenz mit realen Funktionen abgleichen; keine erneute Vollrepo-Analyse oder Roadmap anlegen.
2. Lokale Designtokens und bestehende Controls anpassen, dann die spezifischen Hauptansichten, Nebenansichten und vollständigen Dialoge. Bestehende Daten-/Command-Schnittstellen verwenden. Falls nur die Darstellung betroffen ist, Rust unverändert lassen.
3. Relevante Zustände, Interaktionen und beide Themes prüfen. Für tatsächliche Verhaltensänderungen fokussierte Regressionstests hinzufügen; keine Tests, die nur CSS-Werte oder diese Anleitung nacherzählen. Syntaxprüfung und Mockup-Screenshots sind kein Nachweis nativer Funktion.
4. Den finalen Diff auf unbeabsichtigte Funktionsverluste und abgeschwächte Guards prüfen. Nur wenn die Umsetzung dokumentiertes Verhalten verändert, bestehende passende Dokumentation aktualisieren.
5. Knapp berichten: umgesetzte Ansichten, wichtige Dateien, tatsächlich geprüfte Plattformen/Interaktionen, relevante Screenshots und verbleibende Blocker. Ungeprüfte native Integration ausdrücklich von geprüftem Browser-Rendering unterscheiden. Keine Umsetzung als fertig melden, wenn wichtige vorhandene Dialoge oder Zustände nur Platzhalter sind.

**Fertig ist die Aufgabe, wenn** die betroffenen vollständigen Oberflächen erkennbar der Studio-Familie entsprechen, alle bisherigen Funktionen und Schutzmechanismen erhalten sind, zentrale Abläufe und Fehlerzustände geprüft wurden und die Umsetzung mit einem fokussierten Diff reviewbar ist. Danach stoppen; kein anschließendes Refactoring oder weiterer Produktumfang.
