use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod promise_drag;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, Debouncer};
use notify_debouncer_mini::notify::RecommendedWatcher;

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
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
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
    if let Some(v) = lock_safe(cache).get(&uid) { return v.clone(); }
    let name = unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            uid.to_string()
        } else {
            let cstr = std::ffi::CStr::from_ptr((*pw).pw_name);
            cstr.to_string_lossy().into_owned()
        }
    };
    cache.lock().unwrap_or_else(|p| p.into_inner()).insert(uid, name.clone());
    name
}

fn gid_to_name(gid: u32) -> String {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = lock_safe(cache).get(&gid) { return v.clone(); }
    let name = unsafe {
        let gr = libc::getgrgid(gid as libc::gid_t);
        if gr.is_null() {
            gid.to_string()
        } else {
            let cstr = std::ffi::CStr::from_ptr((*gr).gr_name);
            cstr.to_string_lossy().into_owned()
        }
    };
    cache.lock().unwrap_or_else(|p| p.into_inner()).insert(gid, name.clone());
    name
}

fn ext_to_kind(ext: &str, is_dir: bool, is_symlink: bool) -> String {
    if is_symlink { return "Symlink".into(); }
    if is_dir {
        if ext == "app" { return "Programm".into(); }
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
        "rs" | "ts" | "js" | "tsx" | "jsx" | "py" | "swift" | "c" | "cpp" | "h" | "go" | "rb" | "sh" => "Quellcode".into(),
        other => format!("{}-Datei", other.to_uppercase()),
    }
}

/// Öffnet ein weiteres unabhängiges App-Fenster.
pub(crate) fn open_new_window(app: &AppHandle) {
    use std::sync::atomic::AtomicU32;
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    let label = format!("win-{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        &label,
        tauri::WebviewUrl::App("index.html".into()),
    )
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
            return if rest.is_empty() { home } else { home.join(rest) };
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
        let mode_str = meta.as_ref().map(|m| mode_to_rwx(m.mode())).unwrap_or_default();
        let owner = meta.as_ref().map(|m| uid_to_name(m.uid())).unwrap_or_default();
        let group = meta.as_ref().map(|m| gid_to_name(m.gid())).unwrap_or_default();
        let birth_time = meta
            .as_ref()
            .and_then(|m| m.created().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let size = if is_dir { 0 } else { meta.as_ref().map(|m| m.len()).unwrap_or(0) };
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
fn open_url(url: String) -> Result<(), String> {
    let lower = url.trim_start().to_ascii_lowercase();
    let allowed = lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("x-apple.systempreferences:");
    if !allowed {
        return Err("Nicht erlaubtes URL-Schema".into());
    }
    std::process::Command::new("open")
        .arg(&url)
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
        return Err(format!("Existiert bereits: {}", p.display()));
    }
    std::fs::create_dir(&p).map_err(|e| format!("{}: {}", p.display(), e))
}

#[tauri::command]
fn create_file(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    if p.exists() {
        return Err(format!("Existiert bereits: {}", p.display()));
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
        return Err(format!("Existiert bereits: {}", l.display()));
    }
    std::os::unix::fs::symlink(&t, &l).map_err(|e| format!("{}: {}", l.display(), e))
}

#[tauri::command]
fn create_finder_alias(target: String, link_path: String) -> Result<(), String> {
    let t = expand_tilde(&target);
    let l = expand_tilde(&link_path);
    if l.exists() || std::fs::symlink_metadata(&l).is_ok() {
        return Err(format!("Existiert bereits: {}", l.display()));
    }
    let parent = l
        .parent()
        .ok_or_else(|| "Ungültiges Ziel".to_string())?;
    let name = l
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Ungültiger Name".to_string())?;
    let esc = |s: &str| -> Result<String, String> {
        if s.contains('\n') || s.contains('\r') || s.contains('\0') {
            return Err("Ungültiges Zeichen im Pfad/Namen".into());
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
        return Err(format!("Ziel existiert: {}", b.display()));
    }
    std::fs::rename(&a, &b).map_err(|e| e.to_string())
}

#[tauri::command]
fn move_to_trash(paths: Vec<String>) -> Result<(), String> {
    use std::os::macos::fs::MetadataExt;
    const PROTECT_MASK: u32 = 0x0002 | 0x0004 | 0x00020000 | 0x00040000 | 0x00080000 | 0x00100000;
    for p in &paths {
        let full = expand_tilde(p);
        // Symlinks: das `trash`-Crate folgt auf macOS teilweise dem Ziel
        // und scheitert dann bei fehlenden Rechten am Zielpfad oder bei
        // kaputten Links. Symlinks daher direkt entfernen (nur der Link,
        // nicht das Ziel).
        let is_symlink = std::fs::symlink_metadata(&full)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            std::fs::remove_file(&full)
                .map_err(|e| format!("{}: {}", full.display(), e))?;
            continue;
        }
        let needs_admin = std::fs::symlink_metadata(&full)
            .map(|m| (m.st_flags() & PROTECT_MASK) != 0)
            .unwrap_or(false)
            || full.file_name().and_then(|n| n.to_str())
                .map(|n| n.ends_with(".inprogress"))
                .unwrap_or(false);
        if needs_admin {
            return Err(format!("NEEDS_ADMIN: {}", full.display()));
        }
        trash::delete(&full).map_err(|e| format!("{}: {}", full.display(), e))?;
    }
    Ok(())
}

#[tauri::command]
fn force_delete_admin(paths: Vec<String>) -> Result<(), String> {
    use std::io::Write;
    // Diagnose-Log nur in Debug-Builds; im Release wird nichts auf die Platte geschrieben.
    #[cfg(debug_assertions)]
    let mut log = std::fs::OpenOptions::new().create(true).append(true)
        .open("/tmp/dualbeam-delete.log").ok();
    #[cfg(not(debug_assertions))]
    let mut log: Option<std::fs::File> = None;
    let logln = |log: &mut Option<std::fs::File>, s: &str| {
        if let Some(f) = log.as_mut() { let _ = writeln!(f, "{}", s); }
    };
    logln(&mut log, &format!("=== ts={} ===", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
    logln(&mut log, &format!("paths: {:?}", paths));
    if paths.is_empty() { return Ok(()); }
    let mut parts: Vec<String> = Vec::with_capacity(paths.len() * 6);
    for p in &paths {
        let full = expand_tilde(p);
        let s = full.to_string_lossy().into_owned();
        logln(&mut log, &format!("expanded: {} exists_before={}", s, full.exists()));
        if s.is_empty() || s == "/" {
            return Err(format!("Unerlaubter Pfad: {}", s));
        }
        let parent = full.parent()
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
        logln(&mut log, &format!("after: {} exists_after={}", full.display(), ex));
        if ex {
            still.push(full.to_string_lossy().into_owned());
        }
    }
    if !still.is_empty() {
        return Err(format!("Nicht gelöscht:\n{}\n\nAusgabe:\n{}", still.join("\n"), out.trim()));
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
                        let fstype = opts.split(',').next().unwrap_or("").trim().trim_end_matches(')');
                        map.insert(mp.to_string(), fstype.to_string());
                    }
                }
            }
        }
    }
    map
}

// IONOS HiDrive WebDAV-Netzwerk-Bookmark (Host, Anzeigename, URL an einer Stelle).
const HIDRIVE_HOST: &str = "webdav.hidrive.ionos.com";
const HIDRIVE_NAME: &str = "IONOS HiDrive";
const HIDRIVE_URL: &str = "https://webdav.hidrive.ionos.com/";

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
            let display = match name.as_str() {
                HIDRIVE_HOST => HIDRIVE_NAME.to_string(),
                _ => name,
            };
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

fn known_network_bookmarks() -> Vec<(String, String, String)> {
    // (name, url, expected mount path)
    vec![(
        HIDRIVE_NAME.into(),
        HIDRIVE_URL.into(),
        format!("/Volumes/{}", HIDRIVE_HOST),
    )]
}

#[tauri::command]
fn list_network_bookmarks() -> Result<Vec<NetworkBookmark>, String> {
    let fs = mount_fs_types();
    let mut out = Vec::new();
    for (name, url, mp) in known_network_bookmarks() {
        let connected = fs.contains_key(&mp);
        out.push(NetworkBookmark { name, url, mount_path: mp, connected });
    }
    Ok(out)
}

#[tauri::command]
async fn mount_network_url(url: String) -> Result<String, String> {
    // Nur http(s) erlauben; anschließend per Finder mounten (nutzt Keychain bzw. Dialog).
    let lower = url.trim().to_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("Nur http(s)-URLs erlaubt".into());
    }
    // Escape: keine Newlines/Anführungszeichen erlauben — AppleScript-Injection verhindern.
    if url.contains('"') || url.contains('\n') || url.contains('\r') || url.contains('\0') {
        return Err("Ungültige Zeichen in URL".into());
    }
    let script = format!(
        "tell application \"Finder\" to activate\nmount volume \"{}\"",
        url
    );
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("osascript: {}", e))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() { "Mount fehlgeschlagen".into() } else { err });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn eject_volume(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        // Netzlaufwerke (WebDAV/SMB/NFS/AFP/FTP) kennt `diskutil eject` nicht
        // ("Failed to find disk"). Sie müssen mit `umount`/`diskutil unmount`
        // ausgehängt werden. Physische Datenträger dagegen mit `eject`.
        let fstype = mount_fs_types().get(&path).cloned().unwrap_or_default();
        let is_network = matches!(
            fstype.as_str(),
            "webdav" | "smbfs" | "nfs" | "afpfs" | "ftp" | "cifs"
        );

        if is_network {
            // Zuerst der saubere Weg über diskutil, dann Fallback auf umount.
            let du = std::process::Command::new("diskutil")
                .args(["unmount", &path])
                .output()
                .map_err(|e| format!("diskutil: {}", e))?;
            if du.status.success() {
                return Ok(());
            }
            let um = std::process::Command::new("/sbin/umount")
                .arg(&path)
                .output()
                .map_err(|e| format!("umount: {}", e))?;
            if um.status.success() {
                return Ok(());
            }
            // Erzwungenes Aushängen als letzter Versuch (hängende WebDAV-Sitzung).
            let umf = std::process::Command::new("/sbin/umount")
                .args(["-f", &path])
                .output()
                .map_err(|e| format!("umount -f: {}", e))?;
            if umf.status.success() {
                return Ok(());
            }
            let err = String::from_utf8_lossy(&umf.stderr);
            let so = String::from_utf8_lossy(&umf.stdout);
            return Err(format!("Aushängen fehlgeschlagen: {}{}", err.trim(), so.trim()));
        }

        let out = std::process::Command::new("diskutil")
            .args(["eject", &path])
            .output()
            .map_err(|e| format!("diskutil: {}", e))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let so = String::from_utf8_lossy(&out.stdout);
            return Err(format!("diskutil eject fehlgeschlagen: {}{}", err.trim(), so.trim()));
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
    let basename = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    for block in text.split("================") {
        for l in block.lines() {
            let lt = l.trim();
            if let Some(rest) = lt.strip_prefix("image-path").or_else(|| lt.strip_prefix("image-alias")) {
                let rest = rest.trim_start_matches(|c: char| c == ':' || c.is_whitespace()).trim();
                if rest == p_str || rest == canon || (!basename.is_empty() && rest.ends_with(&basename)) {
                    return Some(block);
                }
            }
        }
    }
    None
}

fn extract_mountpoint(block: &str) -> Option<String> {
    for line in block.lines() {
        let toks: Vec<&str> = line.split('\t').map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
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
            if t.starts_with("/dev/disk") {
                // root disk: keine 's' Partition
                let suffix = &t["/dev/disk".len()..];
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
    let info = std::process::Command::new("hdiutil").arg("info").output().ok()?;
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
        let info = std::process::Command::new("hdiutil").arg("info").output()
            .map_err(|e| format!("hdiutil: {}", e))?;
        let text = String::from_utf8_lossy(&info.stdout);
        let block = find_dmg_block(&text, &p).ok_or_else(|| "Image ist nicht gemountet".to_string())?;
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
    tauri::async_runtime::spawn_blocking(|| promise_drag::clipboard_read_files())
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
        let out = child.wait_with_output().map_err(|e| format!("hdiutil: {}", e))?;
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
    std::process::Command::new("qlmanage")
        .arg("-p")
        .arg(&p)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ---------- Jobs (copy / move) ----------

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobItem {
    src: String,
    dst: String,
    overwrite: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobProgress {
    job_id: String,
    done: u64,
    total: u64,
    current: String,
    finished: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct JobManager {
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[tauri::command]
fn check_conflicts(items: Vec<JobItem>) -> Vec<String> {
    items
        .iter()
        .filter(|i| expand_tilde(&i.dst).exists())
        .map(|i| i.dst.clone())
        .collect()
}

fn count_files(p: &Path) -> u64 {
    let meta = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return 1;
    }
    WalkDir::new(p)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let ft = e.file_type();
            ft.is_symlink() || !ft.is_dir()
        })
        .count()
        .max(1) as u64
}

struct JobCtx<'a> {
    app: &'a AppHandle,
    job_id: &'a str,
    cancel: &'a Arc<AtomicBool>,
    done: u64,
    total: u64,
}

impl<'a> JobCtx<'a> {
    fn emit(&self, current: &str) {
        let _ = self.app.emit(
            "job-progress",
            JobProgress {
                job_id: self.job_id.to_string(),
                done: self.done,
                total: self.total,
                current: current.to_string(),
                finished: false,
                cancelled: false,
                error: None,
            },
        );
    }
}

fn remove_path(p: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(p)?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
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
    let ret = unsafe { copyfile(s.as_ptr(), d.as_ptr(), std::ptr::null_mut(), COPYFILE_ALL) };
    if ret != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn copy_file_with_metadata(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::copy(src, dst).map(|_| ())
}

fn copy_recursive(
    src: &Path,
    dst: &Path,
    overwrite: bool,
    ctx: &mut JobCtx,
) -> std::io::Result<()> {
    if ctx.cancel.load(Ordering::SeqCst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ));
    }
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        if dst.exists() {
            if overwrite {
                remove_path(dst)?;
            } else {
                return Ok(());
            }
        }
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dst)?;
        ctx.done += 1;
        ctx.emit(&src.to_string_lossy());
    } else if meta.is_dir() {
        if !dst.exists() {
            std::fs::create_dir_all(dst)?;
        } else if !dst.is_dir() {
            if overwrite {
                std::fs::remove_file(dst)?;
                std::fs::create_dir_all(dst)?;
            } else {
                return Ok(());
            }
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            copy_recursive(&from, &to, overwrite, ctx)?;
        }
    } else {
        if dst.exists() {
            if overwrite {
                remove_path(dst)?;
            } else {
                return Ok(());
            }
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        copy_file_with_metadata(src, dst)?;
        ctx.done += 1;
        ctx.emit(&src.to_string_lossy());
    }
    Ok(())
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
        let total: u64 = items
            .iter()
            .map(|i| count_files(&expand_tilde(&i.src)))
            .sum();
        let mut ctx = JobCtx {
            app: &app2,
            job_id: &job_id2,
            cancel: &cancel2,
            done: 0,
            total,
        };
        ctx.emit("");
        for it in &items {
            if cancel2.load(Ordering::SeqCst) {
                break;
            }
            let src = expand_tilde(&it.src);
            let dst = expand_tilde(&it.dst);
            let is_move = kind == "move";
            let mut handled = false;
            if is_move && !dst.exists() {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::rename(&src, &dst).is_ok() {
                    let n = count_files(&dst);
                    ctx.done += n;
                    ctx.emit(&it.src);
                    handled = true;
                }
            }
            if !handled {
                if let Err(e) = copy_recursive(&src, &dst, it.overwrite, &mut ctx) {
                    if e.kind() != std::io::ErrorKind::Interrupted {
                        return Err(format!("{}: {}", src.display(), e));
                    }
                } else if is_move {
                    let _ = remove_path(&src);
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
    if let Some(c) = cancel {
        c.store(true, Ordering::SeqCst);
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

/// Berechnet die Unterschiede zwischen `src` und `dst` (einweg: src → dst).
/// Vergleich über Größe + Änderungszeit. Verzeichnisse, die komplett neu sind,
/// werden als eine Einheit gemeldet (Kinder werden übersprungen).
#[tauri::command]
fn sync_preview(src: String, dst: String, delete_extra: bool) -> Result<Vec<SyncEntry>, String> {
    let src_root = expand_tilde(&src);
    let dst_root = expand_tilde(&dst);
    if !src_root.is_dir() {
        return Err(format!("Quelle ist kein Verzeichnis: {}", src_root.display()));
    }
    let mut out: Vec<SyncEntry> = Vec::new();

    // Quelle durchlaufen → copy/update
    let mut it = WalkDir::new(&src_root).into_iter();
    while let Some(entry) = it.next() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if p == src_root {
            continue;
        }
        let rel = match p.strip_prefix(&src_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().into_owned();
        let ft = entry.file_type();
        let dst_path = dst_root.join(rel);
        if ft.is_dir() {
            if !dst_path.exists() {
                // Ganzer Teilbaum ist neu → als Einheit kopieren, Kinder überspringen.
                out.push(SyncEntry { rel: rel_str, action: "copy".into(), is_dir: true, size: 0 });
                it.skip_current_dir();
            }
            continue;
        }
        // Datei oder Symlink
        let smeta = match std::fs::symlink_metadata(p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = smeta.len();
        match std::fs::symlink_metadata(&dst_path) {
            Err(_) => out.push(SyncEntry { rel: rel_str, action: "copy".into(), is_dir: false, size }),
            Ok(dmeta) => {
                if smeta.len() != dmeta.len() || file_mtime_secs(&smeta) > file_mtime_secs(&dmeta) {
                    out.push(SyncEntry { rel: rel_str, action: "update".into(), is_dir: false, size });
                }
            }
        }
    }

    // Ziel durchlaufen → delete (nur Extras, oberste Ebene)
    if delete_extra && dst_root.is_dir() {
        let mut it = WalkDir::new(&dst_root).into_iter();
        while let Some(entry) = it.next() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let p = entry.path();
            if p == dst_root {
                continue;
            }
            let rel = match p.strip_prefix(&dst_root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if !src_root.join(rel).exists() {
                let is_dir = entry.file_type().is_dir();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(SyncEntry {
                    rel: rel.to_string_lossy().into_owned(),
                    action: "delete".into(),
                    is_dir,
                    size,
                });
                if is_dir {
                    it.skip_current_dir();
                }
            }
        }
    }

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

    let mut debouncer = new_debouncer(Duration::from_millis(250), move |res: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify_debouncer_mini::notify::Error>| {
        if res.is_ok() {
            let _ = app_for_cb.emit(
                "pane-changed",
                PaneChanged {
                    pane_id: pane_for_cb.clone(),
                    path: path_for_cb.clone(),
                },
            );
        }
    })
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
    while pi < pat.len() && pat[pi] == '*' { pi += 1; }
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
        if !q.starts_with('*') { s.push('*'); }
        s.push_str(&q);
        if !q.ends_with('*') { s.push('*'); }
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

#[tauri::command]
fn zip_create(srcs: Vec<String>, dst: String) -> Result<(), String> {
    use std::fs::File;
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;

    let dst_path = expand_tilde(&dst);
    let file = File::create(&dst_path).map_err(|e| e.to_string())?;
    let mut zw = zip::ZipWriter::new(file);
    let options: SimpleFileOptions = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for src in srcs {
        let p = expand_tilde(&src);
        let base = p.file_name().ok_or_else(|| format!("ungültiger Pfad: {}", src))?.to_string_lossy().into_owned();
        if p.is_dir() {
            for entry in WalkDir::new(&p) {
                let e = entry.map_err(|err| err.to_string())?;
                let path = e.path();
                let rel = path.strip_prefix(&p).map_err(|err| err.to_string())?;
                if rel.as_os_str().is_empty() { continue; }
                let mut name = base.clone();
                name.push('/');
                name.push_str(&rel.to_string_lossy());
                if e.file_type().is_dir() {
                    if !name.ends_with('/') { name.push('/'); }
                    zw.add_directory(name, options).map_err(|err| err.to_string())?;
                } else if e.file_type().is_file() {
                    zw.start_file(name, options).map_err(|err| err.to_string())?;
                    let mut f = File::open(path).map_err(|err| err.to_string())?;
                    let mut buf = Vec::new();
                    f.read_to_end(&mut buf).map_err(|err| err.to_string())?;
                    zw.write_all(&buf).map_err(|err| err.to_string())?;
                }
            }
        } else if p.is_file() {
            zw.start_file(base, options).map_err(|err| err.to_string())?;
            let mut f = File::open(&p).map_err(|err| err.to_string())?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).map_err(|err| err.to_string())?;
            zw.write_all(&buf).map_err(|err| err.to_string())?;
        }
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn zip_extract(src: String, dst_dir: String) -> Result<(), String> {
    use std::fs::{self, File};
    use std::io::copy;

    let src_path = expand_tilde(&src);
    let dst_path = expand_tilde(&dst_dir);
    fs::create_dir_all(&dst_path).map_err(|e| e.to_string())?;

    let file = File::open(&src_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
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
            let mut out = File::create(&out_path).map_err(|e| e.to_string())?;
            copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
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
    let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    vec![
        Favorite { name: "Home".into(), icon: "🏠".into(), path: home.clone() },
        Favorite { name: "Desktop".into(), icon: "🖥".into(), path: format!("{home}/Desktop") },
        Favorite { name: "Dokumente".into(), icon: "📄".into(), path: format!("{home}/Documents") },
        Favorite { name: "Downloads".into(), icon: "⬇️".into(), path: format!("{home}/Downloads") },
        Favorite { name: "Bilder".into(), icon: "🖼".into(), path: format!("{home}/Pictures") },
        Favorite { name: "Musik".into(), icon: "🎵".into(), path: format!("{home}/Music") },
        Favorite { name: "Filme".into(), icon: "🎬".into(), path: format!("{home}/Movies") },
        Favorite { name: "Programme".into(), icon: "🧰".into(), path: "/Applications".into() },
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
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif" | "heic" | "svg" | "ico" => "image",
        "txt" | "md" | "markdown" | "rs" | "ts" | "tsx" | "js" | "jsx" | "json" | "toml" | "yaml" | "yml"
        | "html" | "htm" | "css" | "scss" | "sh" | "zsh" | "bash" | "py" | "rb" | "go" | "java" | "c"
        | "h" | "cpp" | "hpp" | "cs" | "swift" | "kt" | "php" | "sql" | "xml" | "ini" | "cfg" | "conf"
        | "log" | "csv" | "tsv" | "lock" | "gitignore" | "env" => "text",
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
    let cap = max_bytes.min(1_048_576).max(1);
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
    let expected = tmp_dir.join(format!("{}.png", p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()));
    let final_path = if expected.exists() { expected } else { out_path };
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
        let symlink_meta = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
        let is_symlink = symlink_meta.file_type().is_symlink();
        let symlink_target = if is_symlink {
            std::fs::read_link(&p).ok().map(|t| t.to_string_lossy().into_owned())
        } else { None };
        let meta = std::fs::metadata(&p).unwrap_or_else(|_| symlink_meta.clone());
        let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned());
        let ext = p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        let kind = ext_to_kind(&ext, meta.is_dir(), is_symlink);
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let btime = meta.created().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let atime = meta.accessed().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let mode = meta.mode();
        let mode_str = mode_to_rwx(mode);
        let owner = uid_to_name(meta.uid());
        let group = gid_to_name(meta.gid());

        let (size, file_count, dir_count) = if meta.is_dir() {
            let mut s: u64 = 0;
            let mut fc: u64 = 0;
            let mut dc: u64 = 0;
            for entry in walkdir::WalkDir::new(&p).follow_links(false).into_iter().filter_map(|e| e.ok()) {
                if entry.path() == p { continue; }
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
            name, kind, is_dir: meta.is_dir(), is_symlink, symlink_target,
            size, file_count, dir_count,
            mtime, btime, atime,
            owner, group, uid: meta.uid(), gid: meta.gid(),
            mode, mode_str,
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
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
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

// ---------------- Time Machine ----------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TmDestination {
    pub name: String,
    pub id: String,
    pub mount_point: String,
    pub kind: String,
}

fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' { out.push_str("'\\''"); } else { out.push(ch); }
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
        return Err(if err.is_empty() { "Befehl fehlgeschlagen".into() } else { err });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[tauri::command]
fn tm_list_destinations() -> Result<Vec<TmDestination>, String> {
    let output = std::process::Command::new("tmutil")
        .arg("destinationinfo")
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut dests: Vec<TmDestination> = Vec::new();
    let mut name = String::new();
    let mut id = String::new();
    let mut mount = String::new();
    let mut kind = String::new();
    let push = |dests: &mut Vec<TmDestination>, name: &mut String, id: &mut String, mount: &mut String, kind: &mut String| {
        if !name.is_empty() || !mount.is_empty() || !id.is_empty() {
            dests.push(TmDestination {
                name: std::mem::take(name),
                id: std::mem::take(id),
                mount_point: std::mem::take(mount),
                kind: std::mem::take(kind),
            });
        }
    };
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with("====") {
            push(&mut dests, &mut name, &mut id, &mut mount, &mut kind);
            continue;
        }
        if let Some(idx) = line.find(':') {
            let key = line[..idx].trim();
            let val = line[idx + 1..].trim().to_string();
            match key {
                "Name" => name = val,
                "ID" => id = val,
                "Mount Point" => mount = val,
                "Kind" => kind = val,
                _ => {}
            }
        }
    }
    push(&mut dests, &mut name, &mut id, &mut mount, &mut kind);
    Ok(dests)
}

#[tauri::command]
fn tm_list_backups(mount_point: Option<String>) -> Result<Vec<String>, String> {
    let mut cmd = std::process::Command::new("tmutil");
    cmd.arg("listbackups");
    if let Some(mp) = mount_point.as_ref() {
        if !mp.is_empty() {
            cmd.arg("-d").arg(mp);
        }
    }
    let output = cmd.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(if err.is_empty() { "tmutil listbackups fehlgeschlagen".into() } else { err });
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut result: Vec<String> = text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();

    // Zusätzlich unvollständige Backups (.inprogress / .previous / .interrupted) einsammeln.
    // Diese tauchen in `tmutil listbackups` nicht auf, liegen aber im selben Eltern-Verzeichnis
    // der regulären `.backup` Ordner (typisch /Volumes/.timemachine/<UUID>/).
    let mut scan_dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(first) = result.first() {
        if let Some(parent) = std::path::Path::new(first).parent() {
            scan_dirs.push(parent.to_path_buf());
        }
    }
    if let Some(mp) = mount_point.as_ref() {
        if !mp.is_empty() {
            scan_dirs.push(std::path::PathBuf::from(mp));
        }
    }
    for dir in scan_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() { continue; }
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                if name.ends_with(".inprogress")
                    || name.ends_with(".previous")
                    || name.ends_with(".interrupted")
                {
                    let s = path.to_string_lossy().into_owned();
                    if !result.contains(&s) { result.push(s); }
                }
            }
        }
    }
    Ok(result)
}

#[tauri::command]
fn tm_delete_backup(backup_path: String, password: String) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if backup_path.is_empty() {
        return Err("Pfad ist leer".into());
    }
    if password.is_empty() {
        return Err("Passwort fehlt".into());
    }

    // Strategie: Terminal.app behält als TCC-verantwortlicher Prozess seine FDA und
    //   ist damit der einzige Weg, TM-ACL-geschützte Pfade als root zu löschen.
    //   Damit kein Terminal-Fenster sichtbar bleibt und nichts in argv/ps leakt:
    //     - Passwort in temp-Datei mode 0600 ablegen
    //     - askpass-Skript ruft `cat` auf diese Datei
    //     - Runner-Skript setzt SUDO_ASKPASS und führt `sudo -A rm -rf` aus
    //     - Alle temp-Dateien löschen sich selbst; Terminal-Fenster wird minimiert.

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir();
    let pw_file = tmp.join(format!(".dualbeam-pw-{}-{}", pid, nanos));
    let ask_file = tmp.join(format!(".dualbeam-askpass-{}-{}.sh", pid, nanos));
    let run_file = tmp.join(format!(".dualbeam-run-{}-{}.sh", pid, nanos));
    let wid_file = tmp.join(format!(".dualbeam-wid-{}-{}", pid, nanos));

    let write_secure = |path: &Path, content: &str, mode: u32| -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new()
            .write(true).create_new(true).truncate(true)
            .mode(mode)
            .open(path)
            .map_err(|e| format!("temp anlegen {}: {}", path.display(), e))?;
        f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
        Ok(())
    };

    // Passwort + Newline (sudo erwartet \n am Ende der Eingabe)
    let mut pw_content = password.clone();
    if !pw_content.ends_with('\n') { pw_content.push('\n'); }
    write_secure(&pw_file, &pw_content, 0o600)?;

    let askpass_content = format!(
        "#!/bin/sh\n/bin/cat {}\n",
        shell_single_quote(&pw_file.to_string_lossy())
    );
    write_secure(&ask_file, &askpass_content, 0o700)?;

    // Runner schließt am Ende sein eigenes Terminal-Fenster anhand der zuvor von AppleScript
    // in wid_file abgelegten window-id, damit nach Abschluss nichts sichtbar zurückbleibt.
    let runner_content = format!(
        "#!/bin/sh\nexport SUDO_ASKPASS={ap}\n/usr/bin/sudo -A /bin/rm -rf {bp}\nrc=$?\nfor i in 1 2 3 4 5 6 7 8 9 10; do\n  [ -s {wid} ] && break\n  /bin/sleep 0.1\ndone\nWID=$(/bin/cat {wid} 2>/dev/null | /usr/bin/tr -d ' \\n')\n/bin/rm -f {pw} {ap} {wid} \"$0\"\nif [ -n \"$WID\" ]; then\n  /usr/bin/osascript -e \"tell application \\\"Terminal\\\" to close (every window whose id is $WID)\" >/dev/null 2>&1 || true\nfi\nexit $rc\n",
        ap = shell_single_quote(&ask_file.to_string_lossy()),
        bp = shell_single_quote(&backup_path),
        pw = shell_single_quote(&pw_file.to_string_lossy()),
        wid = shell_single_quote(&wid_file.to_string_lossy()),
    );
    write_secure(&run_file, &runner_content, 0o700)?;

    let cleanup = || {
        let _ = std::fs::remove_file(&pw_file);
        let _ = std::fs::remove_file(&ask_file);
        let _ = std::fs::remove_file(&run_file);
        let _ = std::fs::remove_file(&wid_file);
    };

    // AppleScript: Terminal launchen (ohne activate), Skript ausführen, Fenster-ID nach
    // wid_file schreiben (damit der Runner sich später selbst schließen kann), dann minimieren.
    let do_cmd = format!(
        "{}; exit",
        shell_single_quote(&run_file.to_string_lossy())
    );
    let applescript = format!(
        "tell application \"Terminal\"\n  if not running then launch\n  do script \"{cmd}\"\n  delay 0.15\n  try\n    set wid to id of front window\n    do shell script \"/bin/echo \" & wid & \" > \" & quoted form of \"{widfile}\"\n  end try\n  try\n    set miniaturized of front window to true\n  end try\nend tell",
        cmd = escape_for_applescript(&do_cmd)?,
        widfile = escape_for_applescript(&wid_file.to_string_lossy())?,
    );

    // osascript über stdin → kein Argument im argv (auch kein indirekter Passwort-Leak)
    let spawn = std::process::Command::new("osascript")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawn {
        Ok(c) => c,
        Err(e) => { cleanup(); return Err(format!("osascript starten: {}", e)); }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(applescript.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => { cleanup(); return Err(format!("osascript wait: {}", e)); }
    };
    if !out.status.success() {
        cleanup();
        return Err(format!(
            "Terminal-Start fehlgeschlagen: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // temp Dateien räumt der Runner selbst weg – falls er gar nicht startet, kommt der OS-tmp-Cleanup.
    Ok("Löschvorgang läuft im Hintergrund (Terminal-Fenster ist minimiert). Nach kurzer Wartezeit auf „Aktualisieren“ klicken.".into())
}

#[tauri::command]
fn tm_wipe_volume(mount_point: String) -> Result<String, String> {
    if mount_point.is_empty() {
        return Err("Mountpoint ist leer".into());
    }
    let p = std::path::Path::new(&mount_point);
    if !p.is_dir() {
        return Err(format!("Mountpoint nicht gefunden: {}", mount_point));
    }
    // Sicherheitscheck 1: nur unterhalb /Volumes/ und nicht /Volumes selbst
    if !mount_point.starts_with("/Volumes/") || mount_point == "/Volumes" || mount_point == "/Volumes/" {
        return Err("Nur Volumes unterhalb /Volumes dürfen gelöscht werden".into());
    }
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return Err("Volume-Name konnte nicht ermittelt werden".into());
    }
    // Sicherheitscheck 2: System-/Boot-Volumes hart ablehnen.
    // Niemals systemrelevante Namen zulassen.
    let blocked_names = [
        "Macintosh HD",
        "Macintosh HD - Data",
        "Preboot",
        "Recovery",
        "VM",
        "Update",
        "xarts",
        "iSCPreboot",
        "Hardware",
    ];
    if blocked_names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
        return Err(format!("Sicherheits-Stopp: '{}' ist ein System-Volume und darf nicht gelöscht werden.", name));
    }
    // Auflösen: falls /Volumes/<name> ein Symlink auf / ist (Boot-Volume), ablehnen.
    if let Ok(canon) = std::fs::canonicalize(p) {
        let cs = canon.to_string_lossy();
        if cs == "/" {
            return Err("Sicherheits-Stopp: dieser Mountpoint zeigt auf das Boot-Volume (/).".into());
        }
    }
    // diskutil info abfragen und auf Boot/System/Internal prüfen.
    let info_out = std::process::Command::new("/usr/sbin/diskutil")
        .args(["info", &mount_point])
        .output()
        .map_err(|e| format!("diskutil info: {}", e))?;
    if !info_out.status.success() {
        return Err(format!(
            "diskutil info fehlgeschlagen: {}",
            String::from_utf8_lossy(&info_out.stderr).trim()
        ));
    }
    let info = String::from_utf8_lossy(&info_out.stdout).into_owned();
    let lower = info.to_lowercase();
    // Mountpoint /
    for line in info.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Mount Point:") {
            if v.trim() == "/" {
                return Err("Sicherheits-Stopp: Mountpoint ist / (Boot-Volume).".into());
            }
        }
    }
    // Boot/System-Indikatoren
    let danger_markers = [
        "volume role:                system",
        "volume role:                data",
        "volume role:                preboot",
        "volume role:                recovery",
        "volume role:                vm",
        "volume role:                update",
        "volume role:                hardware",
        "volume role:                xart",
        "signed system volume:       yes",
    ];
    // Toleranter Vergleich: Marker ohne mehrfache Spaces
    let normalized: String = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    for m in danger_markers.iter() {
        let mn: String = m.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.contains(&mn) {
            return Err(format!(
                "Sicherheits-Stopp: Volume '{}' ist als System-/Boot-Volume markiert und wird nicht gelöscht.",
                name
            ));
        }
    }
    // Internal=Yes wird NICHT mehr blockiert — interne TM-Volumes (z.B. zweite SSD)
    // sind erlaubt, sofern sie keine System-/Boot-Rolle haben (oben bereits geprüft).

    // APFS eraseVolume (recreate volume) — funktioniert bei APFS-Volumes inkl. TM
    let cmd = format!(
        "/usr/sbin/diskutil eraseVolume APFS {} {} 2>&1",
        shell_single_quote(&name),
        shell_single_quote(&mount_point)
    );
    let out = run_with_admin(&cmd).map_err(|e| e)?;
    Ok(out)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TmVolume {
    pub name: String,
    pub path: String,
    /// "active" = registered TM destination, "former" = ex-TM, never "none" in returned list
    pub kind: String,
    pub has_backupdb: bool,
    pub role_backup: bool,
    pub registered: bool,
}

fn diskutil_info(mount_point: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/sbin/diskutil")
        .args(["info", mount_point])
        .output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn volume_uuid(info: &str) -> Option<String> {
    for line in info.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Volume UUID:") {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn registered_tm_uuids() -> Vec<String> {
    let out = match std::process::Command::new("/usr/bin/tmutil")
        .args(["destinationinfo", "-X"]).output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let mut uuids = Vec::new();
    let mut in_id = false;
    for line in s.lines() {
        let t = line.trim();
        if in_id {
            if let Some(v) = t.strip_prefix("<string>").and_then(|s| s.strip_suffix("</string>")) {
                uuids.push(v.to_string());
            }
            in_id = false;
        } else if t == "<key>ID</key>" {
            in_id = true;
        }
    }
    uuids
}

fn is_system_name(name: &str) -> bool {
    let blocked = [
        "Macintosh HD","Macintosh HD - Data","Preboot","Recovery","VM",
        "Update","xarts","iSCPreboot","Hardware",
    ];
    blocked.iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn info_has_system_role(info: &str) -> bool {
    let normalized: String = info.to_lowercase().split_whitespace()
        .collect::<Vec<_>>().join(" ");
    let markers = [
        "volume role: system","volume role: data","volume role: preboot",
        "volume role: recovery","volume role: vm","volume role: update",
        "volume role: hardware","volume role: xart",
        "signed system volume: yes",
    ];
    markers.iter().any(|m| normalized.contains(m))
}

fn classify_tm_volume(mount_point: &str, registered: &[String]) -> Option<TmVolume> {
    use std::fs;
    let p = std::path::Path::new(mount_point);
    let name = p.file_name().and_then(|s| s.to_str())?.to_string();
    if is_system_name(&name) { return None; }
    let info = diskutil_info(mount_point)?;
    if info_has_system_role(&info) { return None; }
    let normalized: String = info.to_lowercase().split_whitespace()
        .collect::<Vec<_>>().join(" ");
    let role_backup = normalized.contains("volume role: backup")
        || normalized.contains("apfs role: backup");
    let has_backupdb = p.join("Backups.backupdb").is_dir();
    let has_manifest = p.join("backup_manifest.plist").is_file();
    let has_tm_dir_marker = fs::read_dir(p).ok().map_or(false, |rd| {
        rd.flatten().any(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.ends_with(".inprogress") || s.ends_with(".previous")
                || s.ends_with(".backupbundle")
        })
    });
    let uuid = volume_uuid(&info);

    // Robust: Vergleiche alle Mountpoints aus tmutil destinationinfo nach canonicalize
    let is_registered = if let Some(u) = uuid.as_ref() {
        registered.iter().any(|r| r.eq_ignore_ascii_case(u))
    } else {
        false
    };

    // Zusätzlich: Wenn der aktuelle Mountpoint (z.B. /Volumes/Backup01 1) mit einem TM-Mountpoint (z.B. /Volumes/Backup01) auf dasselbe Device zeigt, als registered markieren
    let tm_mounts = get_tm_mountpoints();
    let this_canon = fs::canonicalize(mount_point).ok();
    let mut registered_by_mount = false;
    if let Some(this_canon) = &this_canon {
        for tm in &tm_mounts {
            if let Ok(tm_canon) = fs::canonicalize(tm) {
                if tm_canon == *this_canon {
                    registered_by_mount = true;
                    break;
                }
            }
        }
    }

    // Noch robuster: Wenn der aktuelle Mountpoint exakt als String in get_tm_mountpoints() vorkommt, ist es ein aktives TM-Volume
    let mut registered_by_mount_str = false;
    for tm in &tm_mounts {
        // Vergleiche nach canonicalize, aber auch als String (z.B. /Volumes/Backup01 1)
        if tm == mount_point {
            registered_by_mount_str = true;
            break;
        }
    }
    let is_registered = is_registered || registered_by_mount || registered_by_mount_str;

    // NEU: Wenn der Mountpoint exakt in get_tm_mountpoints() steht, immer listen
    let is_tm = role_backup || has_backupdb || has_manifest
        || has_tm_dir_marker || is_registered || registered_by_mount_str;
    if !is_tm { return None; }
    let is_active = is_registered || registered_by_mount_str;
    Some(TmVolume {
        name,
        path: mount_point.to_string(),
        kind: if is_active { "active".to_string() } else { "former".to_string() },
        has_backupdb: has_backupdb || has_manifest || has_tm_dir_marker,
        role_backup,
        registered: is_active,
    })
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

#[tauri::command]
fn tm_list_wipeable_volumes() -> Result<Vec<TmVolume>, String> {
    let all = list_volumes()?;
    let registered = registered_tm_uuids();
    Ok(all.into_iter()
        .filter_map(|v| classify_tm_volume(&v.path, &registered))
        .collect())
}

fn list_local_snapshots_raw() -> Result<Vec<String>, String> {
    let output = std::process::Command::new("tmutil")
        .args(["listlocalsnapshots", "/"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut snaps: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("com.apple.TimeMachine.") {
            let date = rest.trim_end_matches(".local").to_string();
            if !date.is_empty() { snaps.push(date); }
        }
    }
    Ok(snaps)
}

#[tauri::command]
fn tm_list_local_snapshots() -> Result<Vec<String>, String> {
    list_local_snapshots_raw()
}

#[tauri::command]
fn tm_delete_local_snapshot(date: String) -> Result<(), String> {
    if date.is_empty() {
        return Err("Datum ist leer".into());
    }

    let still_present = || -> Result<bool, String> {
        Ok(list_local_snapshots_raw()?.iter().any(|s| s == &date))
    };

    let mut last_output = String::new();

    // 1) ohne Admin versuchen
    let out = std::process::Command::new("tmutil")
        .args(["deletelocalsnapshots", &date])
        .output()
        .map_err(|e| e.to_string())?;
    last_output.push_str(&String::from_utf8_lossy(&out.stdout));
    last_output.push_str(&String::from_utf8_lossy(&out.stderr));
    if !still_present()? {
        return Ok(());
    }

    // 2) mit Admin via osascript (Passwort-Dialog)
    let admin_cmd = format!(
        "/usr/bin/tmutil deletelocalsnapshots {}",
        shell_single_quote(&date)
    );
    match run_with_admin(&admin_cmd) {
        Ok(s) => last_output = format!("{}\n{}", last_output.trim(), s.trim()),
        Err(e) => last_output = format!("{}\n{}", last_output.trim(), e.trim()),
    }
    if !still_present()? {
        return Ok(());
    }

    // 3) Fallback: thinlocalsnapshots für diesen Zeitstempel mit purge=4 als Admin
    //    (wird bei "in use"-Snapshots manchmal nötig)
    let thin_cmd = format!(
        "/usr/bin/tmutil thinlocalsnapshots / 999999999999 4"
    );
    if let Ok(s) = run_with_admin(&thin_cmd) {
        last_output = format!("{}\n{}", last_output.trim(), s.trim());
    }
    if !still_present()? {
        return Ok(());
    }

    Err(format!(
        "Snapshot {} konnte nicht gelöscht werden.\n{}",
        date,
        last_output.trim()
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_drag::init())
        .manage(JobManager::default())
        .manage(WatcherManager::default())
        .setup(|app| {
            promise_drag::init(app.handle());
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
                use tauri::Emitter;

                let about_meta = AboutMetadataBuilder::new()
                    .name(Some("DualBeam"))
                    .version(Some(env!("CARGO_PKG_VERSION").to_string()))
                    .copyright(Some("Copyright © 2026 N.J. — MIT License"))
                    .authors(Some(vec!["N.J.".to_string()]))
                    .license(Some("MIT"))
                    .comments(Some("Erstellt mit Claude Opus / Built with Claude Opus"))
                    .build();

                let app_menu = SubmenuBuilder::new(app, "DualBeam")
                    .about(Some(about_meta))
                    .separator()
                    .hide()
                    .separator()
                    .quit()
                    .build()?;

                let edit_menu = SubmenuBuilder::new(app, "Bearbeiten")
                    .cut()
                    .copy()
                    .paste()
                    .select_all()
                    .build()?;

                let theme_auto = MenuItemBuilder::new("Automatisch (System)")
                    .id("theme-auto")
                    .build(app)?;
                let theme_light = MenuItemBuilder::new("Hell")
                    .id("theme-light")
                    .build(app)?;
                let theme_dark = MenuItemBuilder::new("Dunkel")
                    .id("theme-dark")
                    .build(app)?;

                let view_menu = SubmenuBuilder::new(app, "Ansicht")
                    .item(&theme_auto)
                    .item(&theme_light)
                    .item(&theme_dark)
                    .build()?;

                let lang_auto = MenuItemBuilder::new("Automatisch (System) / Automatic (system)")
                    .id("lang-auto")
                    .build(app)?;
                let lang_de = MenuItemBuilder::new("Deutsch")
                    .id("lang-de")
                    .build(app)?;
                let lang_en = MenuItemBuilder::new("English")
                    .id("lang-en")
                    .build(app)?;

                let lang_menu = SubmenuBuilder::new(app, "Sprache / Language")
                    .item(&lang_auto)
                    .item(&lang_de)
                    .item(&lang_en)
                    .build()?;

                let new_window_item = MenuItemBuilder::new("Neues Fenster")
                    .id("new-window")
                    .accelerator("CmdOrCtrl+N")
                    .build(app)?;

                let window_menu = SubmenuBuilder::new(app, "Fenster")
                    .item(&new_window_item)
                    .separator()
                    .minimize()
                    .maximize()
                    .separator()
                    .close_window()
                    .build()?;

                let menu = MenuBuilder::new(app)
                    .item(&app_menu)
                    .item(&edit_menu)
                    .item(&view_menu)
                    .item(&lang_menu)
                    .item(&window_menu)
                    .build()?;
                app.set_menu(menu)?;

                // macOS injiziert automatisch Text-Service-Einträge (AutoFill,
                // Writing Tools, Emoji & Symbole, Diktat …) ins Bearbeiten-Menü.
                // Diese hier entfernen, nur Standard-Befehle behalten.
                promise_drag::clean_edit_menu();

                // Dock-Menü (Rechtsklick aufs Dock-Symbol) mit „Neues Fenster",
                // analog zum Finder.
                promise_drag::install_dock_menu("Neues Fenster");

                app.on_menu_event(move |app_handle, event| {
                    let id = event.id().as_ref();
                    if id == "new-window" {
                        open_new_window(app_handle);
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
            open_url,
            create_dir,
            create_file,
            create_symlink,
            create_finder_alias,
            rename_path,
            move_to_trash,
            force_delete_admin,
            path_exists,
            list_volumes,
            list_network_bookmarks,
            mount_network_url,
            eject_volume,
            mount_dmg,
            find_dmg_mount,
            detach_dmg,
            quick_look,
            check_conflicts,
            run_job,
            cancel_job,
            sync_preview,
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
            tm_list_destinations,
            tm_list_backups,
            tm_delete_backup,
            tm_wipe_volume,
            tm_list_wipeable_volumes,
            tm_list_local_snapshots,
            tm_delete_local_snapshot,
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
            if let tauri::RunEvent::Reopen { has_visible_windows, .. } = &_event {
                if !*has_visible_windows {
                    open_new_window(_app_handle);
                }
            }
        });
}
