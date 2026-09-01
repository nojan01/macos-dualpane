# DualBeam

Schlanker Dual-Pane-Dateimanager für macOS im Stil von **Commander One** /
**ForkLift** — gebaut mit **Tauri 2**, **Rust** und **TypeScript**.

> Status: **Aktiv in Entwicklung** — lauffähige App, regelmäßige DMG-Builds.

## Features

- Zwei Panes nebeneinander, tastatur-zentriert
- **Native macOS-Dateisymbole** (App- & Dokumenttyp-Icons via `NSWorkspace`)
- Drag & Drop mit Modifier-Umschaltung (Kopieren / Verschieben / Alias),
  inkl. nativer Promise-Drags nach Finder
- **Multi-Rename-Tool** (Suchen/Ersetzen, Nummerierung, Datum, Case, …)
- **Duplizieren** im selben Verzeichnis (`⌘D`)
- Copy-/Move-/Trash-Jobs mit Progress, Pause, Abbruch
- **Sync-Dialog** zum Abgleich zweier Verzeichnisse
- QuickLook-Vorschau (`F3`), Sidebar mit Volumes & Favoriten
- **Netzlaufwerke** per `⌘K`: SMB, WebDAV, AFP, NFS sowie **SFTP und FTPS**
  über das mitgelieferte `rclone`
- Auto-Refresh via FSEvents
- **RDP-Verbindungen aus RemoteDeskRDP** in der Seitenleiste; ein Klick öffnet
  die Sitzung in RemoteDeskRDP (siehe unten)
- Mehrsprachig (Deutsch / Englisch)

## RDP-Verbindungen aus RemoteDeskRDP

Ist [RemoteDeskRDP](../remote-client) installiert und dort mindestens eine
Verbindung eingerichtet, zeigt die Seitenleiste unterhalb der Sync-Profile den
Abschnitt **Remote-Desktop**. Ein Klick öffnet die Sitzung dort.

**DualBeam spricht selbst kein RDP.** Es liest lediglich `id`, `name` und `host`
aus der Profildatei von RemoteDeskRDP
(`~/Library/Application Support/RemoteDesk/profiles.json`) und öffnet
`remotedesk://connect?id=<uuid>`. Kennwörter stehen nicht in dieser Datei; sie
bleiben im Schlüsselbund von RemoteDeskRDP und werden von DualBeam nie berührt.

Warum ein URL-Schema und keine Startargumente: `open -a … --args` erreicht eine
**bereits laufende** App nicht – macOS aktiviert dann nur ihr Fenster. Der
Deep-Link greift in beiden Fällen und erzeugt nie eine zweite Instanz.

Die Liste wird beim Start und bei jedem Wechsel in den Vordergrund neu gelesen.
Fehlt der Abschnitt, ist die App nicht installiert oder es ist dort noch keine
Verbindung eingerichtet. Eine beschädigte Profildatei führt zu einem leeren
Abschnitt, nicht zu einem Fehler.

## Doku

- [docs/SPEC.md](docs/SPEC.md) — vollständige Spezifikation

## Stack

| Schicht   | Technologie                          |
|-----------|--------------------------------------|
| Shell     | Tauri 2 (Rust)                       |
| Backend   | Rust + Tokio, `fs_extra`, `notify`, `trash`, `xattr` |
| UI        | TypeScript + SolidJS + Vite          |
| Build     | `npm` + `cargo` + `tauri build`      |

## Voraussetzungen

- macOS 12+
- Xcode Command Line Tools
- Rust (stable) via `rustup`
- Node 20+ und `npm`

## Entwicklung

```sh
npm install
npm run tauri:dev     # Dev-Modus
npm run tauri:build   # DMG bauen
npm test              # Tests
```

`tauri:dev` und `tauri:build` erzeugen die persönliche Variante (Cargo-Feature
`hidrive`, enthält ein vorkonfiguriertes IONOS-HiDrive-Lesezeichen). Für eine
Veröffentlichung `npm run tauri:build:public` verwenden – siehe
[docs/SIGNING.md](docs/SIGNING.md).

Für SFTP und FTPS wird `rclone` als Beiprogramm mitgeliefert. Es wird nicht im
Git verwaltet, sondern vor jedem `tauri:dev`/`tauri:build` automatisch geladen
und anhand seiner Prüfsumme überprüft. Bei Bedarf auch einzeln:

```sh
npm run rclone:fetch          # für die eigene Architektur
bash scripts/fetch-rclone.sh all   # arm64 und amd64
```

## Lizenz

DualBeam steht unter der **[MIT-Lizenz](LICENSE)** — Copyright © 2026 N.J.

Nutzung, Veränderung und Weitergabe sind privat wie geschäftlich frei erlaubt,
solange Urhebervermerk und Lizenztext erhalten bleiben.

Für die mitgelieferten Fremdkomponenten gelten deren eigene Lizenzen — allen
voran **rclone**, ebenfalls unter der MIT-Lizenz. Die vollständige Auflistung
steht in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md), die Lizenztexte
liegen dem Programm unter `DualBeam.app/Contents/Resources/licenses/` bei.
