# DualBeam

Schlanker Dual-Pane-Dateimanager für macOS im Stil von **Commander One** /
**ForkLift** — gebaut mit **Tauri 2**, **Rust** und **TypeScript**.

## Lizenz

[MIT](LICENSE) — Copyright © 2026 N.J.

> Status: **Planung / Iteration 0** — Spezifikation liegt vor, Implementierung
> beginnt mit Iteration 1.

## Features (geplant)

- Zwei Panes nebeneinander, tastatur-zentriert
- Drag & Drop mit Modifier-Umschaltung (Kopieren / Verschieben / Alias)
- **Multi-Rename-Tool** (Suchen/Ersetzen, Nummerierung, Datum, Case, …)
- **Duplizieren** im selben Verzeichnis (`⌘D`)
- Copy-/Move-/Trash-Jobs mit Progress, Pause, Abbruch
- QuickLook-Vorschau (`F3`), Sidebar mit Volumes & Favoriten
- Auto-Refresh via FSEvents

## Doku

- [docs/SPEC.md](docs/SPEC.md) — vollständige Spezifikation

## Stack

| Schicht   | Technologie                          |
|-----------|--------------------------------------|
| Shell     | Tauri 2 (Rust)                       |
| Backend   | Rust + Tokio, `fs_extra`, `notify`, `trash`, `xattr` |
| UI        | TypeScript + Vite (UI-Framework tbd) |
| Build     | `pnpm` + `cargo` + `tauri build`     |

## Voraussetzungen

- macOS 12+
- Xcode Command Line Tools
- Rust (stable) via `rustup`
- Node 20+ und `pnpm`

## Lizenz

tbd
