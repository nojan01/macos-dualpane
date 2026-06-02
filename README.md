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
- Auto-Refresh via FSEvents
- Mehrsprachig (Deutsch / Englisch)

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

## Lizenz

[MIT](LICENSE) — Copyright © 2026 N.J.
