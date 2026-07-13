use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::net::IpAddr;
use std::process::{Command, Stdio};

mod promise_drag;
use notify_debouncer_mini::notify::RecommendedWatcher;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, Debouncer};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;

/// Sperrt einen Mutex und übernimmt im Poison-Fall den inneren Guard,
/// statt zu panicen. Verhindert Folgeabstürze, falls ein Thread beim
/// Halten des Locks paniert.
fn lock_safe<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub mtime: i64,
    pub ext: String,
    pub hidden: bool,
    pub birth_time: i64,
    pub kind: String,
    pub owner: String,
    pub group: String,
    pub mode_str: String,
}

fn mode_to_rwx(mode: u32) -> String {
    let perms = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    let mut s = String::with_capacity(9);
    for (bit, ch) in perms {
        s.push(if mode & bit != 0 { ch } else { '-' });
    }
    s
}

fn uid_to_name(uid: u32) -> String {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = lock_safe(cache).get(&uid) {
        return v.clone();
    }
    let name = unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            uid.to_string()
        } else {
            let cstr = std::ffi::CStr::from_ptr((*pw).pw_name);
            cstr.to_string_lossy().into_owned()
        }
    };
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(uid, name.clone());
    name
}

fn gid_to_name(gid: u32) -> String {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = lock_safe(cache).get(&gid) {
        return v.clone();
    }
    let name = unsafe {
        let gr = libc::getgrgid(gid as libc::gid_t);
        if gr.is_null() {
            gid.to_string()
        } else {
            let cstr = std::ffi::CStr::from_ptr((*gr).gr_name);
            cstr.to_string_lossy().into_owned()
        }
    };
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(gid, name.clone());
    name
}

fn ext_to_kind(ext: &str, is_dir: bool, is_symlink: bool) -> String {
    if is_symlink {
        return "Symlink".into();
    }
    if is_dir {
        if ext == "app" {
            return "Programm".into();
        }
        return "Ordner".into();
    }
    match ext {
        "" => "Datei".into(),
        "pdf" => "PDF-Dokument".into(),
        "txt" | "md" | "rtf" => "Textdokument".into(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "tiff" | "bmp" => "Bild".into(),
        "mp3" | "wav" | "aac" | "flac" | "m4a" | "ogg" => "Audio".into(),
        "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" => "Video".into(),
        "zip" | "tar" | "gz" | "bz2" | "7z" | "rar" | "dmg" => "Archiv".into(),
        "html" | "htm" => "HTML-Dokument".into(),
        "json" | "xml" | "yaml" | "yml" | "toml" | "csv" => "Datendatei".into(),
        "rs" | "ts" | "js" | "tsx" | "jsx" | "py" | "swift" | "c" | "cpp" | "h" | "go" | "rb"
        | "sh" => "Quellcode".into(),
        other => format!("{}-Datei", other.to_uppercase()),
    }
}

/// Öffnet ein weiteres unabhängiges App-Fenster.
pub(crate) fn open_new_window(app: &AppHandle) {
    use std::sync::atomic::AtomicU32;
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    let label = format!("win-{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    let builder =
        tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App("index.html".into()))
            .title("DualBeam")
            .inner_size(1280.0, 800.0)
            .min_inner_size(900.0, 500.0)
            .resizable(true)
            .center();
    if let Err(e) = builder.build() {
        eprintln!("Neues Fenster konnte nicht erstellt werden: {e}");
    }
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(stripped) = p.strip_prefix("~") {
        if let Some(home) = dirs::home_dir() {
            let rest = stripped.trim_start_matches('/');
            return if rest.is_empty() {
                home
            } else {
                home.join(rest)
            };
        }
    }
    PathBuf::from(p)
}

#[tauri::command]
fn home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "Home-Verzeichnis nicht gefunden".into())
}

#[tauri::command]
fn list_dir(path: String, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let p = expand_tilde(&path);
    let read = std::fs::read_dir(&p).map_err(|e| format!("{}: {}", p.display(), e))?;

    use std::os::unix::fs::MetadataExt;
    let mut out: Vec<Entry> = Vec::new();
    for ent in read.flatten() {
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().into_owned();
        let hidden = name.starts_with('.');
        if hidden && !show_hidden {
            continue;
        }
        // `file_type()` stammt aus dem readdir-d_type und braucht (anders als
        // `metadata()`) keinen zusätzlichen stat/PROPFIND-Roundtrip. Auf
        // Netzlaufwerken (WebDAV/SMB) kann `metadata()` zeitweise scheitern;
        // Einträge dürfen dann NICHT verschwinden – wir fallen auf den
        // file_type bzw. symlink_metadata zurück.
        let ft = ent.file_type().ok();
        let symlink_meta = std::fs::symlink_metadata(&path).ok();
        let is_symlink = ft
            .map(|t| t.is_symlink())
            .or_else(|| symlink_meta.as_ref().map(|m| m.file_type().is_symlink()))
            .unwrap_or(false);
        // Für die übrigen Felder die volle Metadata versuchen, sonst auf
        // symlink_metadata zurückfallen, damit der Eintrag erhalten bleibt.
        let meta = ent.metadata().ok().or_else(|| symlink_meta.clone());
        let is_dir = meta
            .as_ref()
            .map(|m| m.is_dir())
            .or_else(|| ft.map(|t| t.is_dir()))
            .unwrap_or(false);
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let ext = Path::new(&name)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let mode_str = meta
            .as_ref()
            .map(|m| mode_to_rwx(m.mode()))
            .unwrap_or_default();
        let owner = meta
            .as_ref()
            .map(|m| uid_to_name(m.uid()))
            .unwrap_or_default();
        let group = meta
            .as_ref()
            .map(|m| gid_to_name(m.gid()))
            .unwrap_or_default();
        let birth_time = meta
            .as_ref()
            .and_then(|m| m.created().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = if is_dir {
            0
        } else {
            meta.as_ref().map(|m| m.len()).unwrap_or(0)
        };
        let kind = ext_to_kind(&ext, is_dir, is_symlink);
        out.push(Entry {
            name,
            path: path.to_string_lossy().into_owned(),
            is_dir,
            is_symlink,
            size,
            mtime,
            ext,
            hidden,
            birth_time,
            kind,
            owner,
            group,
            mode_str,
        });
    }
    Ok(out)
}

#[tauri::command]
fn open_default(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    std::process::Command::new("open")
        .arg(&p)
        .status()
        .map_err(|e| e.to_string())
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(format!("open exit {:?}", s.code()))
            }
        })
}

#[tauri::command]
fn open_privacy_settings() -> Result<(), String> {
    Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .status()
        .map_err(|e| e.to_string())
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(format!("open exit {:?}", s.code()))
            }
        })
}

// ---------- Single-shot file ops ----------

#[tauri::command]
fn create_dir(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    if p.exists() {
        return Err(format!("err.exists\u{1f}{}", p.display()));
    }
    std::fs::create_dir(&p).map_err(|e| format!("{}: {}", p.display(), e))
}

#[tauri::command]
fn create_file(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    if p.exists() {
        return Err(format!("err.exists\u{1f}{}", p.display()));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map(|_| ())
        .map_err(|e| format!("{}: {}", p.display(), e))
}

#[tauri::command]
fn create_symlink(target: String, link_path: String) -> Result<(), String> {
    let t = expand_tilde(&target);
    let l = expand_tilde(&link_path);
    if l.exists() || std::fs::symlink_metadata(&l).is_ok() {
        return Err(format!("err.exists\u{1f}{}", l.display()));
    }
    std::os::unix::fs::symlink(&t, &l).map_err(|e| format!("{}: {}", l.display(), e))
}

#[tauri::command]
fn create_finder_alias(target: String, link_path: String) -> Result<(), String> {
    let t = expand_tilde(&target);
    let l = expand_tilde(&link_path);
    if l.exists() || std::fs::symlink_metadata(&l).is_ok() {
        return Err(format!("err.exists\u{1f}{}", l.display()));
    }
    let parent = l.parent().ok_or_else(|| "Ungültiges Ziel".to_string())?;
    let name = l
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Ungültiger Name".to_string())?;
    let esc = |s: &str| -> Result<String, String> {
        if s.contains('\n') || s.contains('\r') || s.contains('\0') {
            return Err("err.path.invalidChar".into());
        }
        Ok(s.replace('\\', "\\\\").replace('"', "\\\""))
    };
    let script = format!(
        "tell application \"Finder\"\n\
         set theTarget to POSIX file \"{tgt}\" as alias\n\
         set theFolder to POSIX file \"{par}\" as alias\n\
         set newAlias to make new alias file at theFolder to theTarget\n\
         set name of newAlias to \"{nm}\"\n\
         end tell",
        tgt = esc(&t.display().to_string())?,
        par = esc(&parent.display().to_string())?,
        nm = esc(name)?,
    );
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("osascript: {}", e))?;
    if !out.status.success() {
        return Err(format!(
            "Alias-Erstellung fehlgeschlagen: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

#[tauri::command]
fn rename_path(old_path: String, new_path: String) -> Result<(), String> {
    let a = expand_tilde(&old_path);
    let b = expand_tilde(&new_path);
    if a == b {
        return Ok(());
    }
    if b.exists() {
        return Err(format!("err.exists\u{1f}{}", b.display()));
    }
    std::fs::rename(&a, &b).map_err(|e| e.to_string())
}

/// Mountpoints registrierter Time-Machine-Ziele (canonicalize'd) für schnelle
/// Präfix-Vergleiche. `tmutil destinationinfo` wird dabei nur einmal aufgerufen.
fn tm_mountpoints_canon() -> Vec<std::path::PathBuf> {
    get_tm_mountpoints()
        .into_iter()
        .map(|m| std::fs::canonicalize(&m).unwrap_or_else(|_| std::path::PathBuf::from(m)))
        .collect()
}

/// Erkennt, ob ein Pfad zu einem Time-Machine-Backup gehört. Bewusst eng
/// gefasst, um normale Nutzerdateien (z.B. `notiz.backup`) nicht zu blockieren.
fn is_time_machine_path(full: &Path, tm_mounts: &[std::path::PathBuf]) -> bool {
    // 1) Innerhalb eines registrierten Time-Machine-Ziel-Volumes.
    let canon = std::fs::canonicalize(full).ok();
    let target = canon.as_deref().unwrap_or(full);
    for mp in tm_mounts {
        if target == mp.as_path() || target.starts_with(mp) {
            return true;
        }
    }
    // 2) Eindeutige Time-Machine-Pfadbestandteile bzw. -Endungen.
    for comp in full.components() {
        if let std::path::Component::Normal(os) = comp {
            let s = os.to_string_lossy();
            if s.eq_ignore_ascii_case("Backups.backupdb")
                || s == ".timemachine"
                || s == ".MobileBackups"
            {
                return true;
            }
            if s.ends_with(".backupbundle")
                || s.ends_with(".inprogress")
                || s.ends_with(".previous")
                || s.ends_with(".interrupted")
            {
                return true;
            }
        }
    }
    // 3) Ein übergeordnetes Verzeichnis ist eine TM-Backup-Wurzel (greift auch
    //    bei ehemaligen, nicht mehr registrierten Backup-Volumes).
    let mut cur: Option<&Path> = Some(full);
    let mut depth = 0u32;
    while let Some(p) = cur {
        if depth > 64 {
            break;
        }
        if p.join("backup_manifest.plist").is_file() || p.join("Backups.backupdb").is_dir() {
            return true;
        }
        cur = p.parent();
        depth += 1;
    }
    false
}

#[tauri::command]
fn move_to_trash(paths: Vec<String>) -> Result<(), String> {
    use std::os::macos::fs::MetadataExt;
    use trash::macos::{DeleteMethod, TrashContextExtMacos};
    const PROTECT_MASK: u32 = 0x0002 | 0x0004 | 0x00020000 | 0x00040000 | 0x00080000 | 0x00100000;
    let tm_mounts = tm_mountpoints_canon();
    let fs = mount_fs_types();
    let mut local_trash_paths = Vec::new();
    for p in &paths {
        let full = expand_tilde(p);
        // Bereits gelöscht? Auf Netzlaufwerken können verwaiste AppleDouble-
        // Dateien (`._X`) zwischen Vorschau und Löschung verschwinden. Dann
        // ist nichts mehr zu tun – kein Fehler (os error 2 / ENOENT vermeiden).
        match std::fs::symlink_metadata(&full) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {}
        }
        // Time-Machine-Backups dürfen nicht über das normale Panel gelöscht
        // werden – das Frontend zeigt dafür einen Hinweis statt Admin-Löschen.
        if is_time_machine_path(&full, &tm_mounts) {
            return Err(format!("TIMEMACHINE_PROTECTED\u{1f}{}", full.display()));
        }
        // Symlinks: das `trash`-Crate folgt auf macOS teilweise dem Ziel
        // und scheitert dann bei fehlenden Rechten am Zielpfad oder bei
        // kaputten Links. Symlinks daher direkt entfernen (nur der Link,
        // nicht das Ziel).
        let is_symlink = std::fs::symlink_metadata(&full)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            std::fs::remove_file(&full).map_err(|e| format!("{}: {}", full.display(), e))?;
            continue;
        }
        // Netzlaufwerke (WebDAV/SMB/NFS …) haben keinen brauchbaren Papierkorb:
        // `.Trashes` liegt auf demselben Server, und das Verschieben großer
        // Dateien dorthin scheitert und hinterlässt eine 0-Byte-Leiche. Dort
        // – wie der Finder – direkt und dauerhaft löschen.
        if is_network_path(&full, &fs) {
            remove_path(&full).map_err(|e| format!("{}: {}", full.display(), e))?;
            continue;
        }
        let needs_admin = std::fs::symlink_metadata(&full)
            .map(|m| (m.st_flags() & PROTECT_MASK) != 0)
            .unwrap_or(false)
            || full
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".inprogress"))
                .unwrap_or(false);
        if needs_admin {
            return Err(format!("NEEDS_ADMIN: {}", full.display()));
        }
        local_trash_paths.push(full);
    }
    if !local_trash_paths.is_empty() {
        // Der Default des `trash`-Crates ruft Finder auf. Finder spielt für
        // JEDES Objekt einen Löschton ab, was bei einer Sync-Löschung wie ein
        // Maschinengewehr klingt. NSFileManager verschiebt dieselben Objekte
        // lautlos in den Papierkorb und kann sie in einem Batch verarbeiten.
        let mut trash_ctx = trash::TrashContext::new();
        trash_ctx.set_delete_method(DeleteMethod::NsFileManager);
        trash_ctx
            .delete_all(&local_trash_paths)
            .map_err(|e| format!("Papierkorb: {e}"))?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UndoDeleteItem {
    original: String,
    staged: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UndoDeleteBatch {
    token: String,
    items: Vec<UndoDeleteItem>,
}

fn undo_staging_dir(token: &str) -> Result<PathBuf, String> {
    let base = dirs::data_local_dir().ok_or_else(|| "Undo-Ordner nicht verfügbar".to_string())?;
    Ok(base.join("DualBeam").join("Undo").join(token))
}

#[tauri::command]
fn stage_delete_for_undo(paths: Vec<String>) -> Result<UndoDeleteBatch, String> {
    use std::os::macos::fs::MetadataExt;

    const PROTECT_MASK: u32 = 0x0002 | 0x0004 | 0x00020000 | 0x00040000 | 0x00080000 | 0x00100000;
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let dir = undo_staging_dir(&token)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tm_mounts = tm_mountpoints_canon();
    let mounts = mount_fs_types();
    let mut originals = Vec::new();
    for raw in paths {
        let original = expand_tilde(&raw);
        let metadata = match std::fs::symlink_metadata(&original) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {}", original.display(), e)),
        };
        if is_time_machine_path(&original, &tm_mounts) {
            return Err(format!("TIMEMACHINE_PROTECTED\u{1f}{}", original.display()));
        }
        if is_network_path(&original, &mounts) {
            return Err(format!("NETWORK_DELETE_DIRECT: {}", original.display()));
        }
        let needs_admin = (metadata.st_flags() & PROTECT_MASK) != 0
            || original
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".inprogress"))
                .unwrap_or(false);
        if needs_admin {
            return Err(format!("NEEDS_ADMIN: {}", original.display()));
        }
        originals.push(original);
    }

    let mut items: Vec<UndoDeleteItem> = Vec::new();
    for (index, original) in originals.into_iter().enumerate() {
        let name = original
            .file_name()
            .ok_or_else(|| "Ungültiger Löschpfad".to_string())?;
        let staged = dir.join(format!("{index}-{}", name.to_string_lossy()));
        if let Err(e) = std::fs::rename(&original, &staged) {
            // Eine teilweise verschobene Auswahl darf nie zurückbleiben.
            for item in items.iter().rev() {
                let _ = std::fs::rename(&item.staged, &item.original);
            }
            return Err(format!("{}: {}", original.display(), e));
        }
        items.push(UndoDeleteItem {
            original: original.to_string_lossy().into_owned(),
            staged: staged.to_string_lossy().into_owned(),
        });
    }
    Ok(UndoDeleteBatch { token, items })
}

#[tauri::command]
fn undo_staged_delete(items: Vec<UndoDeleteItem>) -> Result<(), String> {
    for item in &items {
        let original = PathBuf::from(&item.original);
        let staged = PathBuf::from(&item.staged);
        if original.exists() {
            return Err(format!("{} existiert bereits", original.display()));
        }
        if let Some(parent) = original.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&staged, &original)
            .map_err(|e| format!("{}: {}", original.display(), e))?;
    }
    Ok(())
}

#[tauri::command]
fn finalize_staged_delete(items: Vec<UndoDeleteItem>) -> Result<(), String> {
    let staged: Vec<String> = items.into_iter().map(|item| item.staged).collect();
    move_to_trash(staged)
}

/// Entfernt abgelaufene Rückgängig-Puffer aus früheren Sitzungen. Der Puffer
/// liegt ausschließlich im App-Datenordner und wird erst nach zehn Minuten
/// lautlos in den Papierkorb verschoben.
#[tauri::command]
fn cleanup_expired_undo() -> Result<(), String> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| "Undo-Ordner nicht verfügbar".to_string())?
        .join("DualBeam")
        .join("Undo");
    let expired = match std::fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let paths: Vec<String> = expired
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_dir() {
                return None;
            }
            let age = meta.modified().ok()?.elapsed().ok()?;
            (age >= Duration::from_secs(10 * 60))
                .then(|| entry.path().to_string_lossy().into_owned())
        })
        .collect();
    if paths.is_empty() {
        return Ok(());
    }
    move_to_trash(paths)
}

/// Ein privilegierter Löschvorgang darf nie auf einen System- oder
/// Benutzerstamm zeigen. Einzelne Objekte darunter dürfen weiterhin bewusst
/// gelöscht werden, nachdem die normale Bestätigung erfolgt ist.
fn is_protected_admin_root(path: &Path) -> bool {
    const ROOTS: &[&str] = &[
        "/",
        "/Applications",
        "/Library",
        "/System",
        "/Users",
        "/Volumes",
        "/bin",
        "/private",
        "/sbin",
        "/usr",
    ];
    let normalized = canonicalize_target_path(path).unwrap_or_else(|_| path.to_path_buf());
    ROOTS.iter().any(|root| normalized == Path::new(root))
}

#[tauri::command]
fn force_delete_admin(paths: Vec<String>) -> Result<(), String> {
    use std::io::Write;
    // Diagnose-Log nur in Debug-Builds; im Release wird nichts auf die Platte geschrieben.
    #[cfg(debug_assertions)]
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/dualbeam-delete.log")
        .ok();
    #[cfg(not(debug_assertions))]
    let mut log: Option<std::fs::File> = None;
    let logln = |log: &mut Option<std::fs::File>, s: &str| {
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{}", s);
        }
    };
    logln(
        &mut log,
        &format!(
            "=== ts={} ===",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ),
    );
    logln(&mut log, &format!("paths: {:?}", paths));
    if paths.is_empty() {
        return Ok(());
    }
    let mut parts: Vec<String> = Vec::with_capacity(paths.len() * 6);
    for p in &paths {
        let full = expand_tilde(p);
        let s = full.to_string_lossy().into_owned();
        logln(
            &mut log,
            &format!("expanded: {} exists_before={}", s, full.exists()),
        );
        if s.is_empty() || is_protected_admin_root(&full) {
            return Err(format!("err.path.forbidden\u{1f}{}", s));
        }
        let parent = full
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        let q = shell_single_quote(&s);
        let qp = shell_single_quote(&parent);
        parts.push(format!(
            "/usr/bin/tmutil delete -p {q} 2>&1; if [ -e {q} ]; then /bin/chmod -N {qp} 2>&1; /usr/bin/chflags nouchg,noschg,nouappnd,nosappnd,nouunlnk,nosunlnk {qp} 2>&1; /usr/bin/xattr -rc {q} 2>&1; /bin/chmod -RN {q} 2>&1; /bin/chmod -R u+rwX {q} 2>&1; /usr/bin/chflags -R nouchg,noschg,nouappnd,nosappnd,nouunlnk,nosunlnk {q} 2>&1; /bin/rm -rfv {q} 2>&1; fi; echo \"final-exit=$?\"; /bin/ls -lad {q} 2>&1 || echo gone",
            q = q, qp = qp
        ));
    }
    let cmd = parts.join(" ; ");
    logln(&mut log, &format!("cmd: {}", cmd));
    let result = run_with_admin(&cmd);
    match &result {
        Ok(out) => logln(&mut log, &format!("admin OK out:\n{}", out)),
        Err(e) => logln(&mut log, &format!("admin ERR: {}", e)),
    }
    let out = result?;
    let mut still: Vec<String> = Vec::new();
    for p in &paths {
        let full = expand_tilde(p);
        let ex = full.exists();
        logln(
            &mut log,
            &format!("after: {} exists_after={}", full.display(), ex),
        );
        if ex {
            still.push(full.to_string_lossy().into_owned());
        }
    }
    if !still.is_empty() {
        return Err(format!(
            "Nicht gelöscht:\n{}\n\nAusgabe:\n{}",
            still.join("\n"),
            out.trim()
        ));
    }
    Ok(())
}

#[tauri::command]
fn path_exists(path: String) -> bool {
    expand_tilde(&path).exists()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub path: String,
    pub kind: String, // "local" | "network"
}

fn mount_fs_types() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(out) = std::process::Command::new("/sbin/mount").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            // Format: "<src> on <mountpoint> (<fstype>, ...)"
            for line in s.lines() {
                if let Some(on_idx) = line.find(" on ") {
                    let rest = &line[on_idx + 4..];
                    if let Some(paren) = rest.rfind(" (") {
                        let mp = &rest[..paren];
                        let opts = &rest[paren + 2..];
                        let fstype = opts
                            .split(',')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .trim_end_matches(')');
                        map.insert(mp.to_string(), fstype.to_string());
                    }
                }
            }
        }
    }
    map
}

// Netzwerk-Dateisystemtypen. Auf solchen Volumes liegt der „Papierkorb" als
// `.Trashes` auf demselben Server – das Verschieben großer Dateien dorthin
// scheitert (Timeout/Serverfehler) und hinterlässt eine 0-Byte-Leiche.
fn is_network_fstype(fstype: &str) -> bool {
    matches!(
        fstype,
        "webdav" | "smbfs" | "nfs" | "afpfs" | "ftp" | "cifs"
    )
}

fn is_hidrive_webdav_path(path: &Path) -> bool {
    #[cfg(feature = "hidrive")]
    {
        path.starts_with(Path::new("/Volumes").join(HIDRIVE_HOST))
    }
    #[cfg(not(feature = "hidrive"))]
    {
        let _ = path;
        false
    }
}

fn is_network_path(path: &Path, mounts: &std::collections::HashMap<String, String>) -> bool {
    // Fallback für macOS WebDAV: In seltenen Fällen liefert die Mount-Tabelle
    // während eines laufenden Finder-Zugriffs keinen passenden Präfix-Treffer.
    // Der bekannte HiDrive-Mount darf dann trotzdem nie in den lokalen
    // Papierkorb/Undo-Ordner verschoben werden.
    is_hidrive_webdav_path(path)
        || path_fstype(path, mounts)
            .map(|fstype| is_network_fstype(&fstype))
            .unwrap_or(false)
}

// Ermittelt den Dateisystemtyp eines Pfads über das am längsten passende
// Mountpoint-Präfix (längster Treffer gewinnt, damit verschachtelte Mounts
// korrekt erkannt werden).
fn path_fstype(full: &Path, mounts: &std::collections::HashMap<String, String>) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (mp, fstype) in mounts {
        if full.starts_with(mp) {
            let len = mp.len();
            if best.as_ref().map(|(l, _)| len > *l).unwrap_or(true) {
                best = Some((len, fstype.clone()));
            }
        }
    }
    best.map(|(_, fstype)| fstype)
}

// Prüft für das Frontend, ob ein Pfad auf einem Netzlaufwerk liegt (dann wird
// beim Löschen direkt dauerhaft entfernt statt in den Papierkorb verschoben).
#[tauri::command]
fn path_is_network(path: String) -> bool {
    let full = expand_tilde(&path);
    let mounts = mount_fs_types();
    is_network_path(&full, &mounts)
}

// IONOS HiDrive WebDAV-Netzwerk-Bookmark (Host, Anzeigename, URL an einer Stelle).
// Nur in der persönlichen Build-Variante (Feature `hidrive`, standardmäßig aktiv).
// Die öffentliche Version wird mit `--no-default-features` gebaut; dann existiert
// diese personenbezogene Voreinstellung nicht im Binary.
#[cfg(feature = "hidrive")]
const HIDRIVE_HOST: &str = "webdav.hidrive.ionos.com";
#[cfg(feature = "hidrive")]
const HIDRIVE_NAME: &str = "IONOS HiDrive";
#[cfg(feature = "hidrive")]
const HIDRIVE_URL: &str = "https://webdav.hidrive.ionos.com/";

// Anzeigename für ein gemountetes Volume. In der HiDrive-Variante wird der
// technische WebDAV-Hostname durch den freundlichen Namen ersetzt.
#[cfg(feature = "hidrive")]
fn volume_display_name(name: &str) -> String {
    match name {
        HIDRIVE_HOST => HIDRIVE_NAME.to_string(),
        _ => name.to_string(),
    }
}
#[cfg(not(feature = "hidrive"))]
fn volume_display_name(name: &str) -> String {
    name.to_string()
}

#[tauri::command]
fn list_volumes() -> Result<Vec<Volume>, String> {
    let mut out: Vec<Volume> = Vec::new();
    let fs = mount_fs_types();
    if let Ok(rd) = std::fs::read_dir("/Volumes") {
        for ent in rd.flatten() {
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            // Synthetische APFS-Firmlinks (z.B. TimeMachine-Snapshots) ausblenden.
            if name == "com.apple.TimeMachine.localsnapshots" {
                continue;
            }
            let path_str = path.to_string_lossy().into_owned();
            let fstype = fs.get(&path_str).cloned().unwrap_or_default();
            let kind = match fstype.as_str() {
                "webdav" | "smbfs" | "nfs" | "afpfs" | "ftp" | "cifs" => "network",
                _ => "local",
            }
            .to_string();
            let display = volume_display_name(&name);
            out.push(Volume {
                name: display,
                path: path_str,
                kind,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkBookmark {
    pub name: String,
    pub url: String,
    pub mount_path: String,
    pub connected: bool,
}

#[derive(Deserialize, Serialize, Default)]
#[serde(default)]
struct NetworkBookmarkSettings {
    removed_urls: Vec<String>,
    bookmarks: Vec<StoredNetworkBookmark>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StoredNetworkBookmark {
    name: String,
    url: String,
    mount_path: String,
}

fn network_bookmark_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("dualbeam").join("network-bookmarks.json"))
}

fn load_network_bookmark_settings() -> NetworkBookmarkSettings {
    let Some(path) = network_bookmark_settings_path() else {
        return NetworkBookmarkSettings::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_network_bookmark_settings(settings: &NetworkBookmarkSettings) -> Result<(), String> {
    let path = network_bookmark_settings_path()
        .ok_or_else(|| "Netzwerk-Lesezeichen können nicht gespeichert werden".to_string())?;
    let dir = path
        .parent()
        .ok_or_else(|| "Ungültiger Einstellungsordner".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn builtin_network_bookmarks() -> Vec<(String, String, String)> {
    // (name, url, expected mount path)
    #[cfg(feature = "hidrive")]
    {
        vec![(
            HIDRIVE_NAME.into(),
            HIDRIVE_URL.into(),
            format!("/Volumes/{}", HIDRIVE_HOST),
        )]
    }
    // Öffentliche Version: keine vordefinierten Netzwerk-Lesezeichen.
    #[cfg(not(feature = "hidrive"))]
    {
        Vec::new()
    }
}

fn known_network_bookmarks() -> Vec<(String, String, String)> {
    let settings = load_network_bookmark_settings();
    let mut bookmarks: Vec<(String, String, String)> = builtin_network_bookmarks()
        .into_iter()
        .filter(|(_, url, _)| !settings.removed_urls.contains(url))
        .collect();
    for bookmark in settings.bookmarks {
        if !settings.removed_urls.contains(&bookmark.url)
            && !bookmarks.iter().any(|(_, url, _)| url == &bookmark.url)
        {
            bookmarks.push((bookmark.name, bookmark.url, bookmark.mount_path));
        }
    }
    bookmarks
}

/// Liest für jeden Mountpoint die Quelle und den Dateisystemtyp aus. Die
/// Quelle enthält bei macOS-Netzmounts die erneute Verbindungs-URL bzw. den
/// SMB-Pfad und wird nur für Lesezeichen verwendet.
fn mount_source_and_fstype() -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    if let Ok(out) = Command::new("/sbin/mount").output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let Some(on_idx) = line.find(" on ") else {
                    continue;
                };
                let source = line[..on_idx].trim();
                let rest = &line[on_idx + 4..];
                let Some(paren_idx) = rest.rfind(" (") else {
                    continue;
                };
                let mount_path = rest[..paren_idx].trim();
                let fstype = rest[paren_idx + 2..]
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(')');
                map.insert(
                    mount_path.to_string(),
                    (source.to_string(), fstype.to_string()),
                );
            }
        }
    }
    map
}

fn bookmark_url_from_mount_source(source: &str, fstype: &str) -> Option<String> {
    let candidate = if source.starts_with("//") && fstype == "smbfs" {
        format!("smb:{source}")
    } else {
        source.to_string()
    };
    let mut parsed = url::Url::parse(&candidate).ok()?;
    if parsed.host_str().is_none() {
        return None;
    }
    // Zugangsdaten gehören in den Schlüsselbund, niemals in das gespeicherte
    // Lesezeichen. Sie würden sonst in der App-Konfiguration landen.
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    Some(parsed.to_string())
}

fn remember_network_volume_inner(path: &Path) -> Result<(), String> {
    let mount_path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let mount_path_str = mount_path.to_string_lossy().into_owned();
    let (source, fstype) = mount_source_and_fstype()
        .remove(&mount_path_str)
        .ok_or_else(|| "Netzlaufwerk ist nicht mehr eingebunden".to_string())?;
    if !is_network_fstype(&fstype) {
        return Err("Kein Netzlaufwerk".into());
    }
    let url = bookmark_url_from_mount_source(&source, &fstype).ok_or_else(|| {
        "Verbindungsadresse des Netzlaufwerks konnte nicht ermittelt werden".to_string()
    })?;
    let name = mount_path
        .file_name()
        .map(|name| volume_display_name(&name.to_string_lossy()))
        .unwrap_or_else(|| url.clone());
    let mut settings = load_network_bookmark_settings();
    settings.removed_urls.retain(|removed| removed != &url);
    if !builtin_network_bookmarks()
        .iter()
        .any(|(_, known_url, _)| known_url == &url)
    {
        if let Some(bookmark) = settings.bookmarks.iter_mut().find(|item| item.url == url) {
            bookmark.name = name;
            bookmark.mount_path = mount_path_str;
        } else {
            settings.bookmarks.push(StoredNetworkBookmark {
                name,
                url,
                mount_path: mount_path_str,
            });
        }
    }
    save_network_bookmark_settings(&settings)
}

#[tauri::command]
fn list_network_bookmarks() -> Result<Vec<NetworkBookmark>, String> {
    let fs = mount_fs_types();
    let mut out = Vec::new();
    for (name, url, mp) in known_network_bookmarks() {
        let connected = fs.contains_key(&mp);
        out.push(NetworkBookmark {
            name,
            url,
            mount_path: mp,
            connected,
        });
    }
    Ok(out)
}

/// Entfernt ein von DualBeam bereitgestelltes Netzwerk-Lesezeichen dauerhaft
/// aus der Seitenleiste. macOS-Anmeldedaten im Schlüsselbund bleiben bewusst
/// unberührt; sie gehören dem Betriebssystem und können dort separat verwaltet
/// werden.
#[tauri::command]
fn remove_network_bookmark(url: String) -> Result<(), String> {
    let is_builtin = builtin_network_bookmarks()
        .iter()
        .any(|(_, known_url, _)| known_url == &url);
    let mut settings = load_network_bookmark_settings();
    let custom_count = settings.bookmarks.len();
    settings.bookmarks.retain(|bookmark| bookmark.url != url);
    if !is_builtin && settings.bookmarks.len() == custom_count {
        return Err("Unbekanntes Netzwerk-Lesezeichen".into());
    }
    if is_builtin && !settings.removed_urls.contains(&url) {
        settings.removed_urls.push(url);
    }
    save_network_bookmark_settings(&settings)
}

/// Macht ein bereits von macOS gemountetes Netzlaufwerk zu einem DualBeam-
/// Lesezeichen, damit es nach dem Aushängen in der Seitenleiste bleibt.
#[tauri::command]
fn remember_network_volume(path: String) -> Result<(), String> {
    remember_network_volume_inner(&expand_tilde(&path))
}

fn is_local_network_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            a == 10
                || a == 127
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
        }
        IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            ip.is_loopback() || (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00
        }
    }
}

/// Validiert eine Mount-URL. Unsichere Protokolle sind bewusst ausschließlich
/// für direkte private, Link-Local- oder Loopback-IP-Adressen erlaubt. Damit
/// kann eine DNS-Auflösung weder unbeabsichtigt nach außen gehen noch später
/// auf ein öffentliches Ziel umgebogen werden.
fn parse_mount_url(
    input: &str,
    allow_insecure_local: bool,
) -> Result<(String, bool, bool), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err("err.network.badchars".into());
    }
    let parsed = url::Url::parse(trimmed).map_err(|_| "err.network.invalidUrl")?;
    let scheme = parsed.scheme();
    let secure = matches!(scheme, "https" | "smb");
    let insecure = matches!(scheme, "http" | "ftp" | "ftps" | "afp" | "nfs" | "cifs");
    if !secure && !insecure {
        return Err("err.network.scheme".into());
    }
    if parsed.host_str().is_none() {
        return Err("err.network.invalidUrl".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("err.network.credentials".into());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("err.network.invalidUrl".into());
    }
    if insecure {
        if !allow_insecure_local {
            return Err("err.network.insecureConfirm".into());
        }
        let host = parsed
            .host_str()
            .expect("host checked above")
            .trim_matches(['[', ']']);
        let ip = host
            .parse::<IpAddr>()
            .map_err(|_| "err.network.localIpOnly")?;
        if !is_local_network_address(ip) {
            return Err("err.network.localIpOnly".into());
        }
    }
    Ok((
        parsed.to_string(),
        scheme == "https" || scheme == "http",
        matches!(scheme, "ftp" | "ftps"),
    ))
}

fn run_osascript_with_timeout(script: &str) -> Result<std::process::Output, String> {
    let mut child = Command::new("/usr/bin/osascript")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "err.mount.failed".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|_| "err.mount.failed".to_string())?;
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if child
            .try_wait()
            .map_err(|_| "err.mount.failed".to_string())?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|_| "err.mount.failed".to_string());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("err.mount.timeout".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[tauri::command]
async fn mount_network_url(url: String, allow_insecure_local: bool) -> Result<String, String> {
    let (url, is_web, is_ftp) = parse_mount_url(&url, allow_insecure_local)?;
    let escaped = escape_for_applescript(&url).map_err(|_| "err.network.badchars".to_string())?;
    let script = format!(
        "tell application \"Finder\" to activate\nmount volume \"{}\"",
        escaped
    );
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let out = run_osascript_with_timeout(&script)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("-3014") {
                if is_web {
                    return Err("err.mount.notWebdav".into());
                }
                if is_ftp {
                    return Err("err.mount.ftp".into());
                }
                return Err("err.mount.unreachable".into());
            }
            if err.contains("-1409") || err.contains("NSURLErrorDomain") {
                return Err("err.mount.unreachable".into());
            }
            return Err("err.mount.failed".into());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    })
    .await
    .map_err(|_| "err.mount.failed".to_string())?
}

/// Gibt die in Cargo.toml gepflegte App-Version zurück (für den Über-Dialog).
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
async fn eject_volume(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let full = std::fs::canonicalize(expand_tilde(&path))
            .map_err(|_| "err.eject.invalidMount".to_string())?;
        if !full.starts_with("/Volumes/") {
            return Err("err.eject.invalidMount".into());
        }
        let path = full.to_string_lossy().into_owned();
        // Netzlaufwerke (WebDAV/SMB/NFS/AFP/FTP) kennt `diskutil eject` nicht
        // ("Failed to find disk"). Sie müssen mit `umount`/`diskutil unmount`
        // ausgehängt werden. Physische Datenträger dagegen mit `eject`.
        let fstype = mount_fs_types().get(&path).cloned().unwrap_or_default();
        let is_network = matches!(
            fstype.as_str(),
            "webdav" | "smbfs" | "nfs" | "afpfs" | "ftp" | "cifs"
        );

        if is_network {
            // Zuerst der saubere Weg über diskutil, dann der normale umount.
            // Kein erzwungenes Aushängen: Es könnte laufende Transfers anderer
            // Programme unterbrechen oder noch nicht geschriebene Daten verlieren.
            let du = Command::new("/usr/sbin/diskutil")
                .args(["unmount", &path])
                .output()
                .map_err(|e| format!("diskutil: {}", e))?;
            if du.status.success() {
                return Ok(());
            }
            let um = Command::new("/sbin/umount")
                .arg(&path)
                .output()
                .map_err(|e| format!("umount: {}", e))?;
            if um.status.success() {
                return Ok(());
            }
            let err = String::from_utf8_lossy(&um.stderr);
            let so = String::from_utf8_lossy(&um.stdout);
            return Err(format!("err.eject.failed\u{1f}{}{}", err.trim(), so.trim()));
        }

        let out = Command::new("/usr/sbin/diskutil")
            .args(["eject", &path])
            .output()
            .map_err(|e| format!("diskutil: {}", e))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let so = String::from_utf8_lossy(&out.stdout);
            return Err(format!("err.eject.failed\u{1f}{}{}", err.trim(), so.trim()));
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn find_dmg_block<'a>(text: &'a str, p: &std::path::Path) -> Option<&'a str> {
    let p_str = p.to_string_lossy().into_owned();
    let canon = std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p_str.clone());
    let basename = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    for block in text.split("================") {
        for l in block.lines() {
            let lt = l.trim();
            if let Some(rest) = lt
                .strip_prefix("image-path")
                .or_else(|| lt.strip_prefix("image-alias"))
            {
                let rest = rest
                    .trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                    .trim();
                if rest == p_str
                    || rest == canon
                    || (!basename.is_empty() && rest.ends_with(&basename))
                {
                    return Some(block);
                }
            }
        }
    }
    None
}

fn extract_mountpoint(block: &str) -> Option<String> {
    for line in block.lines() {
        let toks: Vec<&str> = line
            .split('\t')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect();
        // Mountpoint ist ein Pfad, der nicht mit /dev/ beginnt
        for t in toks.iter().rev() {
            if t.starts_with('/') && !t.starts_with("/dev/") {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn extract_root_device(block: &str) -> Option<String> {
    // Erstes /dev/diskN (ohne sN-Slice)
    for line in block.lines() {
        for tok in line.split('\t') {
            let t = tok.trim();
            if let Some(suffix) = t.strip_prefix("/dev/disk") {
                // root disk: keine 's' Partition
                if suffix.chars().all(|c| c.is_ascii_digit()) {
                    return Some(t.to_string());
                }
            }
        }
    }
    // Fallback: irgendein /dev/disk*
    for line in block.lines() {
        for tok in line.split('\t') {
            let t = tok.trim();
            if t.starts_with("/dev/disk") {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn find_existing_dmg_mount(p: &std::path::Path) -> Option<String> {
    let info = std::process::Command::new("hdiutil")
        .arg("info")
        .output()
        .ok()?;
    if !info.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&info.stdout);
    let block = find_dmg_block(&text, p)?;
    extract_mountpoint(block)
}

#[tauri::command]
async fn detach_dmg(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let p = expand_tilde(&path);
        let info = std::process::Command::new("hdiutil")
            .arg("info")
            .output()
            .map_err(|e| format!("hdiutil: {}", e))?;
        let text = String::from_utf8_lossy(&info.stdout);
        let block =
            find_dmg_block(&text, &p).ok_or_else(|| "Image ist nicht gemountet".to_string())?;
        let dev = extract_root_device(block).ok_or_else(|| "Device nicht gefunden".to_string())?;
        let out = std::process::Command::new("hdiutil")
            .args(["detach", &dev])
            .output()
            .map_err(|e| format!("hdiutil: {}", e))?;
        if !out.status.success() {
            // force-detach versuchen
            let out2 = std::process::Command::new("hdiutil")
                .args(["detach", "-force", &dev])
                .output()
                .map_err(|e| format!("hdiutil: {}", e))?;
            if !out2.status.success() {
                let err = String::from_utf8_lossy(&out2.stderr);
                return Err(format!("hdiutil detach fehlgeschlagen: {}", err.trim()));
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Schreibt eine Liste von Dateipfaden als Datei-Referenzen in die System-Zwischenablage,
/// sodass z. B. Finder sie per Cmd+V einfügen kann.
/// Nutzt AppleScript (osascript), weil das ohne zusätzliche objc-Crates auskommt.
#[tauri::command]
async fn clipboard_write_files(paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let expanded: Vec<String> = paths
            .iter()
            .map(|p| expand_tilde(p).to_string_lossy().to_string())
            .collect();
        promise_drag::clipboard_write_files(expanded)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn clipboard_read_files() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(promise_drag::clipboard_read_files)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn set_dock_badge(label: Option<String>) {
    promise_drag::set_dock_badge(label);
}

/// Schreibt (einmalig) ein 1×1-PNG nach `$TMPDIR/dualbeam_drag.png` und gibt
/// den Pfad zurück. Wird vom Drag-Plugin als Drag-Vorschaubild gebraucht.
#[tauri::command]
fn drag_icon_path() -> Result<String, String> {
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let path = std::env::temp_dir().join("dualbeam_drag.png");
    if !path.exists() {
        std::fs::write(&path, PNG).map_err(|e| e.to_string())?;
    }
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
async fn find_dmg_mount(path: String) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        let p = expand_tilde(&path);
        Ok(find_existing_dmg_mount(&p))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn mount_dmg(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let p = expand_tilde(&path);

        // 1) Prüfen, ob das Image bereits attached ist → bestehenden Mountpunkt zurückgeben
        if let Some(mp) = find_existing_dmg_mount(&p) {
            return Ok(mp);
        }

        // 2) Sonst neu attachen — SLA via stdin "Y\n" akzeptieren
        use std::io::Write;
        let mut child = std::process::Command::new("hdiutil")
            .args(["attach", "-noautoopen", "-noverify"])
            .arg(&p)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("hdiutil: {}", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"Y\n");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("hdiutil: {}", e))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            // Falls "resource busy": evtl. doch schon gemountet — nochmal nachsehen
            if let Some(mp) = find_existing_dmg_mount(&p) {
                return Ok(mp);
            }
            return Err(format!("hdiutil attach fehlgeschlagen: {}", err.trim()));
        }
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut mount: Option<String> = None;
        for line in stdout.lines() {
            for tok in line.split('\t') {
                let t = tok.trim();
                if t.starts_with('/') && !t.starts_with("/dev/") {
                    mount = Some(t.to_string());
                }
            }
        }
        mount.ok_or_else(|| "Mountpunkt nicht gefunden".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn quick_look(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    let s = p.to_string_lossy().to_string();
    promise_drag::quick_look(&[s])
}

// ---------- Jobs (copy / move) ----------

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobItem {
    src: String,
    dst: String,
    overwrite: bool,
}

/// Direkte rsync-over-SSH-Synchronisation. Die Zugangsdaten stammen aus dem
/// Dialog und werden nicht persistent gespeichert.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RsyncRequest {
    job_id: String,
    local_path: String,
    host: String,
    remote_path: String,
    username: String,
    password: String,
    delete_extra: bool,
    exclude_patterns: Vec<String>,
}

fn valid_rsync_username(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_rsync_host(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
}

fn valid_rsync_path(value: &str) -> bool {
    value.starts_with('/')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
        && !value.split('/').any(|part| part == "..")
}

fn rsync_askpass_script() -> Result<PathBuf, String> {
    static NEXT_ASKPASS_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ASKPASS_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "dualbeam-rsync-askpass-{}-{id}.sh",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    // Das Kennwort selbst liegt nur in der Prozessumgebung des Kindprozesses.
    // Das Skript enthält keine Zugangsdaten und wird direkt nach rsync entfernt.
    file.write_all(b"#!/bin/sh\nprintf '%s\\n' \"$DUALBEAM_RSYNC_PASSWORD\"\n")
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn rsync_executable() -> PathBuf {
    // Homebrew installiert die aktuelle rsync-Version auf Apple-Silicon-Macs
    // unter /opt/homebrew. Sie wird bevorzugt, der mit macOS gelieferte Client
    // bleibt als kompatibler Fallback erhalten.
    [
        "/opt/homebrew/bin/rsync",
        "/usr/local/bin/rsync",
        "/usr/bin/rsync",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .unwrap_or_else(|| PathBuf::from("rsync"))
}

const RSYNC_KEYCHAIN_SERVICE: &str = "com.nojan.dualbeam.rsync";

fn rsync_keychain_account(host: &str, username: &str) -> Result<String, String> {
    if !valid_rsync_username(username) || !valid_rsync_host(host) {
        return Err("Ungültiger rsync-Server oder Benutzername".into());
    }
    Ok(format!("{username}@{host}"))
}

#[tauri::command]
fn save_rsync_password(host: String, username: String, password: String) -> Result<(), String> {
    if password.is_empty() {
        return Err("Leeres rsync-Passwort wird nicht gespeichert".into());
    }
    let account = rsync_keychain_account(&host, &username)?;
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(
            RSYNC_KEYCHAIN_SERVICE,
            &account,
            password.as_bytes(),
        )
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        Err("Der macOS-Schlüsselbund ist nur unter macOS verfügbar".into())
    }
}

#[tauri::command]
fn load_rsync_password(host: String, username: String) -> Result<Option<String>, String> {
    let account = rsync_keychain_account(&host, &username)?;
    #[cfg(target_os = "macos")]
    {
        match security_framework::passwords::get_generic_password(RSYNC_KEYCHAIN_SERVICE, &account)
        {
            Ok(password) => String::from_utf8(password)
                .map(Some)
                .map_err(|_| "Ungültiges Kennwort im macOS-Schlüsselbund".to_string()),
            // Ein fehlender Schlüsselbund-Eintrag ist kein Fehler im Dialog.
            Err(_) => Ok(None),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        Err("Der macOS-Schlüsselbund ist nur unter macOS verfügbar".into())
    }
}

fn emit_rsync_status_line(
    app: &AppHandle,
    job_id: &str,
    fallback_current: &str,
    line: &[u8],
    files_done: &mut u64,
) {
    let text = String::from_utf8_lossy(line);
    if let Some(path) = text.trim().strip_prefix("DUALBEAM:") {
        // Nur dieses eigene, zeilenbasierte rsync-Ereignis zählt eine Datei.
        // `to-chk` enthält dagegen die gesamte zu prüfende Baumstruktur und
        // würde fälschlich wie eine Kopiermenge aussehen.
        *files_done += 1;
        let _ = app.emit(
            "job-progress",
            JobProgress {
                job_id: job_id.to_string(),
                done: 0,
                total: 0,
                files_done: *files_done,
                current: if path.is_empty() {
                    fallback_current.to_string()
                } else {
                    path.to_string()
                },
                finished: false,
                cancelled: false,
                error: None,
            },
        );
    }
}

#[cfg(unix)]
fn terminate_rsync_process_group(pid: u32) {
    // rsync startet ssh als Kindprozess. Ein Signal an die eigene Prozessgruppe
    // beendet beides zuverlässig, statt eine offene SSH-Verbindung stehen zu
    // lassen. Fehler sind hier unkritisch: der Prozess kann schon fertig sein.
    unsafe {
        let _ = libc::kill(-(pid as i32), libc::SIGTERM);
    }
}

fn run_rsync_inner(
    app: &AppHandle,
    request: RsyncRequest,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    if !valid_rsync_username(&request.username) {
        return Err("Ungültiger rsync-Benutzername".into());
    }
    if !valid_rsync_host(&request.host) {
        return Err("Ungültiger rsync-Server".into());
    }
    if !valid_rsync_path(&request.remote_path) {
        return Err(
            "Der rsync-Zielpfad muss absolut sein und darf keine '..'-Segmente enthalten".into(),
        );
    }
    if request.password.is_empty() {
        return Err("Für die rsync-Anmeldung ist ein Passwort erforderlich".into());
    }
    let local = expand_tilde(&request.local_path);
    if !local.is_dir() {
        return Err(format!(
            "Lokaler rsync-Quellordner existiert nicht: {}",
            local.display()
        ));
    }
    let askpass = rsync_askpass_script()?;
    let remote = format!(
        "{}@{}:{}",
        request.username, request.host, request.remote_path
    );
    let local_arg = format!("{}/", local.to_string_lossy().trim_end_matches('/'));
    let mut command = Command::new(rsync_executable());
    command
        // Entspricht der von IONOS dokumentierten HiDrive-Empfehlung:
        // rekursiv, Links und Zeiten erhalten, Verzeichnisse übertragen,
        // ausführliche Fehlerausgabe. `-a` wäre hier ungeeignet, weil es
        // zusätzlich Eigentümer, Gruppen und Unix-Rechte setzen möchte.
        .args(["-rltDv", "--partial", "--out-format=DUALBEAM:%n"])
        .arg("-e")
        // Akzeptiert den Hostschlüssel beim allerersten Zugriff und schützt
        // danach weiterhin vor einem geänderten Schlüssel (MITM-Erkennung).
        .arg("/usr/bin/ssh -o StrictHostKeyChecking=accept-new")
        .env("SSH_ASKPASS", &askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "dualbeam:0")
        .env("DUALBEAM_RSYNC_PASSWORD", request.password)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Eigene Prozessgruppe, damit cancel_job rsync und sein ssh-Kind
        // gemeinsam beenden kann.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    if request.delete_extra {
        command.arg("--delete");
    }
    for pattern in request.exclude_patterns {
        // Als separates Argument übergeben; rsync interpretiert die Regel,
        // nicht eine Shell. Damit bleiben gespeicherte Ausschlussmuster wie
        // `node_modules/` und `*.log` ohne Shell-Injection nutzbar.
        command.arg("--exclude").arg(pattern);
    }
    command.arg(local_arg).arg(&remote);
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "rsync ist auf diesem Mac nicht verfügbar".to_string()
        } else {
            e.to_string()
        }
    })?;
    let pid = child.id();
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.rsync_pids).insert(request.job_id.clone(), pid);
    }
    let progress_current = format!("rsync: {remote}");
    let _ = app.emit(
        "job-progress",
        JobProgress {
            job_id: request.job_id.clone(),
            done: 0,
            total: 0,
            files_done: 0,
            current: progress_current.clone(),
            finished: false,
            cancelled: false,
            error: None,
        },
    );
    if cancel.load(Ordering::SeqCst) {
        #[cfg(unix)]
        terminate_rsync_process_group(pid);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "rsync-Ausgabe konnte nicht gelesen werden".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "rsync-Fehlerausgabe konnte nicht gelesen werden".to_string())?;
    let progress_app = app.clone();
    let progress_job_id = request.job_id.clone();
    let stdout_reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        let mut collected = Vec::new();
        let mut files_done = 0;
        let mut byte = [0_u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) if byte[0] == b'\r' || byte[0] == b'\n' => {
                    if !line.is_empty() {
                        collected.extend_from_slice(&line);
                        collected.push(byte[0]);
                        emit_rsync_status_line(
                            &progress_app,
                            &progress_job_id,
                            &progress_current,
                            &line,
                            &mut files_done,
                        );
                        line.clear();
                    }
                }
                Ok(_) => line.push(byte[0]),
            }
        }
        if !line.is_empty() {
            collected.extend_from_slice(&line);
            emit_rsync_status_line(
                &progress_app,
                &progress_job_id,
                &progress_current,
                &line,
                &mut files_done,
            );
        }
        collected
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_end(&mut text);
        text
    });
    let status = child.wait().map_err(|e| e.to_string());
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.rsync_pids).remove(&request.job_id);
    }
    let _ = std::fs::remove_file(&askpass);
    if cancel.load(Ordering::SeqCst) {
        return Err("err.rsyncCancelled".into());
    }
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&stdout).trim().to_owned();
        Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("rsync wurde mit Status {:?} beendet", status.code())
        })
    }
}

#[tauri::command]
async fn run_rsync(app: AppHandle, request: RsyncRequest) -> Result<(), String> {
    if request.job_id.is_empty() {
        return Err("Ungültige rsync-Jobkennung".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).insert(request.job_id.clone(), cancel.clone());
    }
    let job_id = request.job_id.clone();
    let app_for_worker = app.clone();
    let cancel_for_worker = cancel.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_rsync_inner(&app_for_worker, request, &cancel_for_worker)
    })
    .await
    .map_err(|e| e.to_string())?;
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).remove(&job_id);
        lock_safe(&mgr.rsync_pids).remove(&job_id);
    }
    let _ = app.emit(
        "job-progress",
        JobProgress {
            job_id,
            done: 0,
            total: 0,
            files_done: 0,
            current: String::new(),
            finished: true,
            cancelled: cancel.load(Ordering::SeqCst),
            error: result.as_ref().err().cloned(),
        },
    );
    result
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobProgress {
    job_id: String,
    done: u64,
    total: u64,
    files_done: u64,
    current: String,
    finished: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct JobManager {
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    rsync_pids: Mutex<HashMap<String, u32>>,
}

#[tauri::command]
fn check_conflicts(items: Vec<JobItem>) -> Vec<String> {
    items
        .iter()
        .filter(|i| expand_tilde(&i.dst).exists())
        .map(|i| i.dst.clone())
        .collect()
}

struct JobCtx<'a> {
    app: &'a AppHandle,
    job_id: &'a str,
    cancel: &'a Arc<AtomicBool>,
    done: u64,
    total: u64,
    /// Anzahl tatsächlich kopierter Dateien. Sie läuft auch innerhalb eines
    /// einzelnen Sync-Ordners weiter, während `done` bewusst nur die
    /// Vorschau-Einträge zählt.
    files_done: u64,
    /// Verschachtelungstiefe beim Dereferenzieren von Symlinks (Schleifenschutz,
    /// falls das Ziel-Dateisystem keine Symlinks unterstützt).
    deref_depth: u32,
    /// Dateinamen können sich beim rekursiven Kopieren sehr schnell ändern.
    /// Die UI (und insbesondere die Dock-Markierung) darf dadurch nicht mit
    /// hunderten nativen Aktualisierungen pro Sekunde belastet werden.
    last_emit: Cell<Instant>,
    last_reported_done: Cell<u64>,
}

impl<'a> JobCtx<'a> {
    fn emit(&self, current: &str) {
        const MIN_PROGRESS_INTERVAL: Duration = Duration::from_millis(125);
        let now = Instant::now();
        // Ein echter Fortschrittsschritt muss sofort sichtbar werden. Reine
        // Dateinamenwechsel innerhalb desselben Schritts werden gedrosselt.
        if self.last_reported_done.get() == self.done
            && now.duration_since(self.last_emit.get()) < MIN_PROGRESS_INTERVAL
        {
            return;
        }
        self.last_emit.set(now);
        self.last_reported_done.set(self.done);
        // Sicherheitsnetz: `done` darf nie größer als `total` angezeigt werden.
        // Der Fortschritt zählt jetzt Einträge (siehe run_job), sodass dies im
        // Normalfall nicht eintritt.
        let total = self.total.max(self.done);
        let _ = self.app.emit(
            "job-progress",
            JobProgress {
                job_id: self.job_id.to_string(),
                done: self.done,
                total,
                files_done: self.files_done,
                current: current.to_string(),
                finished: false,
                cancelled: false,
                error: None,
            },
        );
    }
}

fn remove_path(p: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        // Bereits weg (z. B. verwaiste AppleDouble-Datei, die das Netzlaufwerk
        // zwischenzeitlich selbst entfernt hat) → Ziel „gelöscht" ist erreicht.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let res = if meta.is_dir() && !meta.file_type().is_symlink() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    match res {
        Ok(()) => Ok(()),
        // WebDAV/SMB liefern Verzeichnis-Listings aus einem veralteten Cache:
        // `stat` meldet die Datei noch als vorhanden, das eigentliche Löschen
        // scheitert dann aber mit ENOENT, weil sie (z. B. über die IONOS
        // Web-GUI) längst entfernt wurde. Das Ziel „nicht mehr vorhanden" ist
        // damit erreicht – als Erfolg werten, nicht als Fehler abbrechen.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Sockets, FIFOs und Geräte sind keine kopierbaren Dateien. Dazu zählt etwa
/// Gits lokaler File-Monitor-Socket `.git/fsmonitor--daemon.ipc`: `copyfile`
/// kann ihn nicht lesen und bricht mit EOPNOTSUPP ab. Symlinks bleiben bewusst
/// zulässig, weil sie separat behandelt bzw. auf Netzlaufwerken dereferenziert
/// werden können.
fn is_untransferable_file(meta: &std::fs::Metadata) -> bool {
    let ty = meta.file_type();
    !ty.is_file() && !ty.is_dir() && !ty.is_symlink()
}

/// Löst auch noch nicht existierende Zielpfade soweit wie möglich auf. Damit
/// werden Symlinks in vorhandenen Elternordnern berücksichtigt, bevor geprüft
/// wird, ob ein Ordner in sich selbst kopiert werden soll.
fn canonicalize_target_path(path: &Path) -> std::io::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        match std::fs::canonicalize(current) {
            Ok(mut base) => {
                for component in missing.iter().rev() {
                    base.push(component);
                }
                return Ok(base);
            }
            Err(_) => {
                let name = current.file_name().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("ungültiger Zielpfad: {}", path.display()),
                    )
                })?;
                missing.push(name.to_os_string());
                current = current.parent().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("ungültiger Zielpfad: {}", path.display()),
                    )
                })?;
            }
        }
    }
}

/// Ein Verzeichnis darf nicht in sich selbst oder einen seiner Unterordner
/// kopiert werden. Sonst würde der rekursive Kopierer den neuen Zielbaum beim
/// weiteren Durchlaufen der Quelle erneut als Eingabe finden.
fn destination_is_within_source(src: &Path, dst: &Path) -> std::io::Result<bool> {
    // Symlinks werden als Link kopiert; sie werden nicht rekursiv durchlaufen.
    // Zwei Arbeitskopien können (wie Trunk) auf dasselbe Cache-Verzeichnis
    // verlinken, ohne dass dadurch eine Selbstkopie entsteht.
    let link_meta = match std::fs::symlink_metadata(src) {
        Ok(meta) => meta,
        // Zwischen Vorschau und Ausführung können temporäre Dateien (etwa
        // Git-Referenzen oder Editor-Backups) bereits verschwunden sein. Für
        // einen nicht mehr vorhandenen Quellpfad gibt es keine Selbstkopie.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if link_meta.file_type().is_symlink() || !link_meta.is_dir() {
        return Ok(false);
    }
    let source = match std::fs::canonicalize(src) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let target = canonicalize_target_path(dst)?;
    Ok(target == source || target.starts_with(source))
}

#[cfg(target_os = "macos")]
fn copy_file_with_metadata(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn copyfile(
            from: *const libc::c_char,
            to: *const libc::c_char,
            state: *mut libc::c_void,
            flags: u32,
        ) -> libc::c_int;
    }
    const COPYFILE_ACL: u32 = 1 << 0;
    const COPYFILE_STAT: u32 = 1 << 1;
    const COPYFILE_XATTR: u32 = 1 << 2;
    const COPYFILE_DATA: u32 = 1 << 3;
    const COPYFILE_ALL: u32 = COPYFILE_ACL | COPYFILE_STAT | COPYFILE_XATTR | COPYFILE_DATA;

    let s = CString::new(src.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let d = CString::new(dst.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let call = |flags: u32| -> std::io::Result<()> {
        let ret = unsafe { copyfile(s.as_ptr(), d.as_ptr(), std::ptr::null_mut(), flags) };
        if ret != 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };

    // Manche Ziele unterstützen ACLs oder erweiterte Attribute nicht. Zudem
    // verweigert macOS für bestimmte, aus Downloads stammende Metadaten (etwa
    // `com.apple.provenance`) das Setzen mit EPERM. In beiden Fällen bleiben
    // die Nutzdaten kopierbar, nur die Metadaten müssen ausgelassen werden.
    // Wir degradieren deshalb schrittweise: erst ACL/xattr weglassen (Daten +
    // Zeitstempel/Rechte), zuletzt reine Datenkopie. Ein echter Lese- oder
    // Schreibfehler wird dabei nicht verschluckt: `std::fs::copy` schlägt dann
    // ebenfalls fehl und wird weitergereicht.
    let is_metadata_unsupported = |e: &std::io::Error| -> bool {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP) | Some(libc::EPERM)
        )
    };

    match call(COPYFILE_ALL) {
        Ok(()) => return Ok(()),
        Err(e) if is_metadata_unsupported(&e) => {}
        Err(e) => return Err(e),
    }

    // Bei erneutem Versuch eine ggf. teilweise erzeugte Zieldatei entfernen,
    // damit copyfile frisch schreiben kann.
    let _ = std::fs::remove_file(dst);
    match call(COPYFILE_DATA | COPYFILE_STAT) {
        Ok(()) => return Ok(()),
        Err(e) if is_metadata_unsupported(&e) => {}
        Err(e) => return Err(e),
    }

    // Letzter Fallback: reine Datenkopie (kopiert auch die Rechte-Bits).
    let _ = std::fs::remove_file(dst);
    std::fs::copy(src, dst).map(|_| ())
}

#[cfg(not(target_os = "macos"))]
fn copy_file_with_metadata(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst).map(|_| ())
}

/// Prüft, ob ein Fehler „Operation nicht unterstützt" (ENOTSUP/EOPNOTSUPP,
/// macOS „os error 45") ist – typisch für WebDAV/SMB/FAT bei Symlinks/ACLs/xattr.
#[cfg(unix)]
fn is_enotsup(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP)
    )
}

/// Prüft, ob ein Fehler vorübergehend/transient ist – typisch für langsame
/// Netzlaufwerke (WebDAV/HiDrive, SMB), die einzelne Operationen mit Timeout
/// (ETIMEDOUT / macOS „os error 60") oder Verbindungsabbrüchen quittieren.
/// Solche Fehler können durch einen erneuten Versuch verschwinden.
#[cfg(unix)]
fn is_transient(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ETIMEDOUT)
            | Some(libc::ECONNRESET)
            | Some(libc::ECONNABORTED)
            | Some(libc::EPIPE)
            | Some(libc::EAGAIN)
            | Some(libc::ENETRESET)
            | Some(libc::ENETDOWN)
            | Some(libc::ENETUNREACH)
            | Some(libc::EHOSTDOWN)
            | Some(libc::EHOSTUNREACH)
            | Some(libc::EINTR)
    )
}

#[cfg(not(unix))]
fn is_transient(_e: &std::io::Error) -> bool {
    false
}

/// Kopiert eine Datei und wiederholt den Versuch bei transienten Netzwerk-
/// fehlern (z. B. os error 60 „Operation timed out" auf HiDrive/WebDAV) mit
/// exponentiellem Backoff. Bricht sofort ab, wenn der Job abgebrochen wurde
/// oder ein nicht-transienter Fehler auftritt.
fn copy_file_retry(src: &Path, dst: &Path, cancel: &AtomicBool) -> std::io::Result<()> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut attempt: u32 = 0;
    loop {
        match copy_file_with_metadata(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) => {
                attempt += 1;
                if !is_transient(&e) || attempt >= MAX_ATTEMPTS || cancel.load(Ordering::SeqCst) {
                    return Err(e);
                }
                // Teilweise geschriebene Zieldatei entfernen, damit der nächste
                // Versuch frisch schreiben kann.
                let _ = std::fs::remove_file(dst);
                // Backoff: 0,5s → 1s → 2s → 4s. Abbruchfreundlich in 100ms-Schritten warten.
                let backoff =
                    std::time::Duration::from_millis(500u64.saturating_mul(1u64 << (attempt - 1)));
                let step = std::time::Duration::from_millis(100);
                let mut waited = std::time::Duration::ZERO;
                while waited < backoff {
                    if cancel.load(Ordering::SeqCst) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "cancelled",
                        ));
                    }
                    std::thread::sleep(step);
                    waited += step;
                }
            }
        }
    }
}

/// Ersetzt eine vorhandene Datei erst, nachdem die neue Version vollständig in
/// eine temporäre Nachbardatei kopiert wurde. Insbesondere WebDAV-Mounts
/// quittieren das vorzeitige Löschen einer offenen/gecachten Zieldatei
/// gelegentlich mit EPERM, obwohl das Hochladen einer neuen Datei erlaubt ist.
/// Ein Rename innerhalb desselben Verzeichnisses entspricht einem WebDAV MOVE
/// und vermeidet diesen fehleranfälligen Zwischenzustand.
fn replace_file_after_copy(src: &Path, dst: &Path, cancel: &AtomicBool) -> std::io::Result<()> {
    static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);
    let parent = dst.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("ungültiger Zielpfad: {}", dst.display()),
        )
    })?;
    let name = dst
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("ungültiger Zieldateiname: {}", dst.display()),
            )
        })?;
    let id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(".{name}.dualbeam-{id}.inprogress"));

    copy_file_retry(src, &temp, cancel)?;
    match std::fs::rename(&temp, dst) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            // Manche WebDAV-Server erlauben MOVE nur ohne vorhandenes Ziel.
            // Erst nachdem der Upload erfolgreich war, ist das Entfernen des
            // alten Ziels als Fallback sicher.
            if let Err(remove_error) = remove_path(dst) {
                let _ = std::fs::remove_file(&temp);
                return Err(std::io::Error::new(
                    remove_error.kind(),
                    format!(
                        "Ziel konnte nach dem Upload nicht ersetzt werden ({rename_error}; {remove_error})"
                    ),
                ));
            }
            if let Err(error) = std::fs::rename(&temp, dst) {
                let _ = std::fs::remove_file(&temp);
                return Err(error);
            }
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyOutcome {
    Copied,
    Skipped,
}

fn remove_source_after_move(src: &Path, outcome: CopyOutcome) -> Result<(), String> {
    if outcome == CopyOutcome::Skipped {
        return Err(format!(
            "{}: Verschieben abgebrochen, weil nicht alle Einträge kopiert wurden; die Quelle wurde nicht gelöscht",
            src.display()
        ));
    }
    remove_path(src).map_err(|e| {
        format!(
            "{}: Quelle wurde kopiert, konnte aber nicht gelöscht werden: {}",
            src.display(),
            e
        )
    })
}

fn copy_recursive(
    src: &Path,
    dst: &Path,
    overwrite: bool,
    ctx: &mut JobCtx,
) -> std::io::Result<CopyOutcome> {
    if ctx.cancel.load(Ordering::SeqCst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ));
    }
    let meta = match std::fs::symlink_metadata(src) {
        Ok(meta) => meta,
        // Die Synchronisationsvorschau ist nur eine Momentaufnahme. Wenn ein
        // Quellobjekt anschließend verschwindet, ist Überspringen korrekt und
        // verhindert, dass ein flüchtiges Git-/Tool-Artefakt den ganzen Job
        // abbricht. Bei Verschiebe-Jobs sorgt `Skipped` weiterhin dafür, dass
        // keine verbliebene Quelle gelöscht wird.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CopyOutcome::Skipped),
        Err(e) => return Err(e),
    };
    if is_untransferable_file(&meta) {
        return Ok(CopyOutcome::Skipped);
    }
    if meta.file_type().is_symlink() {
        if dst.exists() {
            if overwrite {
                remove_path(dst)?;
            } else {
                return Ok(CopyOutcome::Skipped);
            }
        }
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        {
            match std::os::unix::fs::symlink(&target, dst) {
                Ok(()) => {
                    ctx.files_done += 1;
                    ctx.emit(&src.to_string_lossy());
                    Ok(CopyOutcome::Copied)
                }
                Err(e) if is_enotsup(&e) => {
                    // Ziel-Dateisystem (WebDAV/SMB/FAT …) unterstützt keine Symlinks
                    // (ENOTSUP / os error 45). Statt abzubrechen dereferenzieren wir:
                    // dem Link folgen und das Ziel real kopieren, damit z. B.
                    // .app-Bundles (Frameworks mit Versions-Symlinks) nutzbar bleiben.
                    // Tiefenbegrenzung schützt vor Symlink-Schleifen.
                    match std::fs::metadata(src) {
                        Ok(tmeta) => {
                            if ctx.deref_depth >= 64 {
                                return Err(std::io::Error::other(
                                    "Symlink-Schleife oder zu tiefe Verschachtelung beim Dereferenzieren",
                                ));
                            }
                            ctx.deref_depth += 1;
                            let res: std::io::Result<CopyOutcome> = if tmeta.is_dir() {
                                // read_dir folgt dem Symlink und liest das Zielverzeichnis.
                                // Fortschritt zählen die Kind-Kopien selbst.
                                if !dst.exists() {
                                    std::fs::create_dir_all(dst)?;
                                }
                                (|| {
                                    let mut outcome = CopyOutcome::Copied;
                                    for entry in std::fs::read_dir(src)? {
                                        let entry = entry?;
                                        let from = entry.path();
                                        let to = dst.join(entry.file_name());
                                        if copy_recursive(&from, &to, overwrite, ctx)?
                                            == CopyOutcome::Skipped
                                        {
                                            outcome = CopyOutcome::Skipped;
                                        }
                                    }
                                    Ok(outcome)
                                })()
                            } else {
                                // copyfile folgt dem Symlink und kopiert die Zieldaten.
                                copy_file_retry(src, dst, ctx.cancel).map(|_| {
                                    ctx.files_done += 1;
                                    ctx.emit(&src.to_string_lossy());
                                    CopyOutcome::Copied
                                })
                            };
                            ctx.deref_depth -= 1;
                            res
                        }
                        Err(_) => {
                            // Defekter (dangling) Symlink: auf einem FS ohne Symlink-
                            // Unterstützung nicht abbildbar → überspringen statt abbrechen.
                            ctx.emit(&src.to_string_lossy());
                            Ok(CopyOutcome::Skipped)
                        }
                    }
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = &target;
            ctx.emit(&src.to_string_lossy());
            Ok(CopyOutcome::Copied)
        }
    } else if meta.is_dir() {
        if !dst.exists() {
            std::fs::create_dir_all(dst)?;
        } else if !dst.is_dir() {
            if overwrite {
                std::fs::remove_file(dst)?;
                std::fs::create_dir_all(dst)?;
            } else {
                return Ok(CopyOutcome::Skipped);
            }
        }
        let mut outcome = CopyOutcome::Copied;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            let child_outcome = copy_recursive(&from, &to, overwrite, ctx)
                .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", from.display())))?;
            if child_outcome == CopyOutcome::Skipped {
                outcome = CopyOutcome::Skipped;
            }
        }
        Ok(outcome)
    } else {
        let replacing = dst.exists();
        if replacing && !overwrite {
            return Ok(CopyOutcome::Skipped);
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if replacing {
            replace_file_after_copy(src, dst, ctx.cancel)?;
        } else {
            copy_file_retry(src, dst, ctx.cancel)?;
        }
        ctx.files_done += 1;
        ctx.emit(&src.to_string_lossy());
        Ok(CopyOutcome::Copied)
    }
}

#[tauri::command]
async fn run_job(
    app: AppHandle,
    job_id: String,
    kind: String,
    items: Vec<JobItem>,
) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).insert(job_id.clone(), cancel.clone());
    }

    let app2 = app.clone();
    let job_id2 = job_id.clone();
    let cancel2 = cancel.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        // Fortschritt zählt EINTRÄGE (wie im Sync-Dialog: Neu/Geändert), nicht
        // einzelne Dateien. Ein neuer Ordner-Teilbaum ist im Dialog EIN Eintrag,
        // wird aber beim Kopieren rekursiv (inkl. dereferenzierter Symlinks auf
        // Netzlaufwerken) durchlaufen. Würde der Fortschritt Dateien zählen,
        // stünde in der Statusleiste eine viel höhere Zahl als im Dialog. Pro
        // Eintrag wird `done` daher genau einmal erhöht; der aktuelle Dateiname
        // wird zur Rückmeldung weiter pro Datei ausgegeben.
        let total: u64 = items.len() as u64;
        let mut ctx = JobCtx {
            app: &app2,
            job_id: &job_id2,
            cancel: &cancel2,
            done: 0,
            total,
            files_done: 0,
            deref_depth: 0,
            last_emit: Cell::new(Instant::now()),
            last_reported_done: Cell::new(u64::MAX),
        };
        ctx.emit("");
        for it in &items {
            if cancel2.load(Ordering::SeqCst) {
                break;
            }
            let src = expand_tilde(&it.src);
            let dst = expand_tilde(&it.dst);
            if destination_is_within_source(&src, &dst)
                .map_err(|e| format!("{}: Zielpfad prüfen fehlgeschlagen: {}", src.display(), e))?
            {
                return Err(format!(
                    "{}: Ziel {} liegt innerhalb der Quelle",
                    src.display(),
                    dst.display()
                ));
            }
            let is_move = kind == "move";
            let mut handled = false;
            if is_move && !dst.exists() {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::rename(&src, &dst).is_ok() {
                    ctx.done += 1;
                    ctx.emit(&it.src);
                    handled = true;
                }
            }
            if !handled {
                match copy_recursive(&src, &dst, it.overwrite, &mut ctx) {
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::Interrupted {
                            return Err(format!("{}: {}", src.display(), e));
                        }
                    }
                    Ok(outcome) => {
                        if is_move {
                            remove_source_after_move(&src, outcome)?;
                        }
                        ctx.done += 1;
                        ctx.emit(&it.src);
                    }
                }
            }
        }
        Ok(())
    });

    let res = join.await.map_err(|e| e.to_string())?;
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).remove(&job_id);
    }
    let cancelled = cancel.load(Ordering::SeqCst);
    let error = res.as_ref().err().cloned();
    let _ = app.emit(
        "job-progress",
        JobProgress {
            job_id: job_id.clone(),
            done: 0,
            total: 0,
            files_done: 0,
            current: String::new(),
            finished: true,
            cancelled,
            error,
        },
    );
    res
}

#[tauri::command]
fn cancel_job(app: AppHandle, job_id: String) {
    let mgr: State<JobManager> = app.state();
    let cancel = lock_safe(&mgr.cancels).get(&job_id).cloned();
    let rsync_pid = lock_safe(&mgr.rsync_pids).get(&job_id).copied();
    if let Some(c) = cancel {
        c.store(true, Ordering::SeqCst);
    }
    #[cfg(unix)]
    if let Some(pid) = rsync_pid {
        terminate_rsync_process_group(pid);
    }
}

// ---------- Verzeichnis-Synchronisation ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncEntry {
    /// Relativer Pfad innerhalb des Quell-/Ziel-Verzeichnisses.
    rel: String,
    /// "copy" (neu), "update" (geändert) oder "delete" (nur im Ziel vorhanden).
    action: String,
    is_dir: bool,
    size: u64,
}

fn file_mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Aktuelle Wanduhrzeit in Sekunden seit Epoch.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(i64::MAX)
}

/// Effektive Quell-mtime für den Änderungsvergleich. Ein Zeitstempel in der
/// Zukunft ist unglaubwürdig (eine Datei kann nicht „in der Zukunft" geändert
/// worden sein) – z. B. durch fehlerhafte Archiv-Entpackung oder Tools, die
/// falsche Daten setzen (real beobachtet: `DEPLOYMENT.md` mit Jahr 2076). Ohne
/// Kappung gilt eine solche Datei bei gleicher Größe bei JEDEM Sync fälschlich
/// als „geändert", weil das Ziel beim Upload stets das aktuelle Datum erhält
/// und damit immer „älter" als die Zukunft ist. Daher auf jetzt begrenzen.
fn effective_src_mtime_secs(meta: &std::fs::Metadata) -> i64 {
    file_mtime_secs(meta).min(now_secs())
}

/// Toleranz für den mtime-Vergleich. Netzlaufwerke (WebDAV/HiDrive) und FAT
/// speichern Änderungszeiten nur grob (FAT: 2s) bzw. setzen beim Upload eine
/// eigene Zeit. Ohne Toleranz würden gleichnamige Dateien sonst bei jedem
/// Durchlauf fälschlich als „geändert" erscheinen.
const MTIME_TOLERANCE_SECS: i64 = 2;

/// Vergleicht zwei reguläre Dateien in festen Blöcken per SHA-256. Fehler beim
/// Lesen gelten bewusst als „ungleich“, damit eine angeforderte Verifikation
/// niemals stillschweigend eine abweichende Datei als identisch einstuft.
fn files_match_sha256(left: &Path, right: &Path) -> bool {
    fn hash(path: &Path) -> std::io::Result<[u8; 32]> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 128 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hasher.finalize().into())
    }
    match (hash(left), hash(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// AppleDouble-Begleitdatei (`._X`)? Auf Dateisystemen ohne nativen xattr-
/// Support (WebDAV/HiDrive, SMB, FAT) legt macOS für jede Datei `X` mit
/// erweiterten Attributen/Resource-Fork eine sichtbare Datei `._X` an.
fn is_apple_double_name(name: &str) -> bool {
    name.starts_with("._")
}

/// macOS-Metadaten-Artefakt (`._X` oder `.DS_Store`)? Diese Dateien werden vom
/// System erzeugt/verwaltet und erscheinen nur auf Netzlaufwerken als sichtbare
/// Dateien. Auf der lokalen APFS-Quelle existieren sie nicht (xattrs liegen
/// inline). Sie dürfen in der Sync-Vorschau daher weder als „neu"/„geändert"
/// noch – für sich allein – als „zu löschen" gewertet werden; sonst entstehen
/// massenhaft falsche Einträge (z. B. 1000+ `._`-Dateien in .app-Bundles).
fn is_os_metadata_name(name: &str) -> bool {
    name == ".DS_Store" || is_apple_double_name(name)
}

/// Trunk legt diese Unterordner als kurzlebigen Tool-Cache und Laufzeitstatus
/// an. Sie enthalten keine Projektquellen und ändern sich fortlaufend. Alle
/// anderen versteckten Dateien (auch `.git` und `.trunk`-Konfigurationen)
/// bleiben ausdrücklich Teil der Synchronisation.
fn is_transient_trunk_path(rel: &Path) -> bool {
    let mut components = rel.components();
    if components.next().and_then(|part| part.as_os_str().to_str()) != Some(".trunk") {
        return false;
    }
    matches!(
        components.next().and_then(|part| part.as_os_str().to_str()),
        Some("tools" | "out" | "plugins" | "logs" | "actions" | "notifications")
    )
}

/// Der Wurzelordner kann eigene Konfigurationsdateien enthalten. Er wird bei
/// einem fehlenden Ziel daher einzeln durchlaufen, nicht pauschal kopiert.
fn is_trunk_root(rel: &Path) -> bool {
    let mut components = rel.components();
    components.next().and_then(|part| part.as_os_str().to_str()) == Some(".trunk")
        && components.next().is_none()
}

/// Liest `.dualbeamignore` aus der Quelle und ergänzt die optionalen Regeln
/// eines gespeicherten Sync-Profils. Leere Zeilen und `#`-Kommentare werden
/// ignoriert; Muster beziehen sich immer auf den relativen Pfad im Sync-Root.
fn sync_ignore_patterns(src_root: &Path, extra: Vec<String>) -> Vec<String> {
    let mut patterns = extra;
    if let Ok(text) = std::fs::read_to_string(src_root.join(".dualbeamignore")) {
        patterns.extend(text.lines().map(str::to_owned));
    }
    patterns
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// Prüft einfache Gitignore-ähnliche Regeln. Ein Muster ohne `/` gilt für
/// jeden Pfadbestandteil (`*.log`, `node_modules`); ein Muster mit `/` für den
/// gesamten relativen Pfad. Ein abschließendes `/` schließt den Teilbaum aus.
fn is_ignored_sync_path(rel: &Path, patterns: &[String]) -> bool {
    let rel = rel.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|raw| {
        let directory_rule = raw.ends_with('/');
        let pattern = raw.trim_start_matches("./").trim_end_matches('/');
        if pattern.is_empty() || raw.starts_with('!') {
            return false;
        }
        if directory_rule && (rel == pattern || rel.starts_with(&format!("{pattern}/"))) {
            return true;
        }
        let pat_chars: Vec<char> = pattern.chars().collect();
        if pattern.contains('/') {
            return glob_match(&pat_chars, &rel.chars().collect::<Vec<_>>());
        }
        rel.split('/')
            .any(|component| glob_match(&pat_chars, &component.chars().collect::<Vec<_>>()))
    })
}

/// `symlink_metadata` mit Wiederholung bei transienten Netzwerkfehlern
/// (Timeouts o. Ä.). „Nicht vorhanden" (NotFound) wird sofort zurückgegeben
/// und NICHT als transienter Fehler behandelt.
fn symlink_metadata_retry(path: &Path) -> std::io::Result<std::fs::Metadata> {
    let mut attempt: u32 = 0;
    loop {
        match std::fs::symlink_metadata(path) {
            Ok(m) => return Ok(m),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Err(e);
                }
                attempt += 1;
                if !is_transient(&e) || attempt >= 4 {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(300u64 * attempt as u64));
            }
        }
    }
}

/// Liest ein Verzeichnis vollständig ein und wiederholt bei transienten
/// Netzwerkfehlern. Wichtig, damit die Löschvorschau auf langsamen Laufwerken
/// nicht durch übersprungene Einträge falsche/schwankende Zahlen liefert.
fn read_dir_retry(path: &Path) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut attempt: u32 = 0;
    loop {
        let res = std::fs::read_dir(path)
            .and_then(|rd| rd.collect::<std::io::Result<Vec<std::fs::DirEntry>>>());
        match res {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if !is_transient(&e) || attempt >= 4 {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(300u64 * attempt as u64));
            }
        }
    }
}

/// Vergleicht eine Quell-Datei/-Symlink mit dem Ziel und hängt ggf. einen
/// copy/update-Eintrag an. Transiente Netzwerkfehler beim Lesen der Ziel-
/// Metadaten führen zum Abbruch (Err), damit keine falschen „Neu"-Einträge
/// entstehen.
fn preview_compare_file(
    rel_str: String,
    src_path: &Path,
    dst_path: &Path,
    link_meta: &std::fs::Metadata,
    verify_checksums: bool,
    out: &mut Vec<SyncEntry>,
) -> Result<(), String> {
    // Effektive Quell-Metadaten bestimmen: Symlinks folgen, um mit einem ggf.
    // dereferenzierten Ziel (Netzlaufwerk ohne Symlink-Support) zu vergleichen.
    let is_symlink = link_meta.file_type().is_symlink();
    let followed = if is_symlink {
        std::fs::metadata(src_path).ok()
    } else {
        Some(link_meta.clone())
    };

    let dmeta = match symlink_metadata_retry(dst_path) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Ziel-Metadaten lesen fehlgeschlagen: {e}")),
    };

    match (followed, dmeta) {
        // Ziel fehlt → neu kopieren.
        (Some(f), None) => {
            out.push(SyncEntry {
                rel: rel_str,
                action: "copy".into(),
                is_dir: f.is_dir(),
                size: if f.is_dir() { 0 } else { f.len() },
            });
        }
        // Defekter (dangling) Quell-Symlink, Ziel fehlt → als Symlink kopieren.
        (None, None) => {
            out.push(SyncEntry {
                rel: rel_str,
                action: "copy".into(),
                is_dir: false,
                size: link_meta.len(),
            });
        }
        // Ziel vorhanden → auf Änderung prüfen.
        (Some(f), Some(d)) => {
            if f.is_dir() {
                // Quelle ist (Symlink auf) Verzeichnis. Ist das Ziel ebenfalls ein
                // Verzeichnis (real oder dereferenziert), gilt es als vorhanden –
                // die eigentlichen Kinder werden über ihre realen Pfade erfasst.
                if !d.is_dir() {
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "update".into(),
                        is_dir: false,
                        size: 0,
                    });
                }
            } else if is_symlink && d.file_type().is_symlink() {
                // Beide Symlinks: über das Linkziel vergleichen.
                if std::fs::read_link(src_path).ok() != std::fs::read_link(dst_path).ok() {
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "update".into(),
                        is_dir: false,
                        size: f.len(),
                    });
                }
            } else {
                // Datei-Vergleich: Größe oder (deutlich) neuere Quelle. Die
                // Quell-mtime wird auf „jetzt" gekappt, damit zukunftsdatierte
                // Dateien nicht bei jedem Sync als „geändert" erscheinen.
                let metadata_differs = f.len() != d.len()
                    || effective_src_mtime_secs(&f) > file_mtime_secs(&d) + MTIME_TOLERANCE_SECS;
                // Die Prüfsummenprüfung ist nur für Dateien nötig, die der
                // schnelle Metadatenvergleich als gleich einstuft. Bei einer
                // anderen Größe oder eindeutig neuerer Quelle steht das
                // Ergebnis bereits fest. Die Dateien zusätzlich komplett zu
                // lesen war besonders auf WebDAV/SMB extrem teuer und hat die
                // Vorschau unnötig lange blockiert.
                let checksum_differs = verify_checksums
                    && !metadata_differs
                    && !files_match_sha256(src_path, dst_path);
                if metadata_differs || checksum_differs {
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "update".into(),
                        is_dir: false,
                        size: f.len(),
                    });
                }
            }
        }
        // Dangling-Symlink, aber Ziel existiert → nichts zu tun.
        (None, Some(_)) => {}
    }
    Ok(())
}

/// Läuft die Quelle rekursiv ab und sammelt copy/update-Einträge. Folgt keinen
/// Symlink-Verzeichnissen (deren Inhalte liegen unter den realen Pfaden).
fn preview_walk_src(
    src_root: &Path,
    dst_root: &Path,
    cur: &Path,
    ignore_patterns: &[String],
    verify_checksums: bool,
    out: &mut Vec<SyncEntry>,
) -> Result<(), String> {
    let entries = read_dir_retry(cur).map_err(|e| format!("Quelle lesen fehlgeschlagen: {e}"))?;
    for entry in entries {
        let p = entry.path();
        let rel = match p.strip_prefix(src_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_transient_trunk_path(rel) || is_ignored_sync_path(rel, ignore_patterns) {
            continue;
        }
        // macOS-Metadaten (._X, .DS_Store) nicht kopieren – sie werden auf dem
        // Ziel (falls nötig) vom System selbst erzeugt.
        if is_os_metadata_name(&name) {
            continue;
        }
        let rel_str = rel.to_string_lossy().into_owned();
        let dst_path = dst_root.join(rel);
        let link_meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Lokale IPC-Sockets (z. B. `.git/fsmonitor--daemon.ipc`), FIFOs und
        // Geräte sind Laufzeitobjekte und lassen sich nicht sinnvoll kopieren.
        if is_untransferable_file(&link_meta) {
            continue;
        }

        if link_meta.file_type().is_symlink() {
            preview_compare_file(rel_str, &p, &dst_path, &link_meta, verify_checksums, out)?;
            continue;
        }
        if link_meta.is_dir() {
            match symlink_metadata_retry(&dst_path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && is_trunk_root(rel) => {
                    preview_walk_src(
                        src_root,
                        dst_root,
                        &p,
                        ignore_patterns,
                        verify_checksums,
                        out,
                    )?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Ganzer Teilbaum ist neu → als Einheit melden, nicht rekursieren.
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "copy".into(),
                        is_dir: true,
                        size: 0,
                    });
                }
                Err(e) => return Err(format!("Ziel-Metadaten lesen fehlgeschlagen: {e}")),
                Ok(d) if d.is_dir() => preview_walk_src(
                    src_root,
                    dst_root,
                    &p,
                    ignore_patterns,
                    verify_checksums,
                    out,
                )?,
                Ok(_) => {
                    // Ziel existiert, ist aber kein Verzeichnis (z. B. Datei) →
                    // Teilbaum als Einheit kopieren (überschreiben), nicht rekursieren.
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "copy".into(),
                        is_dir: true,
                        size: 0,
                    });
                }
            }
            continue;
        }
        // Reguläre Datei.
        preview_compare_file(rel_str, &p, &dst_path, &link_meta, verify_checksums, out)?;
    }
    Ok(())
}

/// Läuft das Ziel rekursiv ab und sammelt delete-Einträge (im Ziel vorhanden,
/// aber nicht in der Quelle). Bei transienten Fehlern wird wiederholt; schlägt
/// das Lesen dauerhaft fehl, bricht die Vorschau ab (Err), damit keine
/// unvollständige/gefährliche Löschliste entsteht.
fn preview_walk_dst(
    src_root: &Path,
    dst_root: &Path,
    cur: &Path,
    ignore_patterns: &[String],
    out: &mut Vec<SyncEntry>,
) -> Result<(), String> {
    let entries = read_dir_retry(cur).map_err(|e| format!("Ziel lesen fehlgeschlagen: {e}"))?;
    for entry in entries {
        let p = entry.path();
        let rel = match p.strip_prefix(dst_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();

        if is_transient_trunk_path(rel) || is_ignored_sync_path(rel, ignore_patterns) {
            continue;
        }

        // `.DS_Store` ist reines Finder-Artefakt – nie löschen (wird neu erzeugt).
        if name == ".DS_Store" {
            continue;
        }
        // AppleDouble `._X` sind reine macOS-Metadaten-Sidecars (Ressourcen-Fork/
        // xattrs zur Datei `X`). Auf Netzlaufwerken ohne native xattrs legt macOS
        // sie selbst an. Sie werden NIE zum Löschen vorgeschlagen:
        //  * Existiert `X`, gehört `._X` dazu und darf nicht entfernt werden.
        //  * Ist `X` (z. B. über die IONOS Web-GUI) gelöscht, bleibt `._X` als
        //    verwaistes Sidecar auf dem Server. macOS virtualisiert `._X` aber
        //    über die AppleDouble-Schicht und liefert beim Löschen ENOENT
        //    (os error 2), sobald der Partner `X` fehlt – der Eintrag ließe sich
        //    über den Mount gar nicht entfernen und tauchte bei jedem Sync erneut
        //    als „Zu löschen" auf. Wie `.DS_Store` daher grundsätzlich überspringen.
        if is_apple_double_name(&name) {
            continue;
        }

        let dmeta = match symlink_metadata_retry(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("Ziel-Metadaten lesen fehlgeschlagen: {e}")),
        };
        // Sonderdateien sind Laufzeitobjekte und werden weder synchronisiert
        // noch als überzählige Zieldateien zum Löschen vorgeschlagen.
        if is_untransferable_file(&dmeta) {
            continue;
        }
        let is_dir = dmeta.is_dir() && !dmeta.file_type().is_symlink();
        // `exists()` folgt Symlinks – so werden dereferenzierte Ziel-Inhalte
        // korrekt der Quelle zugeordnet und nicht fälschlich zum Löschen markiert.
        if !src_root.join(rel).exists() && is_dir && is_trunk_root(rel) {
            preview_walk_dst(src_root, dst_root, &p, ignore_patterns, out)?;
        } else if !src_root.join(rel).exists() {
            out.push(SyncEntry {
                rel: rel.to_string_lossy().into_owned(),
                action: "delete".into(),
                is_dir,
                size: dmeta.len(),
            });
            // Ganzer Teilbaum wird gelöscht → nicht weiter absteigen.
        } else if is_dir {
            preview_walk_dst(src_root, dst_root, &p, ignore_patterns, out)?;
        }
    }
    Ok(())
}

/// Berechnet die Unterschiede zwischen `src` und `dst` (einweg: src → dst).
/// Vergleich über Größe + Änderungszeit (mit Toleranz). Symlinks werden
/// dereferenziert verglichen; transiente Netzwerkfehler werden wiederholt und
/// führen im Ernstfall zum Abbruch statt zu falschen Zahlen.
#[tauri::command]
async fn sync_preview(
    src: String,
    dst: String,
    delete_extra: bool,
    ignore_patterns: Vec<String>,
    verify_checksums: bool,
) -> Result<Vec<SyncEntry>, String> {
    // Der Verzeichnis-Abgleich kann auf langsamen Netzlaufwerken (WebDAV/HiDrive,
    // SMB) sehr lange dauern. Als synchroner Befehl liefe er auf dem Haupt-Thread
    // und würde die gesamte Oberfläche einfrieren (macOS-Beachball) – der
    // Vorbereitungs-Hinweis im Dialog könnte gar nicht erst gezeichnet werden.
    // Deshalb wird die eigentliche Arbeit auf einem Blocking-Thread ausgeführt,
    // sodass die UI weiterhin reagiert und den Hinweis anzeigt.
    tauri::async_runtime::spawn_blocking(move || {
        sync_preview_inner(&src, &dst, delete_extra, ignore_patterns, verify_checksums)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn sync_preview_inner(
    src: &str,
    dst: &str,
    delete_extra: bool,
    extra_ignore_patterns: Vec<String>,
    verify_checksums: bool,
) -> Result<Vec<SyncEntry>, String> {
    let src_root = expand_tilde(src);
    let dst_root = expand_tilde(dst);
    if !src_root.is_dir() {
        return Err(format!(
            "Quelle ist kein Verzeichnis: {}",
            src_root.display()
        ));
    }
    // Die Ausführung eines Kopierjobs schützt bereits vor diesem Fall. Die
    // Vorschau muss jedoch ebenso früh abbrechen: Liegt das Ziel innerhalb der
    // Quelle, würde der rekursive Durchlauf den gerade angelegten Zielbaum
    // wieder als Quelle besuchen (`Quelle/.../Quelle/...`) und nie fertig.
    if destination_is_within_source(&src_root, &dst_root)
        .map_err(|e| format!("Zielpfad prüfen fehlgeschlagen: {e}"))?
    {
        return Err(format!(
            "Zielverzeichnis liegt innerhalb der Quelle: {}",
            dst_root.display()
        ));
    }
    let mut out: Vec<SyncEntry> = Vec::new();
    let ignore_patterns = sync_ignore_patterns(&src_root, extra_ignore_patterns);

    // Quelle durchlaufen → copy/update (robust gegen transiente Netzwerkfehler).
    preview_walk_src(
        &src_root,
        &dst_root,
        &src_root,
        &ignore_patterns,
        verify_checksums,
        &mut out,
    )?;

    // Ziel durchlaufen → delete (nur Extras; Teilbäume werden als Einheit gemeldet).
    if delete_extra && dst_root.is_dir() {
        preview_walk_dst(&src_root, &dst_root, &dst_root, &ignore_patterns, &mut out)?;
    }

    Ok(out)
}

/// Vorschau für einen konfliktbewussten Zwei-Wege-Sync. Änderungen, die nur
/// auf einer Seite neuer sind, erhalten eine eindeutige Kopierrichtung. Bei
/// gleichzeitigen bzw. nicht zeitlich auflösbaren Änderungen bleibt der
/// Eintrag ein expliziter Konflikt für die Benutzerentscheidung.
#[tauri::command]
async fn sync_two_way_preview(
    left: String,
    right: String,
    ignore_patterns: Vec<String>,
    verify_checksums: bool,
) -> Result<Vec<SyncEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        sync_two_way_preview_inner(&left, &right, ignore_patterns, verify_checksums)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn newer_sync_side(left_root: &Path, right_root: &Path, rel: &str) -> Option<&'static str> {
    let left = std::fs::metadata(left_root.join(rel)).ok()?;
    let right = std::fs::metadata(right_root.join(rel)).ok()?;
    if left.is_dir() || right.is_dir() {
        return None;
    }
    let left_mtime = file_mtime_secs(&left);
    let right_mtime = file_mtime_secs(&right);
    if left_mtime > right_mtime + MTIME_TOLERANCE_SECS {
        Some("left_to_right")
    } else if right_mtime > left_mtime + MTIME_TOLERANCE_SECS {
        Some("right_to_left")
    } else {
        None
    }
}

fn sync_two_way_preview_inner(
    left: &str,
    right: &str,
    ignore_patterns: Vec<String>,
    verify_checksums: bool,
) -> Result<Vec<SyncEntry>, String> {
    let left_root = expand_tilde(left);
    let right_root = expand_tilde(right);
    let left_to_right = sync_preview_inner(
        left,
        right,
        false,
        ignore_patterns.clone(),
        verify_checksums,
    )?;
    let right_to_left = sync_preview_inner(right, left, false, ignore_patterns, verify_checksums)?;
    let mut combined: HashMap<String, (Option<SyncEntry>, Option<SyncEntry>)> = HashMap::new();
    for entry in left_to_right {
        let rel = entry.rel.clone();
        combined.entry(rel).or_default().0 = Some(entry);
    }
    for entry in right_to_left {
        let rel = entry.rel.clone();
        combined.entry(rel).or_default().1 = Some(entry);
    }
    let mut out = Vec::with_capacity(combined.len());
    for (rel, (from_left, from_right)) in combined {
        let base = from_left
            .as_ref()
            .or(from_right.as_ref())
            .expect("entry exists");
        let is_dir = base.is_dir;
        let size = base.size;
        let action = match (&from_left, &from_right) {
            (Some(_), None) => "left_to_right",
            (None, Some(_)) => "right_to_left",
            (Some(_), Some(_)) => {
                newer_sync_side(&left_root, &right_root, &rel).unwrap_or("conflict")
            }
            (None, None) => unreachable!(),
        };
        out.push(SyncEntry {
            rel,
            action: action.into(),
            is_dir,
            size,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

// ---------- Watcher ----------

#[derive(Default)]
struct WatcherManager {
    inner: Mutex<HashMap<String, Debouncer<RecommendedWatcher>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaneChanged {
    pane_id: String,
    path: String,
}

#[tauri::command]
fn watch_path(app: AppHandle, pane_id: String, path: String) -> Result<(), String> {
    let mgr: State<WatcherManager> = app.state();
    let p = expand_tilde(&path);
    if !p.is_dir() {
        return Err(format!("Pfad ist kein Verzeichnis: {}", p.display()));
    }

    let app_for_cb = app.clone();
    let pane_for_cb = pane_id.clone();
    let path_for_cb = p.to_string_lossy().into_owned();

    let mut debouncer = new_debouncer(
        Duration::from_millis(250),
        move |res: Result<
            Vec<notify_debouncer_mini::DebouncedEvent>,
            notify_debouncer_mini::notify::Error,
        >| {
            if res.is_ok() {
                let _ = app_for_cb.emit(
                    "pane-changed",
                    PaneChanged {
                        pane_id: pane_for_cb.clone(),
                        path: path_for_cb.clone(),
                    },
                );
            }
        },
    )
    .map_err(|e| e.to_string())?;

    debouncer
        .watcher()
        .watch(&p, RecursiveMode::NonRecursive)
        .map_err(|e| e.to_string())?;

    // Alten Watcher für diese Pane ersetzen → wird beim Drop gestoppt.
    lock_safe(&mgr.inner).insert(pane_id, debouncer);
    Ok(())
}

#[tauri::command]
fn unwatch_pane(app: AppHandle, pane_id: String) {
    let mgr: State<WatcherManager> = app.state();
    lock_safe(&mgr.inner).remove(&pane_id);
}

#[tauri::command]
/// Glob-Matching für `*` (beliebig viele Zeichen) und `?` (genau ein Zeichen).
fn glob_match(pat: &[char], txt: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut match_idx): (Option<usize>, usize) = (None, 0);
    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

#[tauri::command]
fn search_in_dir(
    root: String,
    query: String,
    show_hidden: bool,
    max_results: usize,
) -> Result<Vec<Entry>, String> {
    let p = expand_tilde(&root);
    let q = query.to_lowercase();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let use_glob = q.contains('*') || q.contains('?');
    // Glob ohne Anker -> als Teilstring matchen (umschließe mit *...*)
    let pattern: Vec<char> = if use_glob {
        let mut s = String::new();
        if !q.starts_with('*') {
            s.push('*');
        }
        s.push_str(&q);
        if !q.ends_with('*') {
            s.push('*');
        }
        s.chars().collect()
    } else {
        Vec::new()
    };
    let limit = if max_results == 0 { 1000 } else { max_results };
    let mut out: Vec<Entry> = Vec::new();
    let walker = WalkDir::new(&p)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Wurzel immer durchsuchen
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if !show_hidden && name.starts_with('.') {
                return false;
            }
            true
        });
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let name_lc = name.to_lowercase();
        let hit = if use_glob {
            glob_match(&pattern, &name_lc.chars().collect::<Vec<_>>())
        } else {
            name_lc.contains(&q)
        };
        if !hit {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let path_buf = entry.path().to_path_buf();
        let is_symlink = std::fs::symlink_metadata(&path_buf)
            .ok()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let ext = Path::new(&name)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let hidden = name.starts_with('.');
        use std::os::unix::fs::MetadataExt;
        let mode_bits = meta.mode();
        let mode_str = mode_to_rwx(mode_bits);
        let owner = uid_to_name(meta.uid());
        let group = gid_to_name(meta.gid());
        let birth_time = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let kind = ext_to_kind(&ext, meta.is_dir(), is_symlink);
        out.push(Entry {
            name,
            path: path_buf.to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            is_symlink,
            size: if meta.is_dir() { 0 } else { meta.len() },
            mtime,
            ext,
            hidden,
            birth_time,
            kind,
            owner,
            group,
            mode_str,
        });
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

const ZIP_MAX_ENTRY_COUNT: usize = 100_000;
const ZIP_MAX_UNCOMPRESSED_BYTES: u64 = 20 * 1024 * 1024 * 1024;

fn zip_create_inner(srcs: Vec<String>, dst: String) -> Result<(), String> {
    use std::fs::File;
    use std::io::copy;
    use zip::write::SimpleFileOptions;

    let dst_path = expand_tilde(&dst);
    let file = File::create(&dst_path).map_err(|e| e.to_string())?;
    let mut zw = zip::ZipWriter::new(file);
    let options: SimpleFileOptions = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for src in srcs {
        let p = expand_tilde(&src);
        let base = p
            .file_name()
            .ok_or_else(|| format!("ungültiger Pfad: {}", src))?
            .to_string_lossy()
            .into_owned();
        if p.is_dir() {
            for entry in WalkDir::new(&p) {
                let e = entry.map_err(|err| err.to_string())?;
                let path = e.path();
                let rel = path.strip_prefix(&p).map_err(|err| err.to_string())?;
                if rel.as_os_str().is_empty() {
                    continue;
                }
                let mut name = base.clone();
                name.push('/');
                name.push_str(&rel.to_string_lossy());
                if e.file_type().is_dir() {
                    if !name.ends_with('/') {
                        name.push('/');
                    }
                    zw.add_directory(name, options)
                        .map_err(|err| err.to_string())?;
                } else if e.file_type().is_file() {
                    zw.start_file(name, options)
                        .map_err(|err| err.to_string())?;
                    let mut f = File::open(path).map_err(|err| err.to_string())?;
                    copy(&mut f, &mut zw).map_err(|err| err.to_string())?;
                }
            }
        } else if p.is_file() {
            zw.start_file(base, options)
                .map_err(|err| err.to_string())?;
            let mut f = File::open(&p).map_err(|err| err.to_string())?;
            copy(&mut f, &mut zw).map_err(|err| err.to_string())?;
        }
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn zip_create(srcs: Vec<String>, dst: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || zip_create_inner(srcs, dst))
        .await
        .map_err(|e| e.to_string())?
}

fn zip_extract_inner(src: String, dst_dir: String) -> Result<(), String> {
    use std::fs::{self, File, OpenOptions};
    use std::io::copy;

    let src_path = expand_tilde(&src);
    let dst_path = expand_tilde(&dst_dir);
    fs::create_dir_all(&dst_path).map_err(|e| e.to_string())?;

    let file = File::open(&src_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() > ZIP_MAX_ENTRY_COUNT {
        return Err(format!(
            "ZIP enthält zu viele Einträge (maximal {ZIP_MAX_ENTRY_COUNT})"
        ));
    }

    let mut total_uncompressed = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        total_uncompressed = total_uncompressed
            .checked_add(entry.size())
            .ok_or_else(|| "ZIP-Größe ist ungültig".to_string())?;
        if total_uncompressed > ZIP_MAX_UNCOMPRESSED_BYTES {
            return Err(format!(
                "ZIP entpackt mehr als {} GiB und wurde aus Sicherheitsgründen abgebrochen",
                ZIP_MAX_UNCOMPRESSED_BYTES / 1024 / 1024 / 1024
            ));
        }
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        // Defense in depth: lehne absolute Pfade und `..`-Komponenten ab,
        // auch wenn enclosed_name() das eigentlich abfangen sollte.
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let out_path = dst_path.join(&rel);
        // Sicherstellen, dass out_path tatsächlich unterhalb von dst_path liegt.
        if !out_path.starts_with(&dst_path) {
            continue;
        }
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // Das Zielverzeichnis wird von der UI immer neu angelegt. `create_new`
            // verhindert, dass doppelte ZIP-Einträge oder ein zwischenzeitlich
            // angelegter Pfad unbemerkt überschrieben werden.
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&out_path)
                .map_err(|e| e.to_string())?;
            copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn zip_extract(src: String, dst_dir: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || zip_extract_inner(src, dst_dir))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub name: String,
    pub icon: String,
    pub path: String,
}

fn favorites_file() -> Result<PathBuf, String> {
    let base = dirs::config_dir().ok_or_else(|| "config dir nicht gefunden".to_string())?;
    let dir = base.join("dualbeam");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("favorites.json"))
}

fn default_favorites() -> Vec<Favorite> {
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    vec![
        Favorite {
            name: "Home".into(),
            icon: "🏠".into(),
            path: home.clone(),
        },
        Favorite {
            name: "Desktop".into(),
            icon: "🖥".into(),
            path: format!("{home}/Desktop"),
        },
        Favorite {
            name: "Dokumente".into(),
            icon: "📄".into(),
            path: format!("{home}/Documents"),
        },
        Favorite {
            name: "Downloads".into(),
            icon: "⬇️".into(),
            path: format!("{home}/Downloads"),
        },
        Favorite {
            name: "Bilder".into(),
            icon: "🖼".into(),
            path: format!("{home}/Pictures"),
        },
        Favorite {
            name: "Musik".into(),
            icon: "🎵".into(),
            path: format!("{home}/Music"),
        },
        Favorite {
            name: "Filme".into(),
            icon: "🎬".into(),
            path: format!("{home}/Movies"),
        },
        Favorite {
            name: "Programme".into(),
            icon: "🧰".into(),
            path: "/Applications".into(),
        },
    ]
}

#[tauri::command]
fn load_favorites() -> Result<Vec<Favorite>, String> {
    let path = favorites_file()?;
    if !path.exists() {
        return Ok(default_favorites());
    }
    let s = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let favs: Vec<Favorite> = serde_json::from_str(&s).map_err(|e| e.to_string())?;
    Ok(favs)
}

#[tauri::command]
fn save_favorites(favs: Vec<Favorite>) -> Result<(), String> {
    let path = favorites_file()?;
    let s = serde_json::to_string_pretty(&favs).map_err(|e| e.to_string())?;
    std::fs::write(&path, s).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub ext: String,
    pub kind: String, // "text" | "image" | "dir" | "binary" | "other"
}

fn classify(ext: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "dir";
    }
    let e = ext.to_ascii_lowercase();
    match e.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif" | "heic" | "svg"
        | "ico" => "image",
        "txt" | "md" | "markdown" | "rs" | "ts" | "tsx" | "js" | "jsx" | "json" | "toml"
        | "yaml" | "yml" | "html" | "htm" | "css" | "scss" | "sh" | "zsh" | "bash" | "py"
        | "rb" | "go" | "java" | "c" | "h" | "cpp" | "hpp" | "cs" | "swift" | "kt" | "php"
        | "sql" | "xml" | "ini" | "cfg" | "conf" | "log" | "csv" | "tsv" | "lock" | "gitignore"
        | "env" => "text",
        "" => "other",
        _ => "binary",
    }
}

#[tauri::command]
fn preview_info(path: String) -> Result<PreviewInfo, String> {
    let p = expand_tilde(&path);
    let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    let is_dir = meta.is_dir();
    let size = if is_dir { 0 } else { meta.len() };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let ext = if is_dir {
        String::new()
    } else {
        p.extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let kind = classify(&ext, is_dir).to_string();
    Ok(PreviewInfo {
        name,
        path: p.to_string_lossy().into_owned(),
        is_dir,
        size,
        mtime,
        ext,
        kind,
    })
}

#[tauri::command]
fn read_text_preview(path: String, max_bytes: usize) -> Result<String, String> {
    use std::io::Read;
    let p = expand_tilde(&path);
    let mut f = std::fs::File::open(&p).map_err(|e| e.to_string())?;
    let cap = max_bytes.clamp(1, 1_048_576);
    let mut buf = vec![0u8; cap];
    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[tauri::command]
fn read_image_thumb(path: String, size: u32) -> Result<String, String> {
    use std::process::Command;
    let p = expand_tilde(&path);
    let tmp_dir = std::env::temp_dir().join("dualbeam-thumbs");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "thumb".into());
    let ts = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out_name = format!("{}-{}.png", stem.replace('/', "_"), ts);
    let out_path = tmp_dir.join(&out_name);
    let status = Command::new("qlmanage")
        .args([
            "-t",
            "-s",
            &size.to_string(),
            "-o",
            &tmp_dir.to_string_lossy(),
            &p.to_string_lossy(),
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("qlmanage fehlgeschlagen".into());
    }
    // qlmanage writes <stem>.png — find it
    let expected = tmp_dir.join(format!(
        "{}.png",
        p.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let final_path = if expected.exists() {
        expected
    } else {
        out_path
    };
    if !final_path.exists() {
        // fallback: search dir for any png with our stem
        if let Ok(rd) = std::fs::read_dir(&tmp_dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.contains(&stem) && n.ends_with(".png") {
                    let bytes = std::fs::read(e.path()).map_err(|e| e.to_string())?;
                    let _ = std::fs::remove_file(e.path());
                    return Ok(format!("data:image/png;base64,{}", base64_encode(&bytes)));
                }
            }
        }
        return Err("Thumbnail nicht gefunden".into());
    }
    let bytes = std::fs::read(&final_path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&final_path);
    Ok(format!("data:image/png;base64,{}", base64_encode(&bytes)))
}

#[tauri::command]
fn read_file_icon(path: String, size: u32) -> Result<String, String> {
    let p = expand_tilde(&path);
    let s = if size == 0 { 32 } else { size };
    let bytes = promise_drag::file_icon_png(&p.to_string_lossy(), s)?;
    Ok(format!("data:image/png;base64,{}", base64_encode(&bytes)))
}

#[tauri::command]
fn open_terminal(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    let dir = if p.is_dir() {
        p
    } else {
        p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
    };
    std::process::Command::new("open")
        .args(["-a", "Terminal"])
        .arg(&dir)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_in_editor(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    // `open -t` öffnet die Datei im Standard-Texteditor.
    std::process::Command::new("open")
        .arg("-t")
        .arg(&p)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Properties {
    path: String,
    name: String,
    kind: String,
    is_dir: bool,
    is_symlink: bool,
    symlink_target: Option<String>,
    size: u64,
    file_count: u64,
    dir_count: u64,
    mtime: i64,
    btime: i64,
    atime: i64,
    owner: String,
    group: String,
    uid: u32,
    gid: u32,
    mode: u32,
    mode_str: String,
}

#[tauri::command]
async fn get_properties(path: String) -> Result<Properties, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Properties, String> {
        use std::os::unix::fs::MetadataExt;
        let p = expand_tilde(&path);
        let symlink_meta =
            std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
        let is_symlink = symlink_meta.file_type().is_symlink();
        let symlink_target = if is_symlink {
            std::fs::read_link(&p)
                .ok()
                .map(|t| t.to_string_lossy().into_owned())
        } else {
            None
        };
        let meta = std::fs::metadata(&p).unwrap_or_else(|_| symlink_meta.clone());
        let name = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned());
        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let kind = ext_to_kind(&ext, meta.is_dir(), is_symlink);
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let btime = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let atime = meta
            .accessed()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mode = meta.mode();
        let mode_str = mode_to_rwx(mode);
        let owner = uid_to_name(meta.uid());
        let group = gid_to_name(meta.gid());

        let (size, file_count, dir_count) = if meta.is_dir() {
            let mut s: u64 = 0;
            let mut fc: u64 = 0;
            let mut dc: u64 = 0;
            for entry in walkdir::WalkDir::new(&p)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.path() == p {
                    continue;
                }
                if let Ok(m) = entry.metadata() {
                    if m.is_dir() {
                        dc += 1;
                    } else {
                        fc += 1;
                        s += m.len();
                    }
                }
            }
            (s, fc, dc)
        } else {
            (meta.len(), 0, 0)
        };

        Ok(Properties {
            path: p.to_string_lossy().into_owned(),
            name,
            kind,
            is_dir: meta.is_dir(),
            is_symlink,
            symlink_target,
            size,
            file_count,
            dir_count,
            mtime,
            btime,
            atime,
            owner,
            group,
            uid: meta.uid(),
            gid: meta.gid(),
            mode,
            mode_str,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_permissions(path: String, mode: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let p = expand_tilde(&path);
        let perms = std::fs::Permissions::from_mode(mode & 0o7777);
        std::fs::set_permissions(&p, perms).map_err(|e| format!("{}: {}", p.display(), e))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 6) & 0x3f) as usize] as char);
        out.push(CHARS[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(CHARS[((n >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3f) as usize] as char);
        out.push(CHARS[((n >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }
    out
}

// ---------------- Admin/Shell-Helfer ----------------

fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn escape_for_applescript(s: &str) -> Result<String, String> {
    if s.contains('\n') || s.contains('\r') || s.contains('\0') {
        return Err("Ungültiges Zeichen im Pfad/Befehl (Zeilenumbruch oder Null)".into());
    }
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    Ok(out)
}

fn run_with_admin(shell_cmd: &str) -> Result<String, String> {
    let escaped = escape_for_applescript(shell_cmd)?;
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        escaped
    );
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if err.is_empty() {
            "Befehl fehlgeschlagen".into()
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// Extrahiere alle Mountpoints aus tmutil destinationinfo
fn get_tm_mountpoints() -> Vec<String> {
    let output = std::process::Command::new("tmutil")
        .arg("destinationinfo")
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let mut mounts = Vec::new();
            for line in text.lines() {
                if let Some(mp) = line.strip_prefix("Mount Point:") {
                    let m = mp.trim();
                    if !m.is_empty() {
                        mounts.push(m.to_string());
                    }
                }
            }
            return mounts;
        }
    }
    Vec::new()
}

#[cfg(target_os = "macos")]
fn detect_menu_lang() -> String {
    // Nur eine Erstschätzung beim Start — das Frontend korrigiert dies sofort
    // über `set_menu_language`, sobald die aufgelöste Sprache feststeht.
    let l = std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .unwrap_or_default()
        .to_lowercase();
    if l.starts_with("de") || l.contains("de_") {
        "de".into()
    } else {
        "en".into()
    }
}

/// Baut das native macOS-Menü in der gewünschten Sprache auf und setzt es.
/// Wird beim Start und bei jedem Sprachwechsel aufgerufen.
#[cfg(target_os = "macos")]
fn build_and_set_menu(app: &tauri::AppHandle, lang: &str) -> tauri::Result<()> {
    use tauri::menu::{
        AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
    };

    let en = lang == "en";
    let s = |de_s: &'static str, en_s: &'static str| -> &'static str {
        if en {
            en_s
        } else {
            de_s
        }
    };

    let about_meta = AboutMetadataBuilder::new()
        .name(Some("DualBeam"))
        .version(Some(env!("CARGO_PKG_VERSION").to_string()))
        .copyright(Some("Copyright © 2026 N.J. — MIT License"))
        .authors(Some(vec!["N.J.".to_string()]))
        .license(Some("MIT"))
        .comments(Some("Erstellt mit Claude Opus / Built with Claude Opus"))
        .build();

    let about_item = PredefinedMenuItem::about(
        app,
        Some(s("Über DualBeam", "About DualBeam")),
        Some(about_meta),
    )?;
    let hide_item = PredefinedMenuItem::hide(app, Some(s("DualBeam ausblenden", "Hide DualBeam")))?;
    let quit_item = PredefinedMenuItem::quit(app, Some(s("DualBeam beenden", "Quit DualBeam")))?;

    let app_menu = SubmenuBuilder::new(app, "DualBeam")
        .item(&about_item)
        .separator()
        .item(&hide_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let theme_auto = MenuItemBuilder::new(s("Automatisch (System)", "Automatic (system)"))
        .id("theme-auto")
        .build(app)?;
    let theme_light = MenuItemBuilder::new(s("Hell", "Light"))
        .id("theme-light")
        .build(app)?;
    let theme_dark = MenuItemBuilder::new(s("Dunkel", "Dark"))
        .id("theme-dark")
        .build(app)?;

    let view_menu = SubmenuBuilder::new(app, s("Ansicht", "View"))
        .item(&theme_auto)
        .item(&theme_light)
        .item(&theme_dark)
        .build()?;

    let lang_auto = MenuItemBuilder::new("Automatisch (System) / Automatic (system)")
        .id("lang-auto")
        .build(app)?;
    let lang_de = MenuItemBuilder::new("Deutsch").id("lang-de").build(app)?;
    let lang_en = MenuItemBuilder::new("English").id("lang-en").build(app)?;

    let lang_menu = SubmenuBuilder::new(app, "Sprache / Language")
        .item(&lang_auto)
        .item(&lang_de)
        .item(&lang_en)
        .build()?;

    let new_window_item = MenuItemBuilder::new(s("Neues Fenster", "New Window"))
        .id("new-window")
        .accelerator("CmdOrCtrl+N")
        .build(app)?;
    let minimize_item = PredefinedMenuItem::minimize(app, Some(s("Im Dock ablegen", "Minimize")))?;
    let maximize_item = PredefinedMenuItem::maximize(app, Some(s("Zoomen", "Zoom")))?;
    let close_item =
        PredefinedMenuItem::close_window(app, Some(s("Fenster schließen", "Close Window")))?;

    let window_menu = SubmenuBuilder::new(app, s("Fenster", "Window"))
        .item(&new_window_item)
        .separator()
        .item(&minimize_item)
        .item(&maximize_item)
        .separator()
        .item(&close_item)
        .build()?;

    let help_item = MenuItemBuilder::new(s("DualBeam-Hilfe", "DualBeam Help"))
        .id("help")
        .build(app)?;
    let help_menu = SubmenuBuilder::new(app, s("Hilfe", "Help"))
        .item(&help_item)
        .build()?;

    let menu = MenuBuilder::new(app)
        .item(&app_menu)
        .item(&view_menu)
        .item(&lang_menu)
        .item(&window_menu)
        .item(&help_menu)
        .build()?;
    app.set_menu(menu)?;

    // Dock-Menü-Titel in der aktuellen Sprache setzen.
    promise_drag::install_dock_menu(s("Neues Fenster", "New Window"));
    Ok(())
}

/// Vom Frontend aufgerufen, wenn sich die Sprache ändert — baut das native
/// macOS-Menü in der neuen Sprache neu auf.
#[tauri::command]
fn set_menu_language(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let resolved = if lang == "de" || lang == "en" {
            lang
        } else {
            detect_menu_lang()
        };
        build_and_set_menu(&app, &resolved).map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, lang);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_drag::init())
        .manage(JobManager::default())
        .manage(WatcherManager::default())
        .setup(|app| {
            promise_drag::init(app.handle());
            #[cfg(target_os = "macos")]
            {
                use tauri::Emitter;

                let lang = detect_menu_lang();
                build_and_set_menu(app.handle(), &lang)?;

                app.on_menu_event(move |app_handle, event| {
                    let id = event.id().as_ref();
                    if id == "new-window" {
                        open_new_window(app_handle);
                        return;
                    }
                    if id == "help" {
                        let _ = app_handle.emit("dualbeam://help", ());
                        return;
                    }
                    let theme = match id {
                        "theme-auto" => Some("auto"),
                        "theme-light" => Some("light"),
                        "theme-dark" => Some("dark"),
                        _ => None,
                    };
                    if let Some(m) = theme {
                        let _ = app_handle.emit("dualbeam://theme", m);
                        return;
                    }
                    let lang = match id {
                        "lang-auto" => Some("auto"),
                        "lang-de" => Some("de"),
                        "lang-en" => Some("en"),
                        _ => None,
                    };
                    if let Some(l) = lang {
                        let _ = app_handle.emit("dualbeam://lang", l);
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            home_dir,
            list_dir,
            open_default,
            open_privacy_settings,
            create_dir,
            create_file,
            create_symlink,
            create_finder_alias,
            rename_path,
            move_to_trash,
            stage_delete_for_undo,
            undo_staged_delete,
            finalize_staged_delete,
            cleanup_expired_undo,
            force_delete_admin,
            path_exists,
            path_is_network,
            list_volumes,
            list_network_bookmarks,
            remove_network_bookmark,
            remember_network_volume,
            mount_network_url,
            app_version,
            set_menu_language,
            eject_volume,
            mount_dmg,
            find_dmg_mount,
            detach_dmg,
            quick_look,
            check_conflicts,
            run_job,
            run_rsync,
            save_rsync_password,
            load_rsync_password,
            cancel_job,
            sync_preview,
            sync_two_way_preview,
            watch_path,
            unwatch_pane,
            search_in_dir,
            zip_create,
            zip_extract,
            load_favorites,
            save_favorites,
            preview_info,
            read_text_preview,
            read_image_thumb,
            read_file_icon,
            open_terminal,
            open_in_editor,
            get_properties,
            set_permissions,
            clipboard_write_files,
            set_dock_badge,
            clipboard_read_files,
            drag_icon_path,
            promise_drag::start_promise_drag,
            promise_drag::resolve_promise_drop,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app_handle, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = &_event
            {
                if !*has_visible_windows {
                    open_new_window(_app_handle);
                }
            }
        });
}

#[cfg(all(test, target_os = "macos"))]
mod copy_tests {
    use super::{
        bookmark_url_from_mount_source, copy_file_with_metadata, destination_is_within_source,
        is_protected_admin_root, is_untransferable_file, parse_mount_url, preview_walk_src,
        remove_source_after_move, replace_file_after_copy, search_in_dir, sync_preview_inner,
        sync_two_way_preview_inner, zip_extract_inner, CopyOutcome,
    };
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixDatagram;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static TEST_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    extern "C" {
        fn setxattr(
            path: *const libc::c_char,
            name: *const libc::c_char,
            value: *const libc::c_void,
            size: libc::size_t,
            position: u32,
            options: libc::c_int,
        ) -> libc::c_int;
        fn getxattr(
            path: *const libc::c_char,
            name: *const libc::c_char,
            value: *mut libc::c_void,
            size: libc::size_t,
            position: u32,
            options: libc::c_int,
        ) -> libc::ssize_t;
    }

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let uniq = format!(
            "dualbeam_copytest_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEST_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        );
        p.push(uniq);
        std::fs::create_dir_all(&p).unwrap();
        p.push(name);
        p
    }

    #[test]
    fn copies_data_and_preserves_xattr() {
        let src = tmp_path("src.bin");
        let dst = {
            let mut d = src.clone();
            d.set_file_name("dst.bin");
            d
        };
        let payload = b"hello dualbeam sync";
        std::fs::write(&src, payload).unwrap();

        // Ein erweitertes Attribut auf die Quelle setzen.
        let cpath = CString::new(src.as_os_str().as_bytes()).unwrap();
        let xname = CString::new("com.dualbeam.test").unwrap();
        let xval = b"marker";
        let rc = unsafe {
            setxattr(
                cpath.as_ptr(),
                xname.as_ptr(),
                xval.as_ptr() as *const libc::c_void,
                xval.len(),
                0,
                0,
            )
        };
        assert_eq!(rc, 0, "setxattr auf Quelle fehlgeschlagen");

        copy_file_with_metadata(&src, &dst).expect("copy sollte gelingen");

        // Daten identisch?
        assert_eq!(std::fs::read(&dst).unwrap(), payload);

        // xattr auf dem Ziel vorhanden (COPYFILE_ALL-Pfad)?
        let dpath = CString::new(dst.as_os_str().as_bytes()).unwrap();
        let mut buf = [0u8; 32];
        let n = unsafe {
            getxattr(
                dpath.as_ptr(),
                xname.as_ptr(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                0,
            )
        };
        assert!(n > 0, "xattr wurde nicht auf das Ziel kopiert");
        assert_eq!(&buf[..n as usize], xval);

        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn overwrites_existing_destination() {
        let src = tmp_path("src2.bin");
        let dst = {
            let mut d = src.clone();
            d.set_file_name("dst2.bin");
            d
        };
        std::fs::write(&src, b"neuer Inhalt").unwrap();
        std::fs::write(&dst, b"alter, laengerer Inhalt der ueberschrieben wird").unwrap();

        copy_file_with_metadata(&src, &dst).expect("copy sollte bestehende Datei ersetzen");
        assert_eq!(std::fs::read(&dst).unwrap(), b"neuer Inhalt");

        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn replaces_existing_file_only_after_copy_succeeds() {
        let src = tmp_path("replacement-source.txt");
        let dst = src.parent().unwrap().join("replacement-destination.txt");
        std::fs::write(&src, b"new version").unwrap();
        std::fs::write(&dst, b"old version").unwrap();

        replace_file_after_copy(&src, &dst, &AtomicBool::new(false))
            .expect("bestehende Datei sollte ersetzt werden");

        assert_eq!(std::fs::read(&dst).unwrap(), b"new version");
        assert!(!src
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".inprogress")));
        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn marks_unix_sockets_as_untransferable() {
        // Unix-Socket-Pfade sind auf macOS auf rund 104 Bytes begrenzt;
        // `temp_dir()` kann unter `/var/folders/...` bereits länger sein.
        let socket_path = PathBuf::from(format!(
            "/tmp/dualbeam_socket_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let socket = UnixDatagram::bind(&socket_path).unwrap();

        let meta = std::fs::symlink_metadata(&socket_path).unwrap();
        assert!(is_untransferable_file(&meta));

        drop(socket);
        let _ = std::fs::remove_file(socket_path);
    }

    #[test]
    fn excludes_unix_sockets_from_sync_preview() {
        let root = PathBuf::from(format!(
            "/tmp/dualbeam_sync_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let socket = UnixDatagram::bind(src.join("fsmonitor--daemon.ipc")).unwrap();

        let mut entries = Vec::new();
        preview_walk_src(&src, &dst, &src, &[], false, &mut entries).unwrap();
        assert!(entries.is_empty());

        drop(socket);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn synchronizes_hidden_project_files_but_excludes_transient_trunk_dirs() {
        let root = tmp_path("hidden-sync-root");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join(".hidden")).unwrap();
        std::fs::create_dir_all(src.join(".trunk").join("logs")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join(".hidden").join("config"), b"keep me").unwrap();
        std::fs::write(src.join(".trunk").join("trunk.yaml"), b"keep config").unwrap();
        std::fs::write(src.join(".trunk").join("logs").join("active"), b"ephemeral").unwrap();

        let entries = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec![],
            false,
        )
        .unwrap();
        assert!(entries.iter().any(|entry| entry.rel == ".hidden"));
        assert!(entries.iter().any(|entry| entry.rel == ".trunk/trunk.yaml"));
        assert!(!entries
            .iter()
            .any(|entry| entry.rel.starts_with(".trunk/logs")));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn applies_profile_and_dualbeamignore_patterns_to_both_sides() {
        let root = tmp_path("ignore-sync-root");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("cache")).unwrap();
        std::fs::create_dir_all(dst.join("cache")).unwrap();
        std::fs::create_dir_all(dst.join("build")).unwrap();
        std::fs::write(src.join("cache").join("source.tmp"), b"skip").unwrap();
        std::fs::write(src.join("keep.txt"), b"copy").unwrap();
        std::fs::write(dst.join("cache").join("target.tmp"), b"keep").unwrap();
        std::fs::write(dst.join("build").join("old.log"), b"keep").unwrap();
        std::fs::write(src.join(".dualbeamignore"), "cache/\n*.log\n").unwrap();

        let entries = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec!["build/".into()],
            false,
        )
        .unwrap();
        assert!(entries.iter().any(|entry| entry.rel == "keep.txt"));
        assert!(!entries.iter().any(|entry| entry.rel.starts_with("cache/")));
        assert!(!entries.iter().any(|entry| entry.rel.starts_with("build/")));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn two_way_preview_assigns_directions_and_reports_conflicts() {
        let root = tmp_path("two-way-sync-root");
        let left = root.join("left");
        let right = root.join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        std::fs::write(left.join("left-only.txt"), b"left").unwrap();
        std::fs::write(right.join("right-only.txt"), b"right").unwrap();
        std::fs::write(left.join("conflict.txt"), b"left version").unwrap();
        std::fs::write(right.join("conflict.txt"), b"right version is longer").unwrap();

        let entries = sync_two_way_preview_inner(
            &left.to_string_lossy(),
            &right.to_string_lossy(),
            vec![],
            false,
        )
        .unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.rel == "left-only.txt" && entry.action == "left_to_right"));
        assert!(entries
            .iter()
            .any(|entry| entry.rel == "right-only.txt" && entry.action == "right_to_left"));
        assert!(entries
            .iter()
            .any(|entry| entry.rel == "conflict.txt" && entry.action == "conflict"));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn checksum_mode_detects_equal_size_files_with_different_contents() {
        let root = tmp_path("checksum-sync-root");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("same-size.txt"), b"AAAA").unwrap();
        std::fs::write(dst.join("same-size.txt"), b"BBBB").unwrap();

        let without_checksums = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            false,
            vec![],
            false,
        )
        .unwrap();
        assert!(without_checksums.is_empty());

        let with_checksums = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            false,
            vec![],
            true,
        )
        .unwrap();
        assert!(with_checksums
            .iter()
            .any(|entry| entry.rel == "same-size.txt"));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn rejects_copying_a_folder_into_its_own_subfolder() {
        let src = tmp_path("source");
        std::fs::create_dir_all(src.join("child")).unwrap();
        let dst = src.join("child").join("source");

        assert!(destination_is_within_source(&src, &dst).unwrap());

        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn rejects_sync_preview_when_target_is_inside_source() {
        let src = tmp_path("source");
        std::fs::create_dir_all(src.join("child")).unwrap();
        let dst = src.join("child").join("source");

        let result = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            false,
            vec![],
            true,
        );
        assert!(matches!(
            result,
            Err(error) if error.contains("innerhalb der Quelle")
        ));

        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn allows_a_source_that_disappeared_after_sync_preview() {
        let root = tmp_path("vanished-source-root");
        let vanished = root.join("temporary-reference");
        let target = root.join("target").join("temporary-reference");

        // Simulates a temporary Git/tool path that vanished after the preview.
        assert!(!destination_is_within_source(&vanished, &target).unwrap());

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn allows_two_symlinks_to_the_same_directory() {
        let root = tmp_path("symlink-root");
        std::fs::create_dir_all(&root).unwrap();
        let source = root.parent().unwrap().join("source-link");
        let target = root.parent().unwrap().join("target-link");
        std::os::unix::fs::symlink(&root, &source).unwrap();
        std::os::unix::fs::symlink(&root, &target).unwrap();

        assert!(!destination_is_within_source(&source, &target).unwrap());

        let _ = std::fs::remove_file(source);
        let _ = std::fs::remove_file(target);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn protects_system_roots_from_admin_delete() {
        assert!(is_protected_admin_root(std::path::Path::new("/")));
        assert!(is_protected_admin_root(std::path::Path::new("/System/..")));
        assert!(!is_protected_admin_root(std::path::Path::new(
            "/tmp/dualbeam-test-file"
        )));
    }

    #[test]
    fn extracts_a_safe_zip_entry() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let zip_path = tmp_path("safe.zip");
        let out_dir = zip_path.parent().unwrap().join("out");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file("hello.txt", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"hello dualbeam").unwrap();
        archive.finish().unwrap();

        zip_extract_inner(
            zip_path.to_string_lossy().into_owned(),
            out_dir.to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(out_dir.join("hello.txt")).unwrap(),
            b"hello dualbeam"
        );

        let _ = std::fs::remove_dir_all(zip_path.parent().unwrap());
    }

    #[test]
    fn retains_source_when_a_move_copy_is_incomplete() {
        let src = tmp_path("move-source.txt");
        std::fs::write(&src, b"keep me").unwrap();

        assert!(remove_source_after_move(&src, CopyOutcome::Skipped).is_err());
        assert!(src.exists());

        let _ = std::fs::remove_dir_all(src.parent().unwrap());
    }

    #[test]
    fn accepts_secure_mounts_and_rejects_credentials() {
        assert!(parse_mount_url("smb://nas.local/share", false).is_ok());
        assert!(parse_mount_url("https://webdav.example.test/remote.php/dav", false).is_ok());
        assert_eq!(
            parse_mount_url("smb://alice:secret@nas.local/share", false).unwrap_err(),
            "err.network.credentials"
        );
    }

    #[test]
    fn derives_bookmark_urls_without_mount_credentials() {
        assert_eq!(
            bookmark_url_from_mount_source("https://alice@cloud.example/webdav", "webdav")
                .as_deref(),
            Some("https://cloud.example/webdav")
        );
        assert_eq!(
            bookmark_url_from_mount_source("//guest@nas.local/share", "smbfs").as_deref(),
            Some("smb://nas.local/share")
        );
    }

    #[test]
    fn allows_insecure_protocols_only_for_confirmed_local_ips() {
        assert!(parse_mount_url("nfs://192.168.1.20/export", true).is_ok());
        assert!(parse_mount_url("http://[fd00::1]/dav", true).is_ok());
        assert_eq!(
            parse_mount_url("nfs://nas.local/export", true).unwrap_err(),
            "err.network.localIpOnly"
        );
        assert_eq!(
            parse_mount_url("http://8.8.8.8/dav", true).unwrap_err(),
            "err.network.localIpOnly"
        );
        assert_eq!(
            parse_mount_url("nfs://192.168.1.20/export", false).unwrap_err(),
            "err.network.insecureConfirm"
        );
    }

    #[test]
    fn search_finds_files_in_nested_directories() {
        let root = tmp_path("search-root");
        let nested = root.join("one").join("two");
        std::fs::create_dir_all(&nested).unwrap();
        let needle = nested.join("Needle.txt");
        std::fs::write(&needle, b"found recursively").unwrap();

        let results = search_in_dir(
            root.to_string_lossy().into_owned(),
            "needle".into(),
            false,
            10,
        )
        .expect("recursive search should succeed");
        assert!(results
            .iter()
            .any(|entry| entry.path == needle.to_string_lossy()));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
