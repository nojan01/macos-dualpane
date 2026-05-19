# macOS Dual-Pane File Manager — Spezifikation

**Projektname (Arbeitstitel):** `dualbeam`
**Stack:** Tauri 2 · Rust · TypeScript · Vite
**Zielplattform:** macOS 12+ (Apple Silicon & Intel)
**Vorbilder:** Commander One, ForkLift, Total Commander

---

## 1. Ziel & Scope

Ein schlanker, tastatur-freundlicher Dateimanager mit **zwei Panes nebeneinander**.
Fokus auf die *wesentlichen* Funktionen, die im täglichen Workflow oft fehlen oder im
Finder umständlich sind:

- Schnelle Navigation per Tastatur
- Datei-Operationen **immer von aktivem → inaktivem Pane**
- Drag & Drop mit **Umschaltung Kopieren / Verschieben**
- **Massen-Umbenennen** (Multi-Rename-Tool wie in ForkLift)
- **Duplizieren** von Dateien/Ordnern im selben Verzeichnis (wie in ForkLift)

**Nicht im Scope (vorerst):**
FTP/SFTP/S3-Anbindung, Cloud-Sync, Archiv-Browser, Editor, Git-Integration,
Themen-Engine.

---

## 2. UI-Layout

```
┌─────────────────────────────────────────────────────────────┐
│  Toolbar:  ← →  ↑   ⟳   ★Favoriten   🔍Filter   ⚙          │
├──────────────────────────────┬──────────────────────────────┤
│ Pfad: /Users/nojan/Docs      │ Pfad: /Volumes/Backup        │
├──────────────────────────────┼──────────────────────────────┤
│ ↑ ..                          │ ↑ ..                          │
│ 📁 Projekte           — 14.05 │ 📁 2026-05            — 12.05 │
│ 📁 Bilder             — 02.04 │ 📄 archive.zip  1.2G   10.05 │
│ 📄 notes.md     12 K   16.05 │ 📄 readme.txt    1 K    09.05 │
│ 📄 todo.txt      2 K   17.05 │                                │
│ ▸ aktives Pane (blau)        │                                │
├──────────────────────────────┴──────────────────────────────┤
│ Sidebar (links optional): Favoriten · Volumes · Zuletzt     │
├─────────────────────────────────────────────────────────────┤
│ Statusleiste: 3 Dateien · 14 KB ausgewählt   |   Job: 45%   │
├─────────────────────────────────────────────────────────────┤
│ F3 View  F5 Copy  F6 Move  F7 NewDir  F8 Delete  F2 Rename  │
└─────────────────────────────────────────────────────────────┘
```

- Aktives Pane farblich hervorgehoben (Border + Header)
- Sortier-Indikator in den Spalten-Headern
- Spalten: **Name · Größe · Geändert · Art** (konfigurierbar)
- Schriftgröße & Zeilenhöhe einstellbar

---

## 3. Tastatur-Bedienung

| Taste                | Aktion                                      |
|----------------------|---------------------------------------------|
| `Tab`                | Pane wechseln                               |
| `↑` / `↓`            | Cursor bewegen                              |
| `PgUp` / `PgDn`      | seitenweise                                 |
| `Home` / `End`       | Anfang / Ende                               |
| `Enter`              | Ordner betreten / Datei mit Default öffnen  |
| `Backspace` / `⌘↑`   | eine Ebene hoch                             |
| `Space`              | Markierung toggle + Cursor runter           |
| `⌘A`                 | alles markieren                             |
| `Esc`                | Markierung aufheben / Dialog schließen      |
| `F2`                 | Umbenennen (Inline)                         |
| `F3`                 | QuickLook-Vorschau                          |
| `F5`                 | Kopieren → Ziel-Pane                        |
| `⌘D` / `F5` mit Alt  | Duplizieren im aktuellen Verzeichnis        |
| `F6`                 | Verschieben → Ziel-Pane                     |
| `F7`                 | Neuer Ordner                                |
| `F8` / `⌘⌫`          | In den Papierkorb                           |
| `⇧F8`                | Endgültig löschen (mit Bestätigung)         |
| `⌘F`                 | Filter im aktiven Pane                      |
| `⌘L`                 | Pfad-Eingabe (Go-to)                        |
| `⌘R`                 | Multi-Rename-Tool öffnen                    |
| `⌘1/⌘2/⌘3`           | Sortierung Name/Größe/Datum                 |
| `⌘.`                 | Versteckte Dateien anzeigen toggle          |
| `⌘\`                 | Beide Panes auf gleichen Pfad               |
| `⌘O`                 | Anderes Pane auf Pfad des aktiven setzen    |

---

## 3a. Maus-Bedienung

Die App ist **gleichwertig per Maus und Tastatur** bedienbar.

### Einfach-Klick
- **Linksklick auf Eintrag** → Cursor setzen, Auswahl = nur dieses Item;
  setzt zugleich das geklickte Pane als **aktiv**
- **Linksklick auf leeren Bereich** → Auswahl aufheben (Pane bleibt aktiv)
- **Linksklick auf Pane-Header / Pfad-Leiste** → Pane aktivieren

### Mehrfach-Auswahl
- **`⌘`-Klick** → Item zur Auswahl hinzufügen / entfernen (toggle)
- **`⇧`-Klick** → Bereich vom letzten Anker bis hier auswählen
- **Lasso (Drag im leeren Bereich)** → Rechteck-Selektion

### Doppelklick
- Auf **Ordner** → betreten
- Auf **Datei** → mit Default-App öffnen (`open_default`)
- Auf **`..`** → eine Ebene hoch

### Rechtsklick (Kontextmenü)
- Öffnen / Öffnen mit … (Phase 2)
- Vorschau (QuickLook)
- Umbenennen (F2) · Duplizieren (⌘D) · Multi-Rename (⌘R)
- Kopieren nach (Ziel-Pane) · Verschieben nach (Ziel-Pane)
- In Papierkorb (F8) · Endgültig löschen (⇧F8)
- Im Finder zeigen · Pfad kopieren
- Bei Klick auf leeren Bereich: Neuer Ordner (F7) · Einfügen · Sortierung
- Bei Klick auf Spalten-Header: Sortierfeld umschalten, Spalten ein-/ausblenden (Phase 2)

### Pfad-Leiste / Breadcrumb
- **Klick auf Segment** → dorthin springen
- **`⌘L` oder Klick auf freien Bereich der Pfadleiste** → Pfad-Eingabefeld

### Maus-Rad
- Vertikal: scrollen
- `⌘` + Rad: Zeilenhöhe / Zoom (Phase 2)

### Hover
- Zeile dezent hervorheben
- Tooltip mit vollem Namen bei abgeschnittenen Einträgen

### Drag mit Maus
- Siehe §4 (Drag & Drop) — funktioniert sowohl Pane → Pane als auch
  Pane → Sidebar (ab Phase 6) und Pane ↔ Finder

### Edge-Cases
- Doppelklick auf bereits selektiertes Item öffnet, ohne Markierung zu ändern
- Beim Beginn eines Drags wird das geklickte Item (falls nicht in Selektion)
  exklusiv selektiert
- Auto-Scroll bei Lasso/Drag nahe Pane-Rand

---

## 4. Drag & Drop

### Verhalten
- **Quelle:** Markierte Dateien des aktiven (oder Quell-)Panes
- **Ziel:** anderes Pane (Hintergrund oder konkreter Ordner) **oder** Ordner im selben Pane
- **Standard-Aktion:**
  - Ziel auf **gleichem Volume** → **Verschieben**
  - Ziel auf **anderem Volume** → **Kopieren**
- **Modifier-Tasten (live umschaltbar während des Drags):**

| Modifier         | Aktion         | Cursor-Indikator |
|------------------|----------------|------------------|
| (keiner)         | Auto (s.o.)    | ↕ / ➜            |
| `⌥` (Option)     | **Kopieren**   | ➕                |
| `⌘`              | **Verschieben**| ➜                |
| `⌃` (Control)    | **Alias / Symlink** anlegen | ⤴ |

- Visuelles Feedback: Drop-Ziel hervorheben, Cursor-Badge anpassen
- Während Drag: Tooltip „N Dateien · 124 MB · Kopieren nach …"
- Konflikt (Datei existiert) → **Überschreiben-Dialog** (s. §7)

### Drag aus / in Finder
- **Drop aus Finder ins Fenster:** wird wie internes Drag behandelt
- **Drag aus Pane in Finder:** Standard-OS-Verhalten (Kopieren)

---

## 5. Massen-Umbenennen (Multi-Rename-Tool)

Aufruf: `⌘R` mit Markierung. Eigenes Dialog-Fenster.

### Bausteine (alle kombinierbar, in Reihenfolge):

1. **Suchen & Ersetzen**
   - Literal oder Regex (mit Capture-Groups `$1`, `$2`)
   - Case-sensitive / nur Dateiname / nur Endung / beides
2. **Nummerierung**
   - Startwert, Schrittweite, Stellen (z. B. `001`, `002`)
   - Position: Prefix, Suffix, vor/nach Match
3. **Datum/Zeit einfügen**
   - aus mtime / heute / EXIF (Phase 2)
   - Format frei: `YYYY-MM-DD`, `YYYYMMDD_HHmm` …
4. **Groß-/Kleinschreibung**
   - UPPER · lower · Title Case · Erstes groß
5. **Trimmen / Padding**
   - N Zeichen von vorn/hinten abschneiden
   - mit Zeichen auffüllen
6. **Endung ändern**
   - Setzen, anhängen, entfernen

### Live-Vorschau
- Tabelle mit Spalten **Alt → Neu**
- Konflikte (doppelte Zielnamen, ungültige Zeichen, Überschreibung)
  rot markiert
- Button **Anwenden** nur aktiv wenn konfliktfrei
- **Undo** der letzten Massen-Aktion (in-memory History, Session-scoped)

### Profile
- Benannte Regelsätze speichern/laden (JSON in App-Config)

---

## 6. Duplizieren

Aufruf: `⌘D` (oder Menü) auf Markierung.
- Erzeugt Kopie **im selben Verzeichnis**
- Namensschema (konfigurierbar, Standard wie macOS):
  - `foo.txt` → `foo copy.txt` → `foo copy 2.txt`
  - Alternativ: `foo (1).txt`, `foo_2026-05-17.txt`
- Bei Markierung mehrerer Items: alle dupliziert mit demselben Schema
- Ordner werden rekursiv kopiert (mit Progress)

---

## 7. Datei-Operationen — Verhalten

### Konflikt-Dialog (Copy/Move/Duplicate)
- Optionen: **Überschreiben** · **Beide behalten** (auto-rename) · **Überspringen** · **Abbrechen**
- Checkbox „Für alle weiteren anwenden"
- Bei Ordnern: rekursives Mergen optional

### Progress
- Job-Liste im Footer (Mini-Indikator), Klick öffnet Job-Fenster
- Pro Job: Fortschritt, aktuelle Datei, Geschwindigkeit, ETA
- **Pausieren / Abbrechen**
- Mehrere Jobs parallel möglich (eigene Tokio-Tasks)

### Löschen
- Standard: **in Papierkorb** (`trash` crate)
- `⇧F8`: endgültig, mit Bestätigung
- Geschützte Pfade (`/`, `~`) abfangen

### Auto-Refresh
- `notify` (FSEvents) abonniert sichtbare Pfade
- Throttled Refresh (max. 10/s) damit UI ruhig bleibt

---

## 8. Sidebar (Phase 2)

- **Favoriten** (vom User pinnbar)
- **Volumes** (alle eingehängten, `diskutil`/`/Volumes`)
- **Zuletzt besuchte Ordner** (max. 20)
- Drop auf Eintrag = Kopier-/Verschiebe-Ziel

---

## 9. Architektur

### Backend (Rust, `src-tauri`)

```
src-tauri/src/
├── main.rs
├── lib.rs                  // Tauri builder, command-Registrierung
├── commands/
│   ├── fs_list.rs          // list_dir, get_volumes
│   ├── fs_ops.rs           // copy, move, duplicate, mkdir, rename, trash
│   ├── rename_batch.rs     // Multi-Rename Engine
│   ├── preview.rs          // QuickLook-Bridge
│   └── watch.rs            // notify-Subscriptions
├── jobs/
│   ├── manager.rs          // JobId, State, Progress-Channel
│   └── progress.rs         // Tauri-Events emit
└── platform/
    └── macos.rs            // NSWorkspace icon, mdfind, xattr
```

**Wichtige Crates:**
```toml
tauri          = "2"
tokio          = { version = "1", features = ["full"] }
serde          = { version = "1", features = ["derive"] }
walkdir        = "2"
fs_extra       = "1"          # Copy mit Progress-Callback
notify         = "6"          # FSEvents Watcher
trash          = "5"          # In Papierkorb
xattr          = "1"          # Extended Attributes / Tags
regex          = "1"          # Multi-Rename
chrono         = "0.4"        # Datum-Format
mime_guess     = "2"
objc2          = "0.5"        # NSWorkspace, QuickLook
objc2-app-kit  = "0.2"
```

### Frontend (TypeScript + Vite, *kein* Framework-Zwang)

Empfehlung: **Solid.js** oder **Svelte** (kompakt, schnell, gut für virtualisierte Listen).
Fallback: Vanilla TS mit kleinen Komponenten.

```
src/
├── main.ts
├── state/
│   ├── pane.ts             // Pane-State, Selection, Cursor
│   └── app.ts              // active pane, settings
├── components/
│   ├── Pane.tsx            // virtualisierte Datei-Liste
│   ├── Toolbar.tsx
│   ├── Statusbar.tsx
│   ├── PathBar.tsx
│   ├── RenameDialog.tsx    // Multi-Rename UI
│   ├── ConflictDialog.tsx
│   └── JobPanel.tsx
├── ipc/
│   └── commands.ts         // Wrapper um invoke()
├── keymap.ts
└── styles.css
```

**Wichtige Libs:**
- `@tanstack/virtual` — Liste mit 100k+ Einträgen flüssig
- `@tauri-apps/api` — invoke, event, dialog
- evtl. `solid-js` für reaktive UI

---

## 10. Daten-Modelle (TS ⇄ Rust)

```ts
type Entry = {
  name: string;
  path: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;          // Bytes (für Ordner: 0 oder lazy)
  mtime: number;         // Unix ms
  ext: string;           // ohne Punkt, lowercase
  hidden: boolean;
};

type Volume = {
  name: string;
  path: string;          // /Volumes/Foo oder /
  totalBytes: number;
  freeBytes: number;
  ejectable: boolean;
};

type JobKind = 'copy' | 'move' | 'duplicate' | 'trash' | 'delete';

type JobProgress = {
  id: string;
  kind: JobKind;
  totalBytes: number;
  doneBytes: number;
  currentFile: string;
  filesDone: number;
  filesTotal: number;
  state: 'running' | 'paused' | 'done' | 'error' | 'cancelled';
  errorMsg?: string;
};

type RenameRule =
  | { kind: 'replace'; pattern: string; replacement: string; regex: boolean; target: 'name'|'ext'|'full' }
  | { kind: 'number'; start: number; step: number; pad: number; position: 'prefix'|'suffix' }
  | { kind: 'case'; mode: 'upper'|'lower'|'title' }
  | { kind: 'date'; source: 'mtime'|'now'; format: string; position: 'prefix'|'suffix' }
  | { kind: 'trim'; from: 'start'|'end'; count: number }
  | { kind: 'ext'; mode: 'set'|'add'|'remove'; value?: string };
```

---

## 11. Tauri-Commands (API)

```rust
#[tauri::command] fn list_dir(path: String, show_hidden: bool) -> Result<Vec<Entry>>;
#[tauri::command] fn get_volumes() -> Result<Vec<Volume>>;
#[tauri::command] fn open_default(path: String) -> Result<()>;
#[tauri::command] fn quicklook(path: String) -> Result<()>;

#[tauri::command] fn make_dir(parent: String, name: String) -> Result<String>;
#[tauri::command] fn rename(path: String, new_name: String) -> Result<String>;
#[tauri::command] fn trash(paths: Vec<String>) -> Result<()>;
#[tauri::command] fn delete_permanent(paths: Vec<String>) -> Result<()>;

#[tauri::command] fn copy_items(sources: Vec<String>, dest_dir: String,
                                on_conflict: ConflictPolicy) -> Result<String /*JobId*/>;
#[tauri::command] fn move_items(sources: Vec<String>, dest_dir: String,
                                on_conflict: ConflictPolicy) -> Result<String>;
#[tauri::command] fn duplicate_items(paths: Vec<String>,
                                     scheme: DupScheme) -> Result<String>;

#[tauri::command] fn job_cancel(id: String) -> Result<()>;
#[tauri::command] fn job_pause(id: String, paused: bool) -> Result<()>;

#[tauri::command] fn rename_preview(paths: Vec<String>, rules: Vec<RenameRule>)
                                    -> Result<Vec<(String, String)>>;
#[tauri::command] fn rename_apply(paths: Vec<String>, rules: Vec<RenameRule>)
                                  -> Result<()>;

#[tauri::command] fn watch_start(pane: String, path: String) -> Result<()>;
#[tauri::command] fn watch_stop(pane: String) -> Result<()>;
```

**Events (Backend → Frontend):**
- `job://progress` → `JobProgress`
- `fs://changed` → `{ pane, path }`
- `conflict://ask` → `{ jobId, src, dst }` (async resolved durch Frontend)

---

## 12. Sicherheit / macOS-Permissions

- `Info.plist` (in `tauri.conf.json` bundle):
  - `NSDesktopFolderUsageDescription`
  - `NSDocumentsFolderUsageDescription`
  - `NSDownloadsFolderUsageDescription`
  - `NSRemovableVolumesUsageDescription`
  - `NSNetworkVolumesUsageDescription`
- Beim ersten Zugriff auf geschützte Ordner zeigt macOS automatisch den TCC-Dialog
- Kein Sandboxing zunächst (keine Mac-App-Store-Distribution geplant)
- Code-Signing + Notarization über `tauri build` wie im bestehenden Backup-Projekt

---

## 13. Settings (persistiert in `~/Library/Application Support/dualbeam/config.json`)

```json
{
  "panes": {
    "left":  { "lastPath": "~", "sort": "name", "sortDir": "asc" },
    "right": { "lastPath": "~/Downloads", "sort": "mtime", "sortDir": "desc" }
  },
  "showHidden": false,
  "confirmTrash": false,
  "confirmDelete": true,
  "duplicateScheme": "macos",   // "macos" | "paren" | "date"
  "favorites": ["/Users/nojan/Projekte"],
  "renameProfiles": [ ... ]
}
```

---

## 14. Roadmap (Iterationen)

| # | Inhalt | Liefert |
|---|--------|---------|
| **1** | Projekt-Setup, 2 Panes, Navigation, Sortierung, Tab-Wechsel, Keyboard-Selection, `list_dir`, `open_default` | Lauffähig: schauen & navigieren |
| **2** | F5/F6/F8/F7/F2 + Konflikt-Dialog + Job-Progress | Kern-Ops nutzbar |
| **3** | Drag & Drop intern + Modifier-Umschaltung | DnD funktional |
| **4** | Duplizieren (`⌘D`) + Auto-Refresh via `notify` | ForkLift-Feature #1 |
| **5** | Multi-Rename-Tool (`⌘R`) inkl. Vorschau & Profile | ForkLift-Feature #2 |
| **6** | Sidebar (Favoriten, Volumes), Filter (`⌘F`), QuickLook (`F3`) | Komfort |
| **7** | Drag in/aus Finder, Settings-Dialog, DMG-Bundling + Notarization | Release-fertig |

Jede Iteration ist eigenständig nutzbar und endet mit lauffähigem Build.

---

## 15. Entscheidungen (vor Iteration 1)

- [x] **UI-Framework:** Solid.js
- [x] **Duplikat-Schema Default:** macOS-Style (`foo copy.txt`, `foo copy 2.txt`)
- [x] **Sidebar:** erst ab Iteration 6
- [x] **App-Icon:** neu (SVG-Quelle unter `src-tauri/icons/icon.svg`,
      Plattform-Sizes werden via `pnpm tauri icon` generiert)
- [x] **Maus-Bedienung:** vollwertig (siehe §3a)
- [ ] Standard-Schriftart Liste: System (SF Pro / Inter) — bei Bedarf später anpassbar
