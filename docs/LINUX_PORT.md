# Roadmap: Portierung von DualBeam auf Linux (Debian / Ubuntu)

Stand: November 2026. Dieses Dokument beschreibt, was einer Linux-Version im
Weg steht, wie der macOS-spezifische Code gekapselt werden soll und in welcher
Reihenfolge die Arbeit sinnvoll ist.

## 1. Ausgangslage

| Schicht | Zustand | Portabel? |
|---|---|---|
| UI (`src/`, SolidJS + Vite) | plattformneutral, nur `metaKey`-Shortcuts | ✅ ja, kleine Anpassungen |
| Tauri-Shell (`tauri.conf.json`, Capabilities) | Bundle-Ziel DMG, `externalBin` rclone | ✅ ja, `.deb`/AppImage ergänzen |
| Rust-Backend `lib.rs` (~9.500 Zeilen) | ca. 85 macOS-Stellen (`/Volumes`, `osascript`, `diskutil`, `qlmanage`, `open`) | 🟡 teilweise |
| `remote.rs` (~5.600 Zeilen, rclone-Mounts) | 6 macOS-Stellen, sonst neutral | ✅ weitgehend |
| `nfs.rs`, `nfs_delete.rs` | `mount_nfs`, `/Volumes` | 🟡 teilweise |
| `object_storage.rs` | Keychain über `security-framework` | 🟡 Ersatz nötig |
| `rdp.rs` | Pfad `~/Library/Application Support/RemoteDesk` | ✅ Pfad anpassen |
| `promise_drag.m` / `promise_drag.rs` | Objective-C (Cocoa, QuickLook, NSWorkspace) | 🔴 komplett nativ |
| Crates `trash`, `notify`, `xattr`-Aufrufe (`libc`) | funktionieren unter Linux (freedesktop-Trash, inotify, `getxattr`) | ✅ ja |

## 2. Architektur-Ziel: Plattform-Modul

Alles Betriebssystem-Spezifische wandert in ein Modul `src-tauri/src/platform/`:

```
platform/
  mod.rs        // pub use nach cfg(target_os)
  macos/        // heutiger Code (osascript, diskutil, promise_drag.m, …)
  linux/        // neue Implementierungen
```

`mod.rs` exportiert eine einheitliche Funktionsmenge; `lib.rs` ruft nur noch
`platform::…` auf und enthält keine `#[cfg(target_os)]`-Blöcke mehr.

Geplante Schnittstelle (Auszug):

```rust
pub fn volumes() -> Vec<VolumeInfo>;                 // heute: read_dir("/Volumes") + diskutil
pub fn mount_url(url: &Url, creds: Option<Creds>) -> Result<PathBuf, String>;
pub fn unmount(path: &Path) -> Result<(), String>;
pub fn is_network_mount(path: &Path) -> bool;
pub fn reveal_in_file_manager(path: &Path);          // heute: open -R
pub fn open_with_default(path: &Path);               // heute: open
pub fn open_with_apps(path: &Path) -> Vec<AppInfo>;  // heute: NSWorkspace
pub fn file_icon_png(path: &Path, size: u32) -> Option<Vec<u8>>;
pub fn quick_look(paths: &[PathBuf]);                // heute: qlmanage / QLPreviewPanel
pub fn thumbnail(path: &Path, size: u32) -> Option<PathBuf>;
pub fn delete_privileged(paths: &[PathBuf]) -> Result<(), String>;
pub fn secret_set/get/delete(service, account, value);
pub fn start_promise_drag(...) -> Result<(), String>; // Linux: Option/No-op
pub fn set_dock_badge(label: Option<&str>);          // Linux: Unity-LauncherEntry / No-op
pub fn app_support_dir(app: &str) -> PathBuf;        // XDG unter Linux
```

## 3. Mapping macOS → Linux

| Funktion | macOS heute | Linux-Lösung | Aufwand |
|---|---|---|---|
| Volumes-Liste | `read_dir("/Volumes")`, `diskutil info`, `nobrowse`-Heuristik | `/proc/self/mountinfo` parsen oder `gio mount -l`; Filter auf `/media/$USER`, `/run/media/$USER`, `/mnt`, `$XDG_RUNTIME_DIR/gvfs` | 🟡 2–3 Tage |
| Mount SMB/WebDAV/AFP | `osascript "mount volume …"`, Ergebnis unter `/Volumes/<Name>` | `gio mount smb://…` / `dav://…` (GVFS, ohne Root; Pfad `$XDG_RUNTIME_DIR/gvfs/…`). AFP entfällt (kein GVFS-Backend mehr) | 🔴 4–6 Tage |
| Mount NFS | `mount_nfs` via Admin-Prompt | `pkexec mount -t nfs …` oder `gio mount nfs://` | 🟡 1–2 Tage |
| Unmount / Eject | `diskutil unmount`, `umount` | `gio mount -u`, `udisksctl unmount/power-off` | 🟢 1 Tag |
| SFTP/FTPS/S3 | `rclone mount` (eigenes Binary) | identisch; FUSE (`fuse3`) als Laufzeit-Abhängigkeit, rclone-Linux-Binary in `binaries/rclone-x86_64-unknown-linux-gnu` | 🟢 1 Tag |
| Datei-Icons | `NSWorkspace.icon(forFile:)` → PNG | `xdg-mime query filetype`, `gio info -a standard::icon`, dann Icon aus aktuellem Theme (`freedesktop-icons`-Crate, Fallback hicolor/Adwaita) | 🟡 3 Tage |
| Vorschau (F3) | `qlmanage -t`, `QLPreviewPanel` | Eigene Vorschau im Webview: Bilder/SVG direkt, PDF via `pdf.js`, Text/Code, Video/Audio via `<video>`; Thumbnails per `gdk-pixbuf`/`ffmpegthumbnailer` optional | 🟡 3–4 Tage |
| Promise-Drag nach Finder | Cocoa `NSFilePromiseProvider` | Kein Pendant. Einfacher Drag mit `text/uri-list` (macht `tauri-plugin-drag` bereits); XDS optional später | 🟢 1 Tag (Feature abschalten) |
| Clipboard (Dateien) | Cocoa Pasteboard | `text/uri-list` + `x-special/gnome-copied-files` über `arboard`/`wl-clipboard` | 🟡 2 Tage |
| Dock-Badge / Dock-Menü | Cocoa | No-op bzw. `com.canonical.Unity.LauncherEntry` über D-Bus | 🟢 0,5 Tag |
| „Öffnen mit“ | `NSWorkspace`, `db_choose_application` | `.desktop`-Dateien nach MIME auswerten (`mimeapps.list`), Start mit `gio launch` / `gtk-launch` | 🟡 2–3 Tage |
| Im Dateimanager zeigen | `open -R` | `xdg-open <dir>` oder D-Bus `org.freedesktop.FileManager1.ShowItems` | 🟢 0,5 Tag |
| Privilegiertes Löschen | `osascript … with administrator privileges`, `tmutil`, `chflags` | `pkexec rm -rf` + `chattr -i`; Time-Machine-Teile entfallen | 🟡 1–2 Tage |
| Schlüsselbund | `security-framework` | `keyring`-Crate (Secret Service / KWallet) | 🟢 1 Tag |
| Auto-Refresh | FSEvents über `notify` | inotify über `notify` — nichts zu tun; Hinweis auf `fs.inotify.max_user_watches` | 🟢 0 |
| Papierkorb | `trash`-Crate | freedesktop-Trash, gleiche Crate; „Papierkorb öffnen“ via `xdg-open trash:///` | 🟢 0,5 Tag |
| xattr-Erhalt beim Kopieren | `copyfile(COPYFILE_ALL)` | `copy_file_range`/`std::fs::copy` + `listxattr/getxattr/setxattr`; `user.*`-Namespace | 🟡 1–2 Tage |
| RDP-Profile | `~/Library/Application Support/RemoteDesk/profiles.json` | `$XDG_CONFIG_HOME/RemoteDesk/profiles.json`; Deep-Link `remotedesk://` via `.desktop`-`MimeType=x-scheme-handler/remotedesk` | 🟢 0,5 Tag |
| Updater | DMG + `latest.json` | AppImage-Updates über `tauri-plugin-updater` (unterstützt), `.deb` ohne Auto-Update | 🟢 1 Tag |
| Shortcuts | `⌘` (`metaKey`) | `Ctrl` auf Linux; Umschaltung über `navigator.platform` zentral in `keymap.ts`, Labels in `i18n.ts` | 🟢 1 Tag |
| Fenster-Chrome | `titleBarStyle: Overlay`, Traffic-Lights | Standard-Dekoration oder CSD; `decorations: true` in Linux-Config | 🟢 0,5 Tag |

## 4. Phasenplan

### Phase 0 — Refactoring (nur macOS, kein Verhaltenswechsel) · ~1 Woche
- `platform/`-Modul anlegen, macOS-Code dorthin verschieben.
- `lib.rs` von direkten `osascript`/`diskutil`/`/Volumes`-Aufrufen befreien.
- `/Volumes`-Annahmen in `is_network_mount`, `volume_root_of`, Fehlertexte
  (`delete_error_message`) hinter Plattformfunktionen kapseln.
- Tests in `lib.rs`, die auf `/Volumes` prüfen, mit `#[cfg(target_os = "macos")]`
  markieren oder auf Plattformfunktionen umstellen.
- `build.rs`: `cc`-Schritt bleibt macOS-only (bereits so).

### Phase 1 — Linux-MVP (kompiliert, startet, lokale Dateiarbeit) · ~1 Woche
- `cargo build --target x86_64-unknown-linux-gnu` grün; alle `platform::linux`-
  Funktionen zunächst als No-op/`Err("nicht unterstützt")`.
- Volumes-Liste aus `/proc/self/mountinfo`.
- `xdg-open`, FileManager1-D-Bus, Papierkorb, Keyring.
- Shortcuts auf `Ctrl`, Fensterdekoration, XDG-Pfade für Profile/Persistenz.
- rclone-Linux-Binary einbinden; SFTP/FTPS/S3 funktionieren damit bereits.
- Build-Ziele `deb` + `appimage` in `tauri.conf.json`; GitHub-Action
  `ubuntu-22.04` (Mindestbasis wegen `webkit2gtk-4.1`).

### Phase 2 — Netzlaufwerke · ~1–1,5 Wochen
- GVFS-Anbindung (`gio mount`) für SMB, WebDAV, NFS inkl. Passwort-Übergabe
  über stdin und Fehlerabbildung auf bestehende Fehlercodes (`NETWORK_BUSY` …).
- Mount-Pfade unter `$XDG_RUNTIME_DIR/gvfs` als Netzlaufwerk erkennen.
- Unmount über `gio mount -u` / `udisksctl`.
- Fallback-Hinweis in der UI, wenn `gvfs-backends` fehlt.

### Phase 3 — Komfort-Funktionen · ~1,5 Wochen
- Datei-Icons aus Icon-Theme.
- Vorschau (F3) im Webview inkl. PDF und Thumbnails.
- „Öffnen mit“ aus `.desktop`-Dateien.
- Datei-Clipboard (`x-special/gnome-copied-files`).
- Privilegiertes Löschen mit `pkexec`.

### Phase 4 — Feinschliff & Release · ~0,5 Woche
- Wayland/X11 testen (GNOME, KDE, XFCE).
- `.deb`-Abhängigkeiten: `libwebkit2gtk-4.1-0`, `libgtk-3-0`, `fuse3`,
  `gvfs-backends` (Recommends), `xdg-utils`.
- Dokumentation: README, SPEC, Installationshinweise.

**Gesamt: ca. 5–6 Wochen** für Feature-Parität ohne Promise-Drag und AFP.
Ein nutzbares MVP (Phase 0+1) steht nach ca. 2 Wochen.

## 5. Was nicht portiert wird

- **Promise-Drags nach Finder** — kein Linux-Äquivalent; normaler URI-Drag reicht.
- **AFP** — unter Linux faktisch tot.
- **Time-Machine-Sonderfälle** (`tmutil`, `Backups.backupdb`, lokale Snapshots).
- **QuickLook-Plugins** Dritter — die eigene Vorschau deckt gängige Typen ab.
- **„Vollen Festplattenzugriff“-Dialog** (`x-apple.systempreferences:`) — unter Linux ohne Entsprechung.

## 6. Risiken

| Risiko | Gegenmaßnahme |
|---|---|
| GVFS fehlt auf minimalen Systemen (Server, Container) | Erkennung beim Start, klarer Hinweis; rclone als Alternative für SMB/WebDAV (rclone kann beides) |
| Wayland-Drag&Drop im WebKitGTK unzuverlässig | Frühzeitig in Phase 1 testen; ggf. nur In-App-Drag |
| WebKitGTK-Rendering-Unterschiede zu WKWebView | Layout mit `-webkit-`-Präfixen bereits kompatibel; Playwright-Screenshots im CI |
| `trash`-Crate auf Fremd-Dateisystemen (NTFS, Netz) | Fallback auf Löschen mit Rückfrage — existiert für macOS schon |
| FUSE/AppArmor für rclone-Mounts | `fuse3` als Abhängigkeit, `user_allow_other` nicht nötig bei eigenem Mountpoint |

## 7. Entwicklungs-Setup unter Ubuntu

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf build-essential curl wget file libssl-dev fuse3 gvfs-backends
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
npm install
npm run tauri dev
npm run tauri build -- --bundles deb,appimage
```

Für das rclone-Binary: `scripts/`-Skript um Download der Linux-Variante
erweitern und als `src-tauri/binaries/rclone-x86_64-unknown-linux-gnu` ablegen
(Tauri wählt die Variante nach Target-Triple).
