use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::process::{Command, Stdio};

mod object_storage;
mod promise_drag;
mod rdp;
mod remote;
use notify_debouncer_mini::notify::RecommendedWatcher;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, Debouncer};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use walkdir::WalkDir;

// Eine Sync-Vorschau läuft auf einem Blocking-Thread. Der Dialog kann jedoch
// schon geschlossen werden, während WebDAV noch ein sehr großes Verzeichnis
// einliest. Der Thread-lokale Abbruchschalter sorgt dafür, dass dieser Scan
// zeitnah endet und seine offenen Verzeichnis-Handles freigibt.
thread_local! {
    static SYNC_PREVIEW_CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

fn check_sync_preview_cancelled() -> Result<(), String> {
    let cancelled = SYNC_PREVIEW_CANCEL.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|cancel| cancel.load(Ordering::SeqCst))
            .unwrap_or(false)
    });
    if cancelled {
        Err("Synchronisationsvorschau abgebrochen".into())
    } else {
        Ok(())
    }
}

fn run_cancellable_preview<T>(
    cancel: Arc<AtomicBool>,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    SYNC_PREVIEW_CANCEL.with(|slot| *slot.borrow_mut() = Some(cancel));
    let result = work();
    SYNC_PREVIEW_CANCEL.with(|slot| *slot.borrow_mut() = None);
    result
}

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

/// Benutzernamen thread-sicher ermitteln.
///
/// `getpwuid` liefert einen Zeiger auf einen prozessweit geteilten Puffer und
/// ist damit nicht thread-sicher – ein paralleler Aufruf kann den Puffer
/// überschreiben, während wir ihn noch lesen. Deshalb die `_r`-Variante mit
/// eigenem Puffer.
fn lookup_pw_name(uid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 1024];
    loop {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                uid as libc::uid_t,
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && buf.len() < 65536 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
        return Some(cstr.to_string_lossy().into_owned());
    }
}

/// Gruppennamen thread-sicher ermitteln (siehe [`lookup_pw_name`]).
fn lookup_gr_name(gid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; 1024];
    loop {
        let mut grp: libc::group = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::group = std::ptr::null_mut();
        let rc = unsafe {
            libc::getgrgid_r(
                gid as libc::gid_t,
                &mut grp,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && buf.len() < 65536 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        let cstr = unsafe { std::ffi::CStr::from_ptr(grp.gr_name) };
        return Some(cstr.to_string_lossy().into_owned());
    }
}

fn uid_to_name(uid: u32) -> String {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = lock_safe(cache).get(&uid) {
        return v.clone();
    }
    let name = lookup_pw_name(uid).unwrap_or_else(|| uid.to_string());
    lock_safe(cache).insert(uid, name.clone());
    name
}

fn gid_to_name(gid: u32) -> String {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = lock_safe(cache).get(&gid) {
        return v.clone();
    }
    let name = lookup_gr_name(gid).unwrap_or_else(|| gid.to_string());
    lock_safe(cache).insert(gid, name.clone());
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

/// Verzeichnis einlesen. Läuft in einem Worker-Thread: Bei hängenden
/// Netzlaufwerken (WebDAV/SMB) blockieren `read_dir` und die Metadaten-Abfragen
/// je Eintrag bis zum Mount-Timeout und würden sonst die gesamte Oberfläche
/// einfrieren.
#[tauri::command]
async fn list_dir(path: String, show_hidden: bool) -> Result<Vec<Entry>, String> {
    tauri::async_runtime::spawn_blocking(move || list_dir_blocking(path, show_hidden))
        .await
        .map_err(|e| e.to_string())?
}

fn list_dir_blocking(path: String, show_hidden: bool) -> Result<Vec<Entry>, String> {
    let p = expand_tilde(&path);
    // S3 und Swift sind ab Version 0.4.17 direkte DualBeam-Dateiräume. Ihre
    // Inhalte werden über die Objekt-Speicher-API gelesen, nicht über einen
    // NFS-Mount. Der lokale Pfad bleibt lediglich die stabile Kennung für die
    // Pane-Navigation und gespeicherte Sync-Profile.
    if let Some(result) = remote::list_object_storage_dir(&p) {
        let remembered_times = remote::object_directory_times_in(&p);
        return result.map(|entries| {
            entries
                .into_iter()
                .filter(|entry| {
                    (show_hidden || !entry.name.starts_with('.')) && entry.name != ".DualBeamUndo"
                })
                .map(|entry| {
                    let hidden = entry.name.starts_with('.');
                    let ext = if entry.is_dir {
                        String::new()
                    } else {
                        Path::new(&entry.name)
                            .extension()
                            .and_then(|part| part.to_str())
                            .map(|part| part.to_ascii_lowercase())
                            .unwrap_or_default()
                    };
                    let mtime = if entry.is_dir && entry.mtime == 946_684_800 {
                        remembered_times.get(&entry.name).copied().unwrap_or(0)
                    } else {
                        entry.mtime
                    };
                    Entry {
                        name: entry.name,
                        path: entry.path.to_string_lossy().into_owned(),
                        is_dir: entry.is_dir,
                        is_symlink: false,
                        size: if entry.is_dir { 0 } else { entry.size },
                        mtime,
                        ext: ext.clone(),
                        hidden,
                        birth_time: mtime,
                        kind: classify(&ext, entry.is_dir).to_string(),
                        owner: String::new(),
                        group: String::new(),
                        mode_str: String::new(),
                    }
                })
                .collect()
        });
    }
    let read = std::fs::read_dir(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
    // S3/Swift haben keine echten Ordnerobjekte. rclone liefert für virtuelle
    // Präfixe den Default 2000-01-01; das ist kein Erstellungsdatum und wird
    // deshalb als fehlender Zeitwert dargestellt.
    const RCLONE_VIRTUAL_DIRECTORY_TIME: i64 = 946_684_800;
    let is_object_storage = remote::is_object_storage_mount(&p);
    let remembered_object_directory_times = remote::object_directory_times_in(&p);
    // webdavfs kann nach einem Schreibvorgang Einträge ausliefern, deren
    // Metadaten noch nicht auflösbar sind – bis hin zu einem `stat()`, das
    // „No such file or directory" meldet, obwohl der Name im Verzeichnis steht.
    // Solche Lücken werden nach der Schleife aus einer Serverabfrage gefüllt.

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
        let reported_mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mtime =
            if is_object_storage && is_dir && reported_mtime == RCLONE_VIRTUAL_DIRECTORY_TIME {
                remembered_object_directory_times
                    .get(&name)
                    .copied()
                    .unwrap_or(0)
            } else {
                reported_mtime
            };
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
    repair_webdav_entries(&mut out, &p);
    Ok(out)
}

/// Füllt Lücken, die `webdavfs` hinterlässt, aus einer Serverabfrage auf.
///
/// Nach einem Schreibvorgang kann der Treiber Einträge liefern, die zwar im
/// Verzeichnis stehen, deren `stat()` aber `ENOENT` meldet. Größe, Datum,
/// Eigentümer und Rechte fehlen dann – die Datei sieht leer aus, obwohl sie am
/// Server vollständig vorliegt. Ein Neu-Einhängen räumt den Zwischenspeicher
/// auf; bis dahin liefert der Server die richtigen Werte.
///
/// Alles Teure geschieht erst, wenn tatsächlich etwas fehlt: sowohl das
/// Beschaffen der Zugangsdaten – das `/sbin/mount` ausführt und den
/// Schlüsselbund befragt – als auch die Abfrage selbst. Im Normalfall kostet
/// die Anzeige damit keinen einzigen zusätzlichen Zugriff, im Störfall genau
/// eine Abfrage für den gesamten Ordner statt einer je Datei.
fn repair_webdav_entries(entries: &mut [Entry], directory: &Path) {
    let incomplete = entries
        .iter()
        .any(|entry| !entry.is_dir && !entry.is_symlink && (entry.size == 0 || entry.mtime == 0));
    if !incomplete {
        return;
    }
    let Some(context) = webdav_listing_context(directory) else {
        return;
    };
    let Some(server) = webdav_server_directory_entries(&context, directory) else {
        return;
    };
    // Eigentümer und Rechte kennt WebDAV nicht. `webdavfs` vergibt für alle
    // Einträge eines Mounts dieselben Werte (der einhängende Benutzer, Zugriff
    // nur für ihn), weshalb das Verzeichnis selbst als Vorlage taugt.
    use std::os::unix::fs::MetadataExt;
    let directory_meta = std::fs::metadata(directory).ok();
    for entry in entries.iter_mut() {
        if entry.is_dir || entry.is_symlink {
            continue;
        }
        let Some(metadata) = server.get(&entry.name) else {
            continue;
        };
        if entry.size == 0 {
            entry.size = metadata.size;
        }
        if entry.mtime == 0 {
            if let Some(modified) = metadata.modified {
                entry.mtime = modified;
            }
        }
        if entry.birth_time == 0 {
            // Fällt der Server ohne Erstellungsdatum aus, ist der
            // Änderungszeitpunkt die nächstbeste Auskunft.
            entry.birth_time = metadata.created.or(metadata.modified).unwrap_or(0);
        }
        if let Some(meta) = directory_meta.as_ref() {
            if entry.owner.is_empty() {
                entry.owner = uid_to_name(meta.uid());
            }
            if entry.group.is_empty() {
                entry.group = gid_to_name(meta.gid());
            }
            if entry.mode_str.is_empty() {
                entry.mode_str = mode_to_rwx(meta.mode());
            }
        }
    }
}

#[tauri::command]
fn open_default(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    let p = remote::download_object_storage_file(&p)
        .transpose()?
        .unwrap_or(p);
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
    if let Some(result) = remote::object_storage_path_exists(&p) {
        if result? {
            return Err(format!("err.exists\u{1f}{}", p.display()));
        }
        remote::create_object_storage_dir(&p)
            .expect("Objekt-Speicher-Kontext wurde vorab erkannt")?;
        remote::remember_object_directory(&p);
        return Ok(());
    }
    if path_occupied_no_follow(&p) {
        return Err(format!("err.exists\u{1f}{}", p.display()));
    }
    std::fs::create_dir(&p).map_err(|e| format!("{}: {}", p.display(), e))?;
    remote::remember_object_directory(&p);
    Ok(())
}

#[tauri::command]
fn create_file(path: String) -> Result<(), String> {
    let p = expand_tilde(&path);
    if let Some(result) = remote::object_storage_path_exists(&p) {
        if result? {
            return Err(format!("err.exists\u{1f}{}", p.display()));
        }
        remote::create_object_storage_file(&p)
            .expect("Objekt-Speicher-Kontext wurde vorab erkannt")?;
        return Ok(());
    }
    if path_occupied_no_follow(&p) {
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

/// Umbenennen, das ein bestehendes Ziel niemals überschreibt.
///
/// `renamex_np(RENAME_EXCL)` prüft und benennt in einem einzigen Systemaufruf um.
/// Ein vorheriges `exists()` gefolgt von `rename()` hätte eine Lücke, in der ein
/// anderer Prozess die Zieldatei anlegen kann – sie würde dann kommentarlos
/// überschrieben.
fn rename_no_clobber(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let to_c = |p: &Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::other("Pfad enthält ein Nullbyte"))
    };
    let ca = to_c(a)?;
    let cb = to_c(b)?;
    let rc = unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        // Nicht jedes Dateisystem (SMB, WebDAV, FAT …) kennt renamex_np.
        // Dort bleibt nur die Prüfung vorab.
        Some(libc::ENOTSUP) | Some(libc::EINVAL) | Some(libc::ENOSYS) => {
            if path_occupied_no_follow(b) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "Ziel existiert bereits",
                ));
            }
            std::fs::rename(a, b)
        }
        _ => Err(err),
    }
}

#[tauri::command]
fn rename_path(old_path: String, new_path: String) -> Result<(), String> {
    let a = expand_tilde(&old_path);
    let b = expand_tilde(&new_path);
    if a == b {
        return Ok(());
    }
    if let Some(result) = remote::object_storage_path_exists(&a) {
        if !result? {
            return Err(format!("{}: Quelle nicht gefunden", a.display()));
        }
        if remote::object_storage_path_exists(&b)
            .expect("Quellpfad und Zielpfad liegen im selben Objekt-Speicher")?
        {
            return Err(format!("err.exists\u{1f}{}", b.display()));
        }
        return remote::rename_object_storage_path(&a, &b)
            .expect("Objekt-Speicher-Kontext wurde vorab erkannt");
    }
    match rename_no_clobber(&a, &b) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(format!("err.exists\u{1f}{}", b.display()))
        }
        Err(e) => Err(e.to_string()),
    }
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
    // 2) Eindeutige Time-Machine-Pfadbestandteile. Dateiendungen wie
    //    `.inprogress` sind absichtlich keine Kennzeichnung: DualBeam nutzt
    //    sie selbst für abgebrochene Netzwerkübertragungen, ebenso können sie
    //    auf beliebigen Servern vorkommen.
    for comp in full.components() {
        if let std::path::Component::Normal(os) = comp {
            let s = os.to_string_lossy();
            if s.eq_ignore_ascii_case("Backups.backupdb")
                || s == ".timemachine"
                || s == ".MobileBackups"
            {
                return true;
            }
        }
    }
    // 3) Ein übergeordnetes Verzeichnis ist eine TM-Backup-Wurzel (greift auch
    //    bei ehemaligen, nicht mehr registrierten Backup-Volumes). Die
    //    MachineID-Datei markiert Netzwerk-Backups in einem `.sparsebundle`;
    //    dessen Endung allein reicht nicht, da sie auch für gewöhnliche
    //    Image-Dateien verwendet wird.
    let mut cur: Option<&Path> = Some(full);
    let mut depth = 0u32;
    while let Some(p) = cur {
        if depth > 64 {
            break;
        }
        if p.join("backup_manifest.plist").is_file()
            || p.join("Backups.backupdb").is_dir()
            || p.join("com.apple.TimeMachine.MachineID.plist").is_file()
        {
            return true;
        }
        cur = p.parent();
        depth += 1;
    }
    false
}

/// Objekte in den Papierkorb verschieben. Im Worker-Thread, weil `mount` und
/// `tmutil destinationinfo` aufgerufen werden und das Verschieben selbst auf
/// Netzlaufwerken lange dauern kann.
#[tauri::command]
async fn move_to_trash(paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || move_to_trash_blocking(paths))
        .await
        .map_err(|e| e.to_string())?
}

fn move_to_trash_blocking(paths: Vec<String>) -> Result<(), String> {
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

/// Geräte-ID (Volume) eines Pfads. `None`, wenn der Pfad nicht lesbar ist.
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// Volume-Wurzel eines Pfads: nach oben laufen, solange die Geräte-ID gleich
/// bleibt. Für `/Volumes/Extern/Foo/bar` also `/Volumes/Extern`.
fn volume_root_of(path: &Path) -> PathBuf {
    let dev = match device_of(path) {
        Some(dev) => dev,
        None => return PathBuf::from("/"),
    };
    let mut cur = path.to_path_buf();
    loop {
        let parent = match cur.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return cur,
        };
        match device_of(&parent) {
            Some(d) if d == dev => cur = parent,
            _ => return cur,
        }
    }
}

/// Verzeichnis, in das ein zu löschendes Objekt zwischengelagert wird.
///
/// `std::fs::rename` scheitert über Volume-Grenzen hinweg immer mit `EXDEV`.
/// Der Puffer muss deshalb auf demselben Volume wie das Original liegen: für
/// Objekte auf dem Boot-Volume im App-Datenordner, sonst in einem versteckten
/// Ordner auf der Wurzel des jeweiligen Volumes.
fn undo_staging_dir_for(original: &Path, token: &str) -> Result<PathBuf, String> {
    let default = undo_staging_dir(token)?;
    // Eigene rclone-Mounts (SFTP, FTP/FTPS, S3, Swift) erlauben keinen
    // zuverlässigen lokalen Undo-Puffer. Auch wenn die Mount-Tabelle während
    // eines Übergangs kurz leer ist, darf `.DualBeamUndo` dort nie entstehen.
    // Der nachfolgende EXDEV-Fallback führt dann kontrolliert zur dauerhaften
    // Netzlaufwerk-Löschung statt zu einem Berechtigungsfehler.
    if remote::is_remote_mount(original) {
        return Ok(default);
    }
    let parent = original.parent().unwrap_or_else(|| Path::new("/"));
    let app_base =
        dirs::data_local_dir().ok_or_else(|| "Undo-Ordner nicht verfügbar".to_string())?;
    match (device_of(parent), device_of(&app_base)) {
        (Some(a), Some(b)) if a == b => Ok(default),
        (Some(_), _) => Ok(volume_root_of(parent).join(".DualBeamUndo").join(token)),
        // Volume nicht ermittelbar: der App-Ordner ist die sicherere Annahme,
        // ein etwaiger EXDEV-Fehler wird sauber als UNDO_UNAVAILABLE gemeldet.
        _ => Ok(default),
    }
}

/// Löschauswahl in den Rückgängig-Puffer verschieben. Im Worker-Thread wegen
/// `mount`/`tmutil` und der Verschiebeoperationen selbst.
#[tauri::command]
async fn stage_delete_for_undo(paths: Vec<String>) -> Result<UndoDeleteBatch, String> {
    tauri::async_runtime::spawn_blocking(move || stage_delete_for_undo_blocking(paths))
        .await
        .map_err(|e| e.to_string())?
}

fn stage_delete_for_undo_blocking(paths: Vec<String>) -> Result<UndoDeleteBatch, String> {
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
    let dir_default = undo_staging_dir(&token)?;
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
    let mut prepared: Vec<PathBuf> = Vec::new();
    for (index, original) in originals.into_iter().enumerate() {
        let name = original
            .file_name()
            .ok_or_else(|| "Ungültiger Löschpfad".to_string())?;
        // Der Puffer muss auf demselben Volume liegen wie das Original, sonst
        // scheitert `rename` mit EXDEV (externe Platten, gemountete Images).
        let dir = undo_staging_dir_for(&original, &token).unwrap_or_else(|_| dir_default.clone());
        if !prepared.contains(&dir) {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                // Kein beschreibbarer Puffer auf diesem Volume (z. B. schreib-
                // geschützt): ohne Rückgängig-Funktion weiterlöschen statt einen
                // Fehler zu melden, den das Frontend als Rechteproblem deutet.
                for item in items.iter().rev() {
                    let _ = std::fs::rename(&item.staged, &item.original);
                }
                return Err(format!("UNDO_UNAVAILABLE\u{1f}{e}"));
            }
            prepared.push(dir.clone());
        }
        let staged = dir.join(format!("{index}-{}", name.to_string_lossy()));
        if let Err(e) = std::fs::rename(&original, &staged) {
            // Eine teilweise verschobene Auswahl darf nie zurückbleiben.
            for item in items.iter().rev() {
                let _ = std::fs::rename(&item.staged, &item.original);
            }
            if e.raw_os_error() == Some(libc::EXDEV) {
                return Err(format!("UNDO_UNAVAILABLE\u{1f}{e}"));
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
    let mut staging_dirs: Vec<PathBuf> = Vec::new();
    for item in &items {
        let original = PathBuf::from(&item.original);
        let staged = PathBuf::from(&item.staged);
        if let Some(parent) = staged.parent() {
            let parent = parent.to_path_buf();
            if !staging_dirs.contains(&parent) {
                staging_dirs.push(parent);
            }
        }
        if original.exists() {
            return Err(format!("{} existiert bereits", original.display()));
        }
        if let Some(parent) = original.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&staged, &original)
            .map_err(|e| format!("{}: {}", original.display(), e))?;
    }
    remove_empty_staging_dirs(&staging_dirs);
    Ok(())
}

/// Räumt leer gewordene Puffer-Verzeichnisse auf. Fehler sind unerheblich: der
/// periodische `cleanup_expired_undo` fängt Reste später ohnehin ab.
fn remove_empty_staging_dirs(dirs: &[PathBuf]) {
    for dir in dirs {
        let _ = std::fs::remove_dir(dir);
        // Auf fremden Volumes ist `.DualBeamUndo` der gemeinsame Elternordner.
        if let Some(parent) = dir.parent() {
            if parent.file_name().and_then(|n| n.to_str()) == Some(".DualBeamUndo") {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

#[tauri::command]
async fn finalize_staged_delete(items: Vec<UndoDeleteItem>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || finalize_staged_delete_blocking(items))
        .await
        .map_err(|e| e.to_string())?
}

fn finalize_staged_delete_blocking(items: Vec<UndoDeleteItem>) -> Result<(), String> {
    let mut staging_dirs: Vec<PathBuf> = Vec::new();
    for item in &items {
        if let Some(parent) = Path::new(&item.staged).parent() {
            let parent = parent.to_path_buf();
            if !staging_dirs.contains(&parent) {
                staging_dirs.push(parent);
            }
        }
    }
    let staged: Vec<String> = items.into_iter().map(|item| item.staged).collect();
    let result = move_to_trash_blocking(staged);
    remove_empty_staging_dirs(&staging_dirs);
    result
}

/// Entfernt abgelaufene Rückgängig-Puffer aus früheren Sitzungen. Der Puffer
/// liegt im App-Datenordner bzw. – für Objekte auf anderen Volumes – in einem
/// versteckten Ordner auf der jeweiligen Volume-Wurzel. Er wird erst nach zehn
/// Minuten lautlos in den Papierkorb verschoben.
#[tauri::command]
async fn cleanup_expired_undo() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(cleanup_expired_undo_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn cleanup_expired_undo_blocking() -> Result<(), String> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(local) = dirs::data_local_dir() {
        bases.push(local.join("DualBeam").join("Undo"));
    }
    // Puffer auf externen Volumes einsammeln.
    bases.push(PathBuf::from("/.DualBeamUndo"));
    if let Ok(volumes) = std::fs::read_dir("/Volumes") {
        for entry in volumes.filter_map(Result::ok) {
            bases.push(entry.path().join(".DualBeamUndo"));
        }
    }
    let mut paths: Vec<String> = Vec::new();
    for base in &bases {
        let entries = match std::fs::read_dir(base) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        paths.extend(entries.filter_map(Result::ok).filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_dir() {
                return None;
            }
            let age = meta.modified().ok()?.elapsed().ok()?;
            (age >= Duration::from_secs(10 * 60))
                .then(|| entry.path().to_string_lossy().into_owned())
        }));
    }
    if paths.is_empty() {
        return Ok(());
    }
    move_to_trash_blocking(paths)
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

/// Privilegiertes Löschen. Zwingend im Worker-Thread: `osascript` zeigt einen
/// modalen Passwortdialog und kehrt erst nach der Eingabe zurück.
#[tauri::command]
async fn force_delete_admin(paths: Vec<String>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || force_delete_admin_blocking(paths))
        .await
        .map_err(|e| e.to_string())?
}

fn force_delete_admin_blocking(paths: Vec<String>) -> Result<(), String> {
    use std::io::Write;
    // Diagnose-Log nur in Debug-Builds; im Release wird nichts auf die Platte geschrieben.
    // Bewusst im nutzereigenen Ordner statt in /tmp: dort könnte ein anderer
    // Nutzer den Namen vorbelegen und per Symlink auf eine fremde Datei zeigen.
    #[cfg(debug_assertions)]
    let mut log = {
        use std::os::unix::fs::OpenOptionsExt;
        dirs::data_local_dir().and_then(|dir| {
            let dir = dir.join("DualBeam");
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(dir.join("delete.log"))
                .ok()
        })
    };
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
    let path = expand_tilde(&path);
    remote::object_storage_path_exists(&path)
        .map(|result| result.unwrap_or(false))
        .unwrap_or_else(|| path.exists())
}

/// Die oberste sichtbare Ebene eines WebDAV- oder S3-/Swift-Laufwerks.
/// Systempfade oberhalb dieser Grenze sind nur der lokale Träger des Mounts
/// bzw. der internen Objekt-Speicherkennung und gehören nicht zur Navigation
/// eines geöffneten Laufwerks.
#[tauri::command]
fn navigation_root(path: String) -> Option<String> {
    let path = expand_tilde(&path);
    if let Some(root) = remote::object_storage_mount_root(&path) {
        return Some(root.to_string_lossy().into_owned());
    }
    if let Some(root) = remote::sftp_mount_root(&path) {
        return Some(root.to_string_lossy().into_owned());
    }
    mount_fs_types()
        .into_iter()
        .filter(|(mount_path, fstype)| {
            fstype == "webdav" && (path == Path::new(mount_path) || path.starts_with(mount_path))
        })
        .max_by_key(|(mount_path, _)| mount_path.len())
        .map(|(mount_path, _)| mount_path)
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

/// Löst ein System-Netzlaufwerk ohne Gewalt. Ein nicht aushängbarer Mount
/// (etwa wegen eines fremden, noch laufenden Zugriffs) darf den App-Ausstieg
/// nicht verhindern.
fn unmount_system_network_volume(path: &str) {
    let diskutil = Command::new("/usr/sbin/diskutil")
        .args(["unmount", path])
        .output();
    if diskutil.as_ref().is_ok_and(|out| out.status.success()) {
        return;
    }
    let _ = Command::new("/sbin/umount").arg(path).output();
}

/// Beim Beenden gilt eine klare Regel: DualBeam lässt keine Netzverbindung
/// zurück. Zuerst werden die eigenen rclone-Mounts (SFTP, FTP/FTPS, S3,
/// Swift) inklusive ihrer Prozesse beendet. Danach folgen alle über macOS
/// eingehängten Netzwerk-Dateisysteme wie WebDAV und SMB. Lokale Datenträger
/// werden nie berücksichtigt.
fn unmount_all_network_volumes() {
    remote::unmount_all();
    let mut mounts: Vec<String> = mount_fs_types()
        .into_iter()
        .filter_map(|(path, fstype)| is_network_fstype(&fstype).then_some(path))
        // Bei verschachtelten Mounts erst den tieferen Pfad lösen.
        .collect();
    mounts.sort_by_key(|path| std::cmp::Reverse(path.len()));
    for path in mounts {
        unmount_system_network_volume(&path);
    }
}

// Fragt den Kernel unmittelbar nach dem Dateisystemtyp eines Pfads. Anders als
// die Auswertung von `/sbin/mount` hängt das weder an einem externen Prozess
// noch am Zerlegen von dessen Textausgabe: Scheitert der Kommandoaufruf, bliebe
// die Mount-Tabelle leer und ein Netzlaufwerk würde als lokal gelten — mit der
// Folge, dass beim Löschen der Papierkorb benutzt wird und große Dateien dort
// als 0-Byte-Rest zurückbleiben.
// Existiert der Pfad selbst noch nicht (etwa ein geplantes Kopierziel), zählt
// das nächstgelegene vorhandene Elternverzeichnis.
fn statfs_fstype(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let mut candidate = Some(path);
    while let Some(current) = candidate {
        if let Ok(raw) = std::ffi::CString::new(current.as_os_str().as_bytes()) {
            let mut info: libc::statfs = unsafe { std::mem::zeroed() };
            // Sicher: `raw` lebt über den Aufruf hinaus, `info` ist voll
            // initialisiert und wird nur bei Rückgabewert 0 ausgelesen.
            if unsafe { libc::statfs(raw.as_ptr(), &mut info) } == 0 {
                let name = unsafe { std::ffi::CStr::from_ptr(info.f_fstypename.as_ptr()) };
                return name.to_str().ok().map(|s| s.to_ascii_lowercase());
            }
        }
        candidate = current.parent();
    }
    None
}

fn is_network_path(path: &Path, mounts: &std::collections::HashMap<String, String>) -> bool {
    // SFTP/FTP/FTPS werden von DualBeam über rclone unterhalb des eigenen
    // App-Ordners eingehängt. Sie erscheinen nicht zuverlässig in der
    // macOS-Mount-Tabelle, sind aber ebenso Netzwerkziele: Für sie darf weder
    // Papierkorb noch der lokale Undo-Puffer verwendet werden.
    // `statfs` fragt den Kernel direkt und greift daher auch dann, wenn der
    // Aufruf von `/sbin/mount` ausfällt; die Mount-Tabelle bleibt als
    // Rückfallebene erhalten.
    remote::is_remote_mount(path)
        || statfs_fstype(path)
            .map(|fstype| is_network_fstype(&fstype))
            .unwrap_or(false)
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
async fn path_is_network(path: String) -> bool {
    tauri::async_runtime::spawn_blocking(move || {
        let full = expand_tilde(&path);
        let mounts = mount_fs_types();
        is_network_path(&full, &mounts)
    })
    .await
    .unwrap_or(false)
}

#[tauri::command]
async fn list_volumes() -> Result<Vec<Volume>, String> {
    tauri::async_runtime::spawn_blocking(list_volumes_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn list_volumes_blocking() -> Result<Vec<Volume>, String> {
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
            out.push(Volume {
                name,
                path: path_str,
                kind,
            });
        }
    }
    // Netzlaufwerke, die DualBeam selbst über rclone eingehängt hat (SFTP,
    // FTPS). Sie liegen nicht unter /Volumes, weil dort ohne Administratorrechte
    // kein Ordner angelegt werden darf.
    // Eigene Art: Diese Laufwerke sind kein macOS-Mount, sondern laufen über
    // rclone. Sie dürfen deshalb nicht als Netzwerk-Lesezeichen gemerkt werden,
    // denn ihre Mount-Quelle lautet "localhost:/..." und taugt nicht zum
    // erneuten Verbinden.
    for mount in remote::active_mounts() {
        out.push(Volume {
            name: mount.label,
            path: mount.path,
            kind: "remote".to_string(),
        });
    }
    out.sort_by_key(|entry| entry.name.to_lowercase());
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

fn known_network_bookmarks() -> Vec<(String, String, String)> {
    let settings = load_network_bookmark_settings();
    // (name, url, erwarteter Mountpfad) – ausschließlich vom Nutzer angelegte
    // Lesezeichen. `removed_urls` bleibt für Einstellungsdateien älterer
    // Versionen erhalten, damit dort entfernte Einträge entfernt bleiben.
    let mut bookmarks: Vec<(String, String, String)> = Vec::new();
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
    parsed.host_str()?;
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
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| url.clone());
    let mut settings = load_network_bookmark_settings();
    settings.removed_urls.retain(|removed| removed != &url);
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
    let mut settings = load_network_bookmark_settings();
    let custom_count = settings.bookmarks.len();
    settings.bookmarks.retain(|bookmark| bookmark.url != url);
    if settings.bookmarks.len() == custom_count {
        return Err("Unbekanntes Netzwerk-Lesezeichen".into());
    }
    save_network_bookmark_settings(&settings)
}

/// Macht ein bereits von macOS gemountetes Netzlaufwerk zu einem DualBeam-
/// Lesezeichen, damit es nach dem Aushängen in der Seitenleiste bleibt.
#[tauri::command]
fn remember_network_volume(path: String) -> Result<(), String> {
    remember_network_volume_inner(&expand_tilde(&path))
}

pub(crate) fn is_local_network_address(ip: IpAddr) -> bool {
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
        if let Err(e) = stdin.write_all(script.as_bytes()) {
            // Sonst bliebe osascript als Waise hängen und würde ewig auf
            // Eingaben warten.
            let _ = child.kill();
            let _ = child.wait();
            let _ = e;
            return Err("err.mount.failed".to_string());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|_| "err.mount.failed".to_string());
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("err.mount.failed".to_string());
            }
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
        // Von DualBeam selbst eingehängte Netzlaufwerke (SFTP, FTPS) liegen im
        // eigenen App-Ordner. Sie brauchen zusätzlich das Beenden des zugehörigen
        // rclone-Prozesses, sonst bliebe er als Waise zurück.
        if remote::is_remote_mount(&full) {
            return remote::unmount_owned(&full);
        }
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobProgress {
    job_id: String,
    done: u64,
    total: u64,
    files_done: u64,
    /// Byte-basierter Fortschritt eines direkten SFTP-Uploads. Die übrigen
    /// Transferwege bleiben bei ihrer Eintragsanzeige.
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_percent: Option<u8>,
    /// Eine bekannte, aber noch nicht zählbare Serveroperation (etwa SFTP
    /// `purge`) zeigt einen animierten Balken statt eines scheinbar hängenden
    /// 0%-Balkens.
    #[serde(default)]
    indeterminate: bool,
    current: String,
    finished: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct JobManager {
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

// Beendet einen Kindprozess samt seiner Prozessgruppe. Nötig, weil etwa `curl`
// eigene Kindprozesse startet: Ein Signal nur an den Elternprozess ließe eine
// offene Netzwerkverbindung zurück. Fehler sind hier unkritisch – der Prozess
// kann bereits beendet sein.
#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    unsafe {
        let _ = libc::kill(-(pid as i32), libc::SIGTERM);
    }
}

#[tauri::command]
fn check_conflicts(items: Vec<JobItem>) -> Vec<String> {
    items
        .iter()
        .filter(|item| {
            let path = expand_tilde(&item.dst);
            remote::object_storage_path_exists(&path)
                .map(|result| result.unwrap_or(false))
                .unwrap_or_else(|| path_occupied_no_follow(&path))
        })
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
    /// Der macOS-`copyfile`-Schnellpfad kann auf WebDAV 0-Byte-Dateien als
    /// Erfolg melden. Für das jeweilige Ziel wird deshalb ein expliziter,
    /// synchron bestätigter Datenstrom erzwungen.
    target_is_webdav: bool,
    /// URL und lokaler Mountpunkt eines WebDAV-Ziels. Ist diese Information
    /// verfügbar, werden Uploads direkt per HTTP PUT ausgeführt statt durch
    /// den macOS-webdavfs-Treiber zu gehen.
    webdav_target: Option<(String, PathBuf)>,
    /// Dateinamen können sich beim rekursiven Kopieren sehr schnell ändern.
    /// Die UI (und insbesondere die Dock-Markierung) darf dadurch nicht mit
    /// hunderten nativen Aktualisierungen pro Sekunde belastet werden.
    last_emit: Cell<Instant>,
    last_reported_done: Cell<u64>,
}

impl<'a> JobCtx<'a> {
    fn force_synchronous_data_copy(&self) -> bool {
        self.target_is_webdav
    }

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
                transfer_percent: None,
                indeterminate: false,
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

/// Kontext für einen rekursiven Löschvorgang auf einem Netzlaufwerk. WebDAV
/// kann einen einzelnen DELETE-Aufruf nicht unterbrechen, aber zwischen zwei
/// Einträgen wird der Abbruch zuverlässig geprüft. Damit bleibt nach
/// „Abbrechen" nur der bereits entfernte Teilbaum gelöscht.
struct DeleteCtx<'a> {
    app: &'a AppHandle,
    job_id: &'a str,
    cancel: &'a Arc<AtomicBool>,
    done: u64,
    total: u64,
    last_emit: Cell<Instant>,
}

/// Die Oberfläche übergibt diesen Zusatz ausschließlich für Pfade eines
/// aktuell eingehängten S3- oder Swift-Profils. Dann kann der Löschauftrag
/// direkt vom Objekt-Speicher ausgeführt werden, statt jedes Objekt über NFS
/// einzeln zu entfernen.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectStorageDeleteRequest {
    profile: object_storage::ObjectStorageProfile,
    mount_path: String,
    #[serde(default)]
    directory_paths: Vec<String>,
}

/// Bei Kopien liest bzw. schreibt rclone S3/Swift direkt. Der NFS-Mount bleibt
/// ausschließlich die Benutzeroberfläche für die Dateiansicht.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectStorageTransferRequest {
    profile: object_storage::ObjectStorageProfile,
    mount_path: String,
    source_is_object_storage: bool,
}

/// Entsprechender Direkt-Löschauftrag für die über rclone eingehängten
/// FTP-/FTPS-Profile. SFTP wird ausschließlich über SSHFS gelöscht.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStorageDeleteRequest {
    spec: remote::RemoteSpec,
    mount_path: String,
}

impl<'a> DeleteCtx<'a> {
    fn check_cancelled(&self) -> std::io::Result<()> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Löschen abgebrochen",
            ))
        } else {
            Ok(())
        }
    }

    fn removed(&mut self, path: &Path) {
        self.done += 1;
        // Bei sehr großen node_modules-Bäumen nicht für jede Datei ein
        // Webview-Update senden. Der erste und der letzte Eintrag bleiben
        // trotzdem unmittelbar sichtbar.
        if self.done == 1 || self.last_emit.get().elapsed() >= Duration::from_millis(125) {
            self.last_emit.set(Instant::now());
            let _ = self.app.emit(
                "job-progress",
                JobProgress {
                    job_id: self.job_id.to_string(),
                    done: self.done,
                    total: self.total,
                    files_done: self.done,
                    transfer_percent: None,
                    indeterminate: false,
                    current: path.to_string_lossy().into_owned(),
                    finished: false,
                    cancelled: false,
                    error: None,
                },
            );
        }
    }

    /// Meldet, welcher Eintrag gerade bearbeitet wird, ohne den Zähler zu
    /// verändern. Wichtig vor dem WebDAV-Collection-DELETE: Der Server kann für
    /// einen großen Ordner eine Weile brauchen, in der sonst gar nichts
    /// passieren würde.
    fn working_on(&self, path: &Path) {
        self.last_emit.set(Instant::now());
        let _ = self.app.emit(
            "job-progress",
            JobProgress {
                job_id: self.job_id.to_string(),
                done: self.done,
                total: self.total,
                files_done: self.done,
                transfer_percent: None,
                indeterminate: false,
                current: path.to_string_lossy().into_owned(),
                finished: false,
                cancelled: false,
                error: None,
            },
        );
    }

    /// Meldet den Wegfall eines ganzen Teilbaums auf einen Schlag – etwa nach
    /// einem WebDAV-Collection-DELETE, bei dem der Server den kompletten Ordner
    /// in einer Anfrage entfernt. `count` ist die vorab gezählte Zahl der
    /// Einträge dieses Ordners, damit der Fortschritt zur Gesamtzahl passt.
    fn removed_bulk(&mut self, path: &Path, count: u64) {
        self.done += count.max(1);
        self.last_emit.set(Instant::now());
        let _ = self.app.emit(
            "job-progress",
            JobProgress {
                job_id: self.job_id.to_string(),
                done: self.done,
                total: self.total,
                files_done: self.done,
                transfer_percent: None,
                indeterminate: false,
                current: path.to_string_lossy().into_owned(),
                finished: false,
                cancelled: false,
                error: None,
            },
        );
    }
}

/// Zählt vorab alle zu löschenden Einträge (Dateien, Ordner, Symlinks) eines
/// Pfades – analog zur Vorschau beim Kopieren. So kennt die Statusleiste eine
/// echte Gesamtzahl statt „?". Symlinks werden nicht verfolgt (sie zählen wie
/// eine Datei). Reines Auflisten; weit günstiger als das eigentliche Löschen.
fn count_delete_entries(
    p: &Path,
    cancel: &Arc<AtomicBool>,
    deadline: Instant,
) -> std::io::Result<u64> {
    if cancel.load(Ordering::SeqCst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Zählen abgebrochen",
        ));
    }
    // Auf Netzlaufwerken kostet das Auflisten genauso viel wie das Löschen
    // selbst: Jede Ebene ist eine eigene Anfrage. Bei großen Ordnern über
    // WebDAV liefe die Vorschau minutenlang, bevor überhaupt etwas passiert.
    // Nach Ablauf des Zeitbudgets wird deshalb ohne Gesamtzahl weitergemacht –
    // der Fortschritt zählt dann live hoch, statt auf „0" stehen zu bleiben.
    if Instant::now() >= deadline {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Zählen dauert zu lange",
        ));
    }
    let meta = match std::fs::symlink_metadata(p) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut count = 1; // der Knoten selbst
    if meta.is_dir() && !meta.file_type().is_symlink() {
        let entries = match std::fs::read_dir(p) {
            Ok(entries) => entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, std::io::Error>>()?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(count),
            Err(err) => return Err(err),
        };
        for entry_path in entries {
            match count_delete_entries(&entry_path, cancel, deadline) {
                Ok(n) => count += n,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(count)
}

/// `EBUSY` ("Resource busy") ist auf Netzlaufwerken – allen voran macOS'
/// `webdavfs` – kein echter Blockadefehler, sondern ein Übergangszustand: Der
/// Client hält den Eintrag noch im Cache, räumt eine gerade gelöschte
/// Kind-Ressource nach oder wartet auf die Antwort des Servers. Ein Moment
/// später gelingt derselbe Aufruf. Auch `ENOTEMPTY` gehört dazu, weil
/// `webdavfs` ein gecachtes, veraltetes Listing melden kann.
fn is_retryable_remove_error(err: &std::io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EBUSY) | Some(libc::ENOTEMPTY)
    )
}

/// Entfernt einen einzelnen Eintrag und wiederholt den Versuch bei
/// vorübergehenden Netzwerkfehlern mit wachsender Wartezeit.
fn remove_entry_with_retry(p: &Path, is_dir: bool, ctx: &DeleteCtx<'_>) -> std::io::Result<()> {
    const MAX_RETRIES: u32 = 4;
    let mut delay = Duration::from_millis(150);
    let mut attempt = 0;
    loop {
        let res = if is_dir {
            std::fs::remove_dir(p)
        } else {
            std::fs::remove_file(p)
        };
        match res {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) if attempt < MAX_RETRIES && is_retryable_remove_error(&err) => {
                attempt += 1;
                ctx.check_cancelled()?;
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(1200));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Löscht alle Kinder eines Verzeichnisses. Das Listing wird bei jedem Aufruf
/// frisch geholt, damit ein zweiter Durchgang auch Einträge erwischt, die ein
/// veraltetes WebDAV-Listing beim ersten Mal verschwiegen hat.
fn remove_dir_children(p: &Path, ctx: &mut DeleteCtx<'_>) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(p) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, std::io::Error>>()?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry_path in entries {
        ctx.check_cancelled()?;
        match remove_path_cancellable(&entry_path, ctx) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Formt die Fehlermeldung eines fehlgeschlagenen Löschvorgangs. „Resource busy
/// (os error 16)" ist auf Netzlaufwerken irreführend: Es hat niemand die Datei
/// geöffnet, das Volume gibt den Eintrag nur nicht frei. Solche Fälle bekommen
/// eine eigene Kennung, die die Oberfläche in verständlichen Text übersetzt.
fn delete_error_message(path: &Path, err: &std::io::Error) -> String {
    if is_retryable_remove_error(err) {
        return format!("NETWORK_BUSY\u{1f}{}", path.display());
    }
    format!("{}: {}", path.display(), err)
}

fn remove_path_cancellable(p: &Path, ctx: &mut DeleteCtx<'_>) -> std::io::Result<()> {
    ctx.check_cancelled()?;
    let meta = match std::fs::symlink_metadata(p) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if meta.is_dir() && !meta.file_type().is_symlink() {
        // Bleibt das Verzeichnis belegt, hat das Listing womöglich Kinder
        // ausgelassen. Dann noch einmal frisch listen, aufräumen und erneut
        // versuchen – statt den ganzen Auftrag abzubrechen.
        const MAX_PASSES: u32 = 3;
        let mut pass = 1;
        loop {
            remove_dir_children(p, ctx)?;
            match remove_entry_with_retry(p, true, ctx) {
                Ok(()) => break,
                Err(err) if pass < MAX_PASSES && is_retryable_remove_error(&err) => {
                    pass += 1;
                    ctx.check_cancelled()?;
                    std::thread::sleep(Duration::from_millis(400));
                }
                Err(err) => return Err(err),
            }
        }
    } else {
        remove_entry_with_retry(p, false, ctx)?;
    }
    ctx.removed(p);
    Ok(())
}

/// Gemountete WebDAV-Volumes als (Server-URL, Mountpunkt). Anders als
/// `mount_fs_types` behält diese Variante die Quell-URL, damit ein lokaler
/// `/Volumes/...`-Pfad in seine entfernte WebDAV-Adresse übersetzt werden kann.
fn webdav_mounts() -> Vec<(String, PathBuf)> {
    let mut mounts = Vec::new();
    if let Ok(out) = Command::new("/sbin/mount").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            // Format: "<src> on <mountpoint> (<fstype>, ...)"
            for line in s.lines() {
                let Some(on_idx) = line.find(" on ") else {
                    continue;
                };
                let src = line[..on_idx].trim();
                let rest = &line[on_idx + 4..];
                let Some(paren) = rest.rfind(" (") else {
                    continue;
                };
                let mp = &rest[..paren];
                let fstype = rest[paren + 2..]
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(')');
                if fstype == "webdav" && (src.starts_with("https://") || src.starts_with("http://"))
                {
                    mounts.push((src.to_string(), PathBuf::from(mp)));
                }
            }
        }
    }
    mounts
}

/// Wählt das WebDAV-Volume mit dem längsten passenden Mountpunkt-Präfix.
fn best_webdav_mount(mounts: &[(String, PathBuf)], path: &Path) -> Option<(String, PathBuf)> {
    let mut best: Option<(usize, &String, &PathBuf)> = None;
    for (src, mp) in mounts {
        if path.starts_with(mp) {
            let len = mp.as_os_str().len();
            if best.as_ref().map(|(l, _, _)| len > *l).unwrap_or(true) {
                best = Some((len, src, mp));
            }
        }
    }
    best.map(|(_, src, mp)| (src.clone(), mp.clone()))
}

/// Kodiert ein einzelnes Pfadsegment für eine URL. Nur die von RFC 3986 als
/// „unreserved" definierten Zeichen bleiben unverändert; alles andere (inkl.
/// Leerzeichen und Umlaute in UTF-8) wird prozentkodiert.
fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Gegenstück zu [`percent_encode_segment`]: löst Prozentsequenzen wieder auf.
/// Wird für die `href`-Werte einer PROPFIND-Antwort gebraucht, denn dort stehen
/// Dateinamen stets kodiert (`%20`, `%C3%84`).
///
/// Unvollständige oder ungültige Sequenzen bleiben unverändert stehen, statt
/// den Namen zu verwerfen: ein Eintrag mit merkwürdigem Namen ist immer noch
/// besser als ein fehlender Eintrag. Das Ergebnis wird als UTF-8 gelesen;
/// schlägt das fehl, gilt derselbe Grundsatz und der Rohwert bleibt erhalten.
fn percent_decode_segment(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|part| u8::from_str_radix(part, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| segment.to_string())
}

/// Übersetzt einen lokalen WebDAV-Mountpfad in seine entfernte URL. Für Ordner
/// wird ein abschließender Schrägstrich gesetzt, sodass der Server das Element
/// als Collection (rekursiv) löscht.
fn webdav_remote_url(
    source_url: &str,
    mountpoint: &Path,
    path: &Path,
    is_dir: bool,
) -> Option<String> {
    let rel = path.strip_prefix(mountpoint).ok()?;
    let mut url = source_url.trim_end_matches('/').to_string();
    for component in rel.components() {
        let part = component.as_os_str().to_str()?;
        if part.is_empty() || part == "/" {
            continue;
        }
        url.push('/');
        url.push_str(&percent_encode_segment(part));
    }
    // Ohne relatives Segment zeigt die URL auf die Mount-Wurzel selbst – das
    // wäre ein versehentliches Löschen des gesamten Laufwerks. Das lehnen wir ab.
    if url.len() <= source_url.trim_end_matches('/').len() {
        return None;
    }
    if is_dir {
        url.push('/');
    }
    Some(url)
}

/// Extrahiert den Hostnamen aus einer `https://host/...`-URL.
fn webdav_host_from_url(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    let host = after.split('/').next()?;
    // Möglichen `user@`-Präfix und Port entfernen.
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Escaped einen Wert für die curl-Konfigurationsdatei (doppelt gequotet).
fn escape_curl_value(value: &str) -> String {
    // curl-Config-Strings in Anführungszeichen kennen \\, \", \t, \n, \r, \v.
    // Rohe Zeilenumbrüche würden die Direktive beenden und erlauben es,
    // weitere Config-Zeilen einzuschleusen – daher alle escapen.
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0b}' => out.push_str("\\v"),
            // Übrige Steuerzeichen sind in Zugangsdaten/URLs nicht legitim
            // und in der Config nicht sicher darstellbar → entfernen.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Liest Benutzernamen und Kennwort des WebDAV-Mounts aus dem macOS-Schlüssel-
/// bund. Das Kennwort wird über einen separaten Aufruf geholt und nie geloggt.
#[cfg(target_os = "macos")]
fn webdav_credentials(host: &str) -> Option<(String, String)> {
    let attrs = Command::new("/usr/bin/security")
        .args(["find-internet-password", "-s", host])
        .output()
        .ok()?;
    if !attrs.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&attrs.stdout);
    let account = text.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("\"acct\"<blob>=\"")?;
        rest.strip_suffix('"').map(|s| s.to_string())
    })?;
    if account.is_empty() {
        return None;
    }
    let pw = Command::new("/usr/bin/security")
        .args(["find-internet-password", "-s", host, "-a", &account, "-w"])
        .output()
        .ok()?;
    if !pw.status.success() {
        return None;
    }
    let password = String::from_utf8_lossy(&pw.stdout)
        .strip_suffix('\n')
        .map(|s| s.to_string())
        .unwrap_or_else(|| String::from_utf8_lossy(&pw.stdout).to_string());
    if password.is_empty() {
        return None;
    }
    Some((account, password))
}

/// Führt einen WebDAV-curl-Aufruf aus, ohne URL oder Kennwort in der
/// Prozessliste bzw. auf der Platte zu hinterlassen. Die Config wird nur über
/// stdin übergeben. Der Benutzer kann eine eigene `~/.curlrc` besitzen; sie
/// darf einen Dateimanager-Upload nicht beeinflussen, daher wird curl mit
/// `--disable` gestartet.
#[cfg(target_os = "macos")]
fn run_webdav_curl(config: String, cancel: &AtomicBool) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("/usr/bin/curl");
    command
        .args(["--disable", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(config.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output(),
            Ok(None) if cancel.load(Ordering::SeqCst) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(error),
        }
    }
}

/// Der HTTP-Code wird am Ende jeder curl-Ausgabe mit einem eindeutigen Marker
/// ausgegeben. Die eigentlichen Header dürfen dann unverändert bleiben und
/// sicher auf `Content-Length` untersucht werden.
#[cfg(target_os = "macos")]
fn webdav_response_code(stdout: &[u8]) -> u16 {
    String::from_utf8_lossy(stdout)
        .rsplit("__DUALBEAM_HTTP_CODE__:")
        .next()
        .and_then(|part| part.trim().parse().ok())
        .unwrap_or(0)
}

/// Liest die vom Server bestätigte Dateigröße aus einer WebDAV-PROPFIND-
/// Antwort. Die lokale Größe des webdavfs-Mounts genügt nicht: genau dieser
/// Treiber kann einen leeren Cache-Platzhalter als vollständige Datei anzeigen.
/// Ein gewöhnliches HTTP-HEAD wird von pCloud für diesen WebDAV-Endpunkt trotz
/// vorhandener Objekte mit 404 beantwortet und ist daher keine Prüfung.
fn webdav_propfind_tag_value<'a>(response: &'a str, tag: &str) -> Option<&'a str> {
    let lower = response.to_ascii_lowercase();
    let tag_start = lower.find(&tag.to_ascii_lowercase())?;
    let value_start = response[tag_start..].find('>')? + tag_start + 1;
    let value_end = response[value_start..].find('<')? + value_start;
    Some(response[value_start..value_end].trim())
}

fn webdav_propfind_content_length(response: &str) -> Option<u64> {
    webdav_propfind_tag_value(response, "getcontentlength")?
        .parse()
        .ok()
}

/// WebDAV verwendet für `getlastmodified` einen HTTP-Tag. Die Umrechnung
/// erfolgt absichtlich ohne zusätzliche Bibliothek; gültig sind die von
/// RFC 7231 definierten GMT-Werte (z. B. `Wed, 21 Oct 2015 07:28:00 GMT`).
fn webdav_http_date_epoch(value: &str) -> Option<i64> {
    let parts: Vec<_> = value.split_whitespace().collect();
    if parts.len() != 6 || parts[5] != "GMT" {
        return None;
    }
    let day: i64 = parts[1].parse().ok()?;
    let month: i64 = match parts[2] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts[3].parse().ok()?;
    let mut time = parts[4].split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;
    if time.next().is_some()
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    // Tage seit 1970-01-01 (proleptischer gregorianischer Kalender).
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let month_of_year = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some((era * 146_097 + day_of_era - 719_468) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn webdav_propfind_last_modified(response: &str) -> Option<i64> {
    webdav_http_date_epoch(webdav_propfind_tag_value(response, "getlastmodified")?)
}

/// Sucht ab `from` das nächste XML-Tag mit dem lokalen Namen `name`.
///
/// Der Namensraumpräfix ist serverabhängig (`<D:response>`, `<response>`,
/// `<ns0:response>`), deshalb wird nur der Teil hinter dem letzten Doppelpunkt
/// verglichen. `lower` muss die kleingeschriebene Fassung des Originals sein;
/// `to_ascii_lowercase` lässt die Bytelänge unverändert, sodass die
/// zurückgegebene Position auch im Original gilt.
fn find_local_tag(lower: &str, from: usize, name: &str, closing: bool) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut cursor = from;
    while let Some(relative) = lower.get(cursor..)?.find('<') {
        let open = cursor + relative;
        let mut scan = open + 1;
        if closing {
            if bytes.get(scan) != Some(&b'/') {
                cursor = open + 1;
                continue;
            }
            scan += 1;
        } else if bytes.get(scan) == Some(&b'/') {
            cursor = open + 1;
            continue;
        }
        let relative = lower
            .get(scan..)
            .and_then(|rest| rest.find(|c: char| c == '>' || c == '/' || c.is_whitespace()))?;
        let end = scan + relative;
        let tag = lower.get(scan..end)?;
        if tag.rsplit(':').next().unwrap_or(tag) == name {
            return Some(open);
        }
        cursor = open + 1;
    }
    None
}

/// Zerlegt eine PROPFIND-Antwort in die Rümpfe ihrer `<response>`-Elemente.
///
/// Nötig, weil [`webdav_propfind_tag_value`] stets den ersten Treffer im
/// gesamten Text liefert. Bei `Depth: 0` ist das richtig, bei einem
/// Verzeichnislisting (`Depth: 1`) bekäme man sonst für jede Datei die Werte
/// des Verzeichnisses selbst.
fn webdav_propfind_response_blocks(response: &str) -> Vec<&str> {
    let lower = response.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some(open) = find_local_tag(&lower, cursor, "response", false) {
        let Some(body_start) = response[open..].find('>').map(|offset| open + offset + 1) else {
            break;
        };
        let Some(close) = find_local_tag(&lower, body_start, "response", true) else {
            break;
        };
        blocks.push(&response[body_start..close]);
        cursor = close + 1;
    }
    blocks
}

/// Löst die fünf vordefinierten XML-Entitäten sowie numerische Referenzen auf.
/// Unbekannte Sequenzen bleiben unverändert, damit ein ungewöhnlicher Name den
/// Eintrag nicht verliert.
fn xml_unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(position) = rest.find('&') {
        out.push_str(&rest[..position]);
        let tail = &rest[position..];
        let resolved = tail.find(';').and_then(|end| {
            let entity = &tail[1..end];
            let character = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => entity.strip_prefix('#').and_then(|number| {
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)
                }),
            }?;
            Some((character, end + 1))
        });
        match resolved {
            Some((character, consumed)) => {
                out.push(character);
                rest = &tail[consumed..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Zieht den Dateinamen aus einem `href`. Der Wert ist prozentkodiert und je
/// nach Server absolut (`/ordner/datei.png`) oder vollständig
/// (`https://host/ordner/datei.png`); für beide Formen genügt das letzte
/// Segment, da ein `Depth: 1`-Listing nur direkte Kinder enthält.
fn webdav_href_file_name(href: &str) -> Option<String> {
    let trimmed = xml_unescape(href.trim());
    let trimmed = trimmed.trim_end_matches('/');
    let last = trimmed.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    let decoded = percent_decode_segment(last);
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

#[derive(Clone, Copy)]
struct WebDavFileMetadata {
    size: u64,
    modified: Option<i64>,
    created: Option<i64>,
}

/// Liest `creationdate`. WebDAV schreibt hier – anders als bei
/// `getlastmodified` – einen ISO-8601-Zeitstempel.
fn webdav_propfind_created(response: &str) -> Option<i64> {
    let value = webdav_propfind_tag_value(response, "creationdate")?;
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp())
}

/// Wertet ein `Depth: 1`-Listing aus und ordnet jedem Dateinamen die vom
/// Server gemeldeten Werte zu. Ordner werden übersprungen – für sie liefert
/// WebDAV keine Größe, und ihr Zeitstempel ist für die Anzeige belanglos.
fn webdav_propfind_entries(response: &str) -> HashMap<String, WebDavFileMetadata> {
    let mut entries = HashMap::new();
    for block in webdav_propfind_response_blocks(response) {
        let lower = block.to_ascii_lowercase();
        if find_local_tag(&lower, 0, "collection", false).is_some() {
            continue;
        }
        let Some(href) = webdav_propfind_tag_value(block, "href") else {
            continue;
        };
        let Some(name) = webdav_href_file_name(href) else {
            continue;
        };
        let Some(size) = webdav_propfind_content_length(block) else {
            continue;
        };
        entries.insert(
            name,
            WebDavFileMetadata {
                size,
                modified: webdav_propfind_last_modified(block),
                created: webdav_propfind_created(block),
            },
        );
    }
    entries
}

#[cfg(target_os = "macos")]
fn webdav_propfind_response(
    url: &str,
    user: &str,
    password: &str,
    cancel: &AtomicBool,
) -> std::io::Result<(Vec<u8>, u16)> {
    webdav_propfind_response_with_depth(url, user, password, 0, cancel)
}

/// `depth` entspricht dem gleichnamigen WebDAV-Header: 0 fragt genau ein
/// Element ab, 1 zusätzlich dessen direkte Kinder.
#[cfg(target_os = "macos")]
fn webdav_propfind_response_with_depth(
    url: &str,
    user: &str,
    password: &str,
    depth: u8,
    cancel: &AtomicBool,
) -> std::io::Result<(Vec<u8>, u16)> {
    let config = format!(
        "silent\nshow-error\nrequest = \"PROPFIND\"\nheader = \"Depth: {depth}\"\noutput = \"-\"\nwrite-out = \"\\n__DUALBEAM_HTTP_CODE__:%{{http_code}}\\n\"\nurl = \"{}\"\nuser = \"{}:{}\"\n",
        escape_curl_value(url),
        escape_curl_value(user),
        escape_curl_value(password),
    );
    let output = run_webdav_curl(config, cancel)?;
    let code = webdav_response_code(&output.stdout);
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(std::io::Error::other(if detail.is_empty() {
            format!("WebDAV-Serverabfrage fehlgeschlagen (HTTP {code})")
        } else {
            format!("WebDAV-Serverabfrage fehlgeschlagen: {detail}")
        }));
    }
    Ok((output.stdout, code))
}

#[cfg(target_os = "macos")]
fn webdav_file_metadata_optional(
    url: &str,
    user: &str,
    password: &str,
    cancel: &AtomicBool,
) -> std::io::Result<Option<WebDavFileMetadata>> {
    let (stdout, code) = webdav_propfind_response(url, user, password, cancel)?;
    if code == 404 {
        return Ok(None);
    }
    if code != 207 {
        return Err(std::io::Error::other(format!(
            "WebDAV-Serverabfrage fehlgeschlagen (HTTP {code})"
        )));
    }
    let body = String::from_utf8_lossy(&stdout);
    let body = body
        .rsplit_once("__DUALBEAM_HTTP_CODE__:")
        .map(|(body, _)| body)
        .unwrap_or(&body);
    let size = webdav_propfind_content_length(body)
        .ok_or_else(|| std::io::Error::other("WebDAV-Server hat keine Dateigröße bestätigt"))?;
    Ok(Some(WebDavFileMetadata {
        size,
        modified: webdav_propfind_last_modified(body),
        created: webdav_propfind_created(body),
    }))
}

#[cfg(target_os = "macos")]
fn webdav_file_metadata(
    url: &str,
    user: &str,
    password: &str,
    cancel: &AtomicBool,
) -> std::io::Result<WebDavFileMetadata> {
    webdav_file_metadata_optional(url, user, password, cancel)?.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "WebDAV-Datei nicht gefunden")
    })
}

#[cfg(target_os = "macos")]
fn webdav_path_exists(
    url: &str,
    user: &str,
    password: &str,
    cancel: &AtomicBool,
) -> std::io::Result<bool> {
    let (_, code) = webdav_propfind_response(url, user, password, cancel)?;
    match code {
        207 => Ok(true),
        404 => Ok(false),
        _ => Err(std::io::Error::other(format!(
            "WebDAV-Serverabfrage fehlgeschlagen (HTTP {code})"
        ))),
    }
}

#[cfg(target_os = "macos")]
fn webdav_file_size(
    url: &str,
    user: &str,
    password: &str,
    cancel: &AtomicBool,
) -> std::io::Result<u64> {
    webdav_file_metadata(url, user, password, cancel).map(|metadata| metadata.size)
}

/// Zugangsdaten und Zielzuordnung für eine Verzeichnisanzeige. Die Daten
/// werden einmal je Listing gelesen; einzelne Cache-Platzhalter können dann
/// ohne erneuten Schlüsselbundzugriff direkt am Server geprüft werden.
struct WebDavListingContext {
    source_url: String,
    mountpoint: PathBuf,
    user: String,
    password: String,
}

#[cfg(target_os = "macos")]
fn webdav_listing_context(directory: &Path) -> Option<WebDavListingContext> {
    let (source_url, mountpoint) = best_webdav_mount(&webdav_mounts(), directory)?;
    let host = webdav_host_from_url(&source_url)?;
    let (user, password) = webdav_credentials(&host)?;
    Some(WebDavListingContext {
        source_url,
        mountpoint,
        user,
        password,
    })
}

#[cfg(not(target_os = "macos"))]
fn webdav_listing_context(_directory: &Path) -> Option<WebDavListingContext> {
    None
}

/// Baut die URL eines Verzeichnisses für ein Listing.
///
/// Anders als [`webdav_remote_url`] ist die Mount-Wurzel hier ausdrücklich
/// erlaubt: jene Funktion verweigert sie, weil ein Löschbefehl auf die Wurzel
/// das gesamte Laufwerk träfe. Ein Lesezugriff ist dagegen harmlos.
#[cfg(target_os = "macos")]
fn webdav_directory_url(context: &WebDavListingContext, directory: &Path) -> Option<String> {
    if directory == context.mountpoint {
        return Some(format!("{}/", context.source_url.trim_end_matches('/')));
    }
    webdav_remote_url(&context.source_url, &context.mountpoint, directory, true)
}

/// Holt die Serverwerte für ein ganzes Verzeichnis in einer einzigen Abfrage.
///
/// Hintergrund: `webdavfs` kann nach einem Schreibvorgang Einträge liefern, die
/// zwar im Verzeichnis stehen, deren `stat()` aber „No such file or directory"
/// meldet. Größe und Datum fehlen dann. Ein `Depth: 1`-PROPFIND beantwortet den
/// gesamten Ordner mit einem Roundtrip – deutlich günstiger, als jede Datei
/// einzeln nachzufragen.
///
/// Ein Fehler bleibt bewusst unsichtbar: die Pane zeigt dann weiterhin die
/// Werte des Betriebssystems, statt dass ein Serverproblem die Ansicht
/// blockiert.
#[cfg(target_os = "macos")]
fn webdav_server_directory_entries(
    context: &WebDavListingContext,
    directory: &Path,
) -> Option<HashMap<String, WebDavFileMetadata>> {
    let url = webdav_directory_url(context, directory)?;
    let cancel = AtomicBool::new(false);
    let (stdout, code) =
        webdav_propfind_response_with_depth(&url, &context.user, &context.password, 1, &cancel)
            .ok()?;
    if code != 207 {
        return None;
    }
    let body = String::from_utf8_lossy(&stdout);
    let body = body
        .rsplit_once("__DUALBEAM_HTTP_CODE__:")
        .map(|(body, _)| body)
        .unwrap_or(&body);
    // Ein Verzeichnis ohne Dateien liefert eine leere Auskunft – das ist ein
    // gültiges Ergebnis und kein Fehler.
    Some(webdav_propfind_entries(body))
}

#[cfg(not(target_os = "macos"))]
fn webdav_server_directory_entries(
    _context: &WebDavListingContext,
    _directory: &Path,
) -> Option<HashMap<String, WebDavFileMetadata>> {
    None
}

#[cfg(target_os = "macos")]
fn webdav_server_file_metadata_result(
    context: &WebDavListingContext,
    path: &Path,
) -> std::io::Result<Option<WebDavFileMetadata>> {
    let url = webdav_remote_url(&context.source_url, &context.mountpoint, path, false).ok_or_else(
        || std::io::Error::new(std::io::ErrorKind::InvalidInput, "ungültiger WebDAV-Pfad"),
    )?;
    let cancel = AtomicBool::new(false);
    webdav_file_metadata_optional(&url, &context.user, &context.password, &cancel)
}

#[cfg(target_os = "macos")]
fn webdav_server_path_exists(context: &WebDavListingContext, path: &Path) -> std::io::Result<bool> {
    let url = webdav_remote_url(&context.source_url, &context.mountpoint, path, true).ok_or_else(
        || std::io::Error::new(std::io::ErrorKind::InvalidInput, "ungültiger WebDAV-Pfad"),
    )?;
    let cancel = AtomicBool::new(false);
    webdav_path_exists(&url, &context.user, &context.password, &cancel)
}

#[cfg(not(target_os = "macos"))]
fn webdav_server_file_metadata_result(
    _context: &WebDavListingContext,
    _path: &Path,
) -> std::io::Result<Option<WebDavFileMetadata>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn webdav_server_path_exists(
    _context: &WebDavListingContext,
    _path: &Path,
) -> std::io::Result<bool> {
    Ok(false)
}

/// Legt eine WebDAV-Collection unmittelbar auf dem Server an. Auch dies darf
/// nicht über `webdavfs` laufen: Der macOS-Mount bestätigt ein `mkdir` teils
/// nur in seinem lokalen Cache. Beim anschließenden Upload eines kompletten
/// Verzeichnisses entstanden so leere Platzhalterdateien.
///
/// `405 Method Not Allowed` bedeutet bei MKCOL üblicherweise, dass die
/// Collection bereits existiert. Das ist für den rekursiven Kopierer genau der
/// gewünschte Zustand und daher kein Fehler.
#[cfg(target_os = "macos")]
fn webdav_create_collection(
    source_url: &str,
    mountpoint: &Path,
    destination: &Path,
    cancel: &AtomicBool,
) -> Option<std::io::Result<()>> {
    let host = webdav_host_from_url(source_url)?;
    let url = webdav_remote_url(source_url, mountpoint, destination, true)?;
    let (user, password) = webdav_credentials(&host)?;
    if cancel.load(Ordering::SeqCst) {
        return Some(Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        )));
    }
    let config = format!(
        "silent\nshow-error\nrequest = \"MKCOL\"\noutput = \"/dev/null\"\nwrite-out = \"__DUALBEAM_HTTP_CODE__:%{{http_code}}\"\nurl = \"{}\"\nuser = \"{}:{}\"\n",
        escape_curl_value(&url),
        escape_curl_value(&user),
        escape_curl_value(&password),
    );
    Some((|| -> std::io::Result<()> {
        let output = run_webdav_curl(config, cancel)?;
        let code = webdav_response_code(&output.stdout);
        if output.status.success() && matches!(code, 200 | 201 | 204 | 405) {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(std::io::Error::other(if detail.is_empty() {
            format!("WebDAV-Ordner konnte nicht angelegt werden (HTTP {code})")
        } else {
            format!("WebDAV-Ordner konnte nicht angelegt werden: {detail}")
        }))
    })())
}

/// Stellt die gesamte Collection-Kette bis zum Elternordner einer Datei
/// serverseitig bereit. Die Synchronisierung übergibt häufig nur einzelne
/// geänderte Dateien (`Desktop/datei.pdf`), nicht deren Ordner. Ein PUT darf
/// dann nicht mit HTTP 409 scheitern, nur weil `Desktop` noch nicht existiert.
#[cfg(target_os = "macos")]
fn webdav_create_parent_collections(
    source_url: &str,
    mountpoint: &Path,
    destination: &Path,
    cancel: &AtomicBool,
) -> Option<std::io::Result<()>> {
    let parent = destination.parent()?;
    let relative_parent = parent.strip_prefix(mountpoint).ok()?;
    let mut current = mountpoint.to_path_buf();
    for component in relative_parent.components() {
        current.push(component.as_os_str());
        match webdav_create_collection(source_url, mountpoint, &current, cancel) {
            Some(Ok(())) => {}
            Some(Err(error)) => return Some(Err(error)),
            None => return None,
        }
    }
    Some(Ok(()))
}

/// Überträgt eine einzelne lokale Datei unmittelbar per WebDAV-PUT. Der
/// Mount-Treiber wird bewusst umgangen: `webdavfs` kann auf macOS ENFILE
/// melden, obwohl der aufrufende Prozess kaum Dateideskriptoren besitzt.
///
/// Die Datei wird unmittelbar unter ihrem endgültigen Namen hochgeladen und
/// der Server bestätigt die Größe danach per HEAD. Ein serverseitiges MOVE
/// wird absichtlich nicht verwendet: Zusammen mit einer über webdavfs
/// angelegten Collection konnte pCloud dabei Inhalte auf die falsche Ebene
/// verschieben und im Ziel leere Platzhalter zurücklassen.
/// `None` bedeutet, dass URL oder Schlüsselbundzugang nicht ermittelt werden
/// konnten; der Aufrufer darf dann auf den Dateisystemweg zurückfallen.
#[cfg(target_os = "macos")]
fn webdav_upload_file(
    source: &Path,
    destination: &Path,
    source_url: &str,
    mountpoint: &Path,
    cancel: &AtomicBool,
) -> Option<std::io::Result<()>> {
    let host = webdav_host_from_url(source_url)?;
    let target_url = webdav_remote_url(source_url, mountpoint, destination, false)?;
    let (user, password) = webdav_credentials(&host)?;
    let expected_size = match std::fs::metadata(source) {
        Ok(metadata) => metadata.len(),
        Err(error) => return Some(Err(error)),
    };
    if cancel.load(Ordering::SeqCst) {
        return Some(Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        )));
    }

    // Alles Sensible (URL mit möglichem Benutzeranteil und Kennwort) wird nur
    // über stdin an curl übergeben. In der Prozessliste steht lediglich
    // `curl --config -`.
    let config = format!(
        "silent\nshow-error\nfail\nupload-file = \"{}\"\nconnect-timeout = \"30\"\nmax-time = \"1800\"\noutput = \"/dev/null\"\nwrite-out = \"__DUALBEAM_HTTP_CODE__:%{{http_code}}\"\nurl = \"{}\"\nuser = \"{}:{}\"\n",
        escape_curl_value(&source.to_string_lossy()),
        escape_curl_value(&target_url),
        escape_curl_value(&user),
        escape_curl_value(&password),
    );
    let output = match run_webdav_curl(config, cancel) {
        Ok(output) => output,
        Err(error) => return Some(Err(error)),
    };
    let code = webdav_response_code(&output.stdout);
    if !output.status.success() || !matches!(code, 200 | 201 | 204) {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Some(Err(std::io::Error::other(if detail.is_empty() {
            format!("WebDAV-Upload wurde mit HTTP {code} abgelehnt")
        } else {
            format!("WebDAV-Upload fehlgeschlagen: {detail}")
        })));
    }
    let result = webdav_file_size(&target_url, &user, &password, cancel).and_then(|uploaded_size| {
        if uploaded_size == expected_size {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                format!("WebDAV-Upload unvollständig: erwartet {expected_size} Byte, Server meldet {uploaded_size} Byte"),
            ))
        }
    });
    // Nur nach einem erfolgreichen PUT entfernen wir ein unvollständiges
    // Ziel. Bei einer abgelehnten Anfrage bleibt eine eventuell vorhandene
    // frühere Datei dadurch unangetastet.
    if result.is_err() && !cancel.load(Ordering::SeqCst) {
        let cleanup = format!(
            "silent\nshow-error\nrequest = \"DELETE\"\noutput = \"/dev/null\"\nurl = \"{}\"\nuser = \"{}:{}\"\n",
            escape_curl_value(&target_url),
            escape_curl_value(&user),
            escape_curl_value(&password),
        );
        let _ = run_webdav_curl(cleanup, cancel);
    }
    Some(result)
}

#[cfg(not(target_os = "macos"))]
fn webdav_upload_file(
    _source: &Path,
    _destination: &Path,
    _source_url: &str,
    _mountpoint: &Path,
    _cancel: &AtomicBool,
) -> Option<std::io::Result<()>> {
    None
}

/// Löscht einen Ordner auf einem WebDAV-Laufwerk mit einer einzigen
/// `DELETE`-Anfrage auf die Collection. Der Server entfernt den gesamten
/// Unterbaum in einem Schritt (RFC 4918), statt tausende Einzel-Requests über
/// das gemountete Dateisystem zu senden. Gibt `true` zurück, wenn der Ordner
/// dadurch vollständig entfernt wurde; andernfalls `false`, damit der Aufrufer
/// auf das rekursive Einzel-Löschen zurückfällt.
///
/// Dateien und Ordner auf WebDAV dürfen niemals über den macOS-Mount gelöscht
/// werden: webdavfs kann einen gelöschten Eintrag noch minutenlang cachen oder
/// „Resource busy“ liefern. Der direkte Serverweg ist deshalb der verbindliche
/// Löschweg für jedes Element auf einem erkannten WebDAV-Laufwerk.
#[cfg(target_os = "macos")]
fn webdav_delete_path(
    path: &Path,
    mounts: &[(String, PathBuf)],
    creds_cache: &mut HashMap<String, Option<(String, String)>>,
    cancel: &AtomicBool,
) -> Option<std::io::Result<()>> {
    let (source_url, mountpoint) = best_webdav_mount(mounts, path)?;
    if path.strip_prefix(&mountpoint).ok()?.as_os_str().is_empty() {
        return Some(Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Das Stammverzeichnis des WebDAV-Laufwerks kann nicht gelöscht werden",
        )));
    }
    let host = webdav_host_from_url(&source_url)?;
    let is_dir = std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false);
    let url = webdav_remote_url(&source_url, &mountpoint, path, is_dir)?;
    let creds = creds_cache
        .entry(host.clone())
        .or_insert_with(|| webdav_credentials(&host))
        .clone()?;
    if cancel.load(Ordering::SeqCst) {
        return Some(Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        )));
    }
    let (user, password) = creds;
    let config = format!(
        "silent\nshow-error\nrequest = \"DELETE\"\nconnect-timeout = \"30\"\nmax-time = \"1800\"\noutput = \"/dev/null\"\nwrite-out = \"__DUALBEAM_HTTP_CODE__:%{{http_code}}\"\nurl = \"{}\"\nuser = \"{}:{}\"\n",
        escape_curl_value(&url),
        escape_curl_value(&user),
        escape_curl_value(&password),
    );
    let output = match run_webdav_curl(config, cancel) {
        Ok(output) => output,
        Err(error) => return Some(Err(error)),
    };
    let code = webdav_response_code(&output.stdout);
    if output.status.success() && matches!(code, 200 | 202 | 204 | 404) {
        return Some(Ok(()));
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Some(Err(std::io::Error::other(if detail.is_empty() {
        format!("WebDAV-Löschen wurde mit HTTP {code} abgelehnt")
    } else {
        format!("WebDAV-Löschen fehlgeschlagen: {detail}")
    })))
}

#[cfg(not(target_os = "macos"))]
fn webdav_delete_path(
    _path: &Path,
    _mounts: &[(String, PathBuf)],
    _creds_cache: &mut HashMap<String, Option<(String, String)>>,
    _cancel: &AtomicBool,
) -> Option<std::io::Result<()>> {
    None
}

#[cfg(target_os = "macos")]
fn webdav_collection_delete(
    path: &Path,
    mounts: &[(String, PathBuf)],
    creds_cache: &mut HashMap<String, Option<(String, String)>>,
    cancel: &Arc<AtomicBool>,
) -> bool {
    let Some((source_url, mountpoint)) = best_webdav_mount(mounts, path) else {
        return false;
    };
    let Some(host) = webdav_host_from_url(&source_url) else {
        return false;
    };
    let Some(url) = webdav_remote_url(&source_url, &mountpoint, path, true) else {
        return false;
    };
    let creds = creds_cache
        .entry(host.clone())
        .or_insert_with(|| webdav_credentials(&host))
        .clone();
    let Some((user, password)) = creds else {
        return false;
    };
    if cancel.load(Ordering::SeqCst) {
        return false;
    }
    // curl liest Ziel und Zugangsdaten aus der Konfiguration auf stdin, damit
    // das Kennwort weder in der Prozessliste (argv) noch auf der Platte landet.
    let config = format!(
        "silent\nshow-error\nrequest = \"DELETE\"\nconnect-timeout = \"30\"\nmax-time = \"1800\"\noutput = \"/dev/null\"\nwrite-out = \"%{{http_code}}\"\nurl = \"{}\"\nuser = \"{}:{}\"\n",
        escape_curl_value(&url),
        escape_curl_value(&user),
        escape_curl_value(&password),
    );
    let mut command = Command::new("/usr/bin/curl");
    command
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return false,
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(config.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    }
    let pid = child.id();
    // Warten, aber weiterhin auf Abbruch reagieren: curl (und damit die offene
    // Verbindung) wird bei Abbruch samt Prozessgruppe beendet.
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if cancel.load(Ordering::SeqCst) {
                    #[cfg(unix)]
                    terminate_process_group(pid);
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return false,
        }
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(_) => return false,
    };
    if !output.status.success() {
        return false;
    }
    let code: u16 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    // 2xx = erfolgreich gelöscht, 404 = war bereits weg (idempotent). 207
    // (Multi-Status) kann Teilfehler im Body verstecken und zählt daher NICHT
    // als Erfolg. Alles außer den Codes unten führt zum sicheren Fallback auf
    // das rekursive Einzel-Löschen (das löscht dann auch bei 207 die Reste).
    matches!(code, 200 | 201 | 202 | 204 | 404)
}

#[cfg(not(target_os = "macos"))]
fn webdav_collection_delete(
    _path: &Path,
    _mounts: &[(String, PathBuf)],
    _creds_cache: &mut HashMap<String, Option<(String, String)>>,
    _cancel: &Arc<AtomicBool>,
) -> bool {
    false
}

/// Löscht dauerhaft auf Netzlaufwerken mit einem abbrechbaren Job. Lokale
/// Löschungen verwenden weiterhin den Papierkorb und laufen nicht hierdurch.
#[tauri::command]
async fn run_network_delete(
    app: AppHandle,
    job_id: String,
    paths: Vec<String>,
    object_storage: Option<ObjectStorageDeleteRequest>,
    remote_storage: Option<RemoteStorageDeleteRequest>,
) -> Result<(), String> {
    if job_id.is_empty() || paths.is_empty() {
        return Err("Ungültiger Löschauftrag".into());
    }
    // Time-Machine-Backups liegen häufig auf per SMB/AFP eingebundenen NAS-
    // Freigaben und laufen dann über diesen Pfad. Ohne die Prüfung ließe sich
    // ein komplettes Backup hier endgültig und rekursiv löschen.
    let guard_paths = paths.clone();
    let protected = tauri::async_runtime::spawn_blocking(move || {
        let tm_mounts = tm_mountpoints_canon();
        guard_paths
            .iter()
            .map(|raw| expand_tilde(raw))
            .find(|path| is_time_machine_path(path, &tm_mounts))
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Some(path) = protected {
        return Err(format!("TIMEMACHINE_PROTECTED\u{1f}{}", path.display()));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).insert(job_id.clone(), cancel.clone());
    }
    let app_for_worker = app.clone();
    let job_id_for_worker = job_id.clone();
    let cancel_for_worker = cancel.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut ctx = DeleteCtx {
            app: &app_for_worker,
            job_id: &job_id_for_worker,
            cancel: &cancel_for_worker,
            done: 0,
            total: 0,
            last_emit: Cell::new(Instant::now()),
        };
        // Den Job unmittelbar sichtbar machen, auch wenn das erste WebDAV-
        // Listing mehrere Sekunden benötigt.
        let _ = ctx.app.emit(
            "job-progress",
            JobProgress {
                job_id: ctx.job_id.to_string(),
                done: 0,
                total: 0,
                files_done: 0,
                transfer_percent: None,
                indeterminate: false,
                current: String::new(),
                finished: false,
                cancelled: false,
                error: None,
            },
        );
        let object_paths: Vec<PathBuf> = paths.iter().map(|path| expand_tilde(path)).collect();
        // Die Löschzuordnung wird primär aus den aktiven Backend-Mounts
        // abgeleitet. Das verhindert, dass ein nach macOS-Pfadnormalisierung
        // nicht mehr exakt passender WebView-Wert in den generischen
        // Dateisystem-Löschpfad fällt.
        if let Some(context) = remote::object_storage_delete_context(&object_paths) {
            ctx.working_on(&context.mount_path);
            let directory_paths = object_storage
                .as_ref()
                .map(|request| {
                    request
                        .directory_paths
                        .iter()
                        .map(|path| expand_tilde(path))
                        .collect::<Vec<PathBuf>>()
                })
                .unwrap_or_default();
            remote::purge_object_storage(
                &context.profile,
                &context.mount_path,
                &object_paths,
                &directory_paths,
                &cancel_for_worker,
            )?;
            if !cancel_for_worker.load(Ordering::SeqCst) {
                remote::refresh_mount_after_direct_delete(&context.mount_path, &object_paths);
                ctx.removed_bulk(&context.mount_path, object_paths.len() as u64);
            }
            return Ok(());
        }
        if let Some(request) = object_storage {
            let mount_path = PathBuf::from(&request.mount_path);
            ctx.working_on(&mount_path);
            let directory_paths = request
                .directory_paths
                .iter()
                .map(|path| expand_tilde(path))
                .collect::<Vec<_>>();
            remote::purge_object_storage(
                &request.profile,
                &mount_path,
                &object_paths,
                &directory_paths,
                &cancel_for_worker,
            )?;
            if !cancel_for_worker.load(Ordering::SeqCst) {
                remote::refresh_mount_after_direct_delete(&mount_path, &object_paths);
                ctx.removed_bulk(&mount_path, object_paths.len() as u64);
            }
            return Ok(());
        }
        if let Some(request) = remote_storage {
            // SFTP ist ein SSHFS-Dateisystem. Ein zusätzlicher rclone-Purge
            // wäre ein zweiter, widersprüchlicher Zugriff auf dieselben
            // Dateien und ist deshalb selbst bei einer manipulierten IPC-
            // Anfrage ausgeschlossen.
            if request.spec.protocol == remote::RemoteProtocol::Sftp {
                return Err("SFTP-Löschvorgänge laufen über das eingehängte SSHFS-Dateisystem".into());
            }
            let mount_path = PathBuf::from(&request.mount_path);
            ctx.working_on(&mount_path);
            let remote_paths: Vec<PathBuf> = paths.iter().map(|path| expand_tilde(path)).collect();
            remote::purge_remote_storage(
                &request.spec,
                &mount_path,
                &remote_paths,
                &cancel_for_worker,
                None,
            )?;
            if !cancel_for_worker.load(Ordering::SeqCst) {
                remote::refresh_mount_after_direct_delete(&mount_path, &remote_paths);
                // Die FTP/FTPS-Wege liefern keine Einzelmeldungen; mindestens
                // der gewählte Eintrag zählt deshalb als abgeschlossen.
                ctx.removed_bulk(&mount_path, remote_paths.len() as u64);
            }
            return Ok(());
        }
        // Vorab die Gesamtzahl der Einträge ermitteln – wie die Vorschau beim
        // Kopieren. Damit zeigen Statusleiste, Fortschrittsbalken und Dock-
        // Badge einen echten Wert statt „?". Die Vorschau ist aber nur ein
        // Komfortgewinn: Sie darf den eigentlichen Löschvorgang nicht aufhalten
        // und wird deshalb übersprungen, sobald sie zu teuer wird.
        let mut planned: Vec<(PathBuf, u64)> = Vec::with_capacity(paths.len());
        let mut count_ok = true;
        // Zeitbudget für die gesamte Vorschau. Läuft es ab, wird ohne
        // Gesamtzahl gelöscht statt den Auftrag weiter aufzuhalten.
        let count_deadline = Instant::now() + Duration::from_secs(2);
        let mount_list = webdav_mounts();
        for raw in &paths {
            if cancel_for_worker.load(Ordering::SeqCst) {
                break;
            }
            let path = expand_tilde(raw);
            // WebDAV wird unabhängig vom Mount direkt auf dem Server gelöscht.
            // Die lokale Cache-Anzeige ist keine zuverlässige Quelle für eine
            // rekursive Zählung und darf den DELETE-Auftrag nicht verzögern.
            if best_webdav_mount(&mount_list, &path).is_some() {
                count_ok = false;
                planned.push((path, 0));
                continue;
            }
            match count_delete_entries(&path, &cancel_for_worker, count_deadline) {
                Ok(n) => planned.push((path, n)),
                // Lässt sich ein Pfad nicht zählen, bleibt die Gesamtzahl offen
                // (unbestimmter Balken) statt eine falsche Zahl anzuzeigen.
                Err(_) => {
                    count_ok = false;
                    planned.push((path, 0));
                }
            }
        }
        ctx.total = if count_ok {
            planned.iter().map(|(_, n)| *n).sum()
        } else {
            0
        };
        // Gesamtzahl sofort sichtbar machen (done bleibt 0).
        if ctx.total > 0 {
            let _ = ctx.app.emit(
                "job-progress",
                JobProgress {
                    job_id: ctx.job_id.to_string(),
                    done: 0,
                    total: ctx.total,
                    files_done: 0,
                    transfer_percent: None,
                    indeterminate: false,
                    current: String::new(),
                    finished: false,
                    cancelled: false,
                    error: None,
                },
            );
        }
        let mut webdav_creds: HashMap<String, Option<(String, String)>> = HashMap::new();
        // Elternordner, deren Listing der Mount nach einem serverseitigen
        // DELETE neu einlesen muss (siehe Aufräumen unterhalb der Schleife).
        let mut stale_parents: Vec<PathBuf> = Vec::new();
        let mut outcome: Result<(), String> = Ok(());
        for (path, count) in planned {
            if cancel_for_worker.load(Ordering::SeqCst) {
                break;
            }
            // Verbindlicher, vom Objekt-Speicher vollständig getrennter
            // WebDAV-Weg: genau ein serverseitiger DELETE für Datei oder
            // Collection. Kein rclone, kein webdavfs-Fallback und daher keine
            // Wiederholungen wegen dessen veraltetem Mount-Cache.
            if best_webdav_mount(&mount_list, &path).is_some() {
                ctx.working_on(&path);
                match webdav_delete_path(
                    &path,
                    &mount_list,
                    &mut webdav_creds,
                    &cancel_for_worker,
                ) {
                    Some(Ok(())) => {
                        ctx.removed_bulk(&path, count);
                        if let Some(parent) = path.parent() {
                            stale_parents.push(parent.to_path_buf());
                        }
                        continue;
                    }
                    Some(Err(error)) if cancel_for_worker.load(Ordering::SeqCst)
                        && error.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        break;
                    }
                    Some(Err(error)) => {
                        outcome = Err(delete_error_message(&path, &error));
                        break;
                    }
                    None => {
                        outcome = Err(format!(
                            "{}: WebDAV-Zugangsdaten konnten nicht aus dem macOS-Schlüsselbund gelesen werden. Bitte das Laufwerk neu verbinden.",
                            path.display()
                        ));
                        break;
                    }
                }
            }
            // Schnellpfad: Ordner auf einem WebDAV-Laufwerk werden mit einer
            // einzigen Collection-DELETE-Anfrage serverseitig rekursiv gelöscht.
            // Das ersetzt zehntausende Einzel-Requests über den Mount. Schlägt
            // der Versuch fehl (kein WebDAV-Mount, keine Zugangsdaten, Server-
            // fehler), greift der bewährte rekursive Weg darunter.
            let is_dir = std::fs::symlink_metadata(&path)
                .map(|m| m.is_dir() && !m.file_type().is_symlink())
                .unwrap_or(false);
            if is_dir {
                ctx.working_on(&path);
            }
            if is_dir
                && webdav_collection_delete(
                    &path,
                    &mount_list,
                    &mut webdav_creds,
                    &cancel_for_worker,
                )
            {
                ctx.removed_bulk(&path, count);
                if let Some(parent) = path.parent() {
                    stale_parents.push(parent.to_path_buf());
                }
                continue;
            }
            if cancel_for_worker.load(Ordering::SeqCst) {
                break;
            }
            if let Err(err) = remove_path_cancellable(&path, &mut ctx) {
                if cancel_for_worker.load(Ordering::SeqCst)
                    && err.kind() == std::io::ErrorKind::Interrupted
                {
                    break;
                }
                // Letzte Rettung, wenn der Mount den Ordner dauerhaft als
                // belegt meldet: Der Server kennt diese Sperre nicht. Nach dem
                // rekursiven Durchlauf ist nur noch ein Rest übrig, die
                // Anfrage also schnell.
                if is_dir
                    && is_retryable_remove_error(&err)
                    && webdav_collection_delete(
                        &path,
                        &mount_list,
                        &mut webdav_creds,
                        &cancel_for_worker,
                    )
                {
                    ctx.removed_bulk(&path, 1);
                    if let Some(parent) = path.parent() {
                        stale_parents.push(parent.to_path_buf());
                    }
                    continue;
                }
                outcome = Err(delete_error_message(&path, &err));
                break;
            }
        }
        // macOS' `webdavfs` merkt sich den Verzeichniseintrag eines
        // serverseitig gelöschten Ordners noch eine Weile. Er bliebe dann als
        // Geist in der Liste stehen und ein zweiter Löschversuch würde mit
        // „Resource busy" scheitern. Ein frisches Listing des Elternordners
        // räumt diesen Cache auf, bevor die Oberfläche neu einliest.
        stale_parents.sort();
        stale_parents.dedup();
        for parent in stale_parents {
            if let Ok(entries) = std::fs::read_dir(&parent) {
                for entry in entries {
                    let _ = entry;
                }
            }
        }
        outcome
    })
    .await
    .map_err(|err| err.to_string())?;
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).remove(&job_id);
    }
    let cancelled = cancel.load(Ordering::SeqCst);
    let error = result.as_ref().err().cloned();
    let _ = app.emit(
        "job-progress",
        JobProgress {
            job_id: job_id.clone(),
            done: 0,
            total: 0,
            files_done: 0,
            transfer_percent: None,
            indeterminate: false,
            current: String::new(),
            finished: true,
            cancelled,
            error,
        },
    );
    result
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

/// Prüft ohne Symlink-Auflösung, ob an `path` irgendein Eintrag existiert.
/// `Path::exists()` folgt Symlinks und meldet für hängende (dangling) Symlinks
/// fälschlich "nicht vorhanden" – für Lösch-/Konflikt-Entscheidungen zählt aber
/// der Verzeichniseintrag selbst.
fn path_occupied_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
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
    // `com.apple.provenance`) das Setzen mit EPERM. Manche Downloads enthalten
    // außerdem inzwischen nicht mehr vorhandene Attribute; `copyfile` meldet
    // dafür ENOATTR (macOS os error 93). In allen Fällen bleiben die
    // Nutzdaten kopierbar, nur die Metadaten müssen ausgelassen werden.
    // Wir degradieren deshalb schrittweise: erst ACL/xattr weglassen (Daten +
    // Zeitstempel/Rechte), zuletzt reine Datenkopie. Ein echter Lese- oder
    // Schreibfehler wird dabei nicht verschluckt: `std::fs::copy` schlägt dann
    // ebenfalls fehl und wird weitergereicht.
    let is_metadata_unsupported = |e: &std::io::Error| -> bool {
        matches!(
            e.raw_os_error(),
            Some(libc::ENOTSUP) | Some(libc::EOPNOTSUPP) | Some(libc::EPERM) | Some(libc::ENOATTR)
        )
    };

    // `copyfile(3)` meldet auf einigen WebDAV-Implementierungen Erfolg, obwohl
    // nur die leere Zieldatei angelegt wurde. Der Finder bemerkt das später
    // ebenfalls erst am Server. Deshalb ist ein erfolgreicher Rückgabewert
    // allein nicht ausreichend: Die Nutzdatenlänge wird stets geprüft und bei
    // Abweichung mit einem synchronen, regulären Schreibvorgang wiederholt.
    let copied_payload = || -> std::io::Result<bool> {
        Ok(std::fs::metadata(dst)?.len() == std::fs::metadata(src)?.len())
    };

    match call(COPYFILE_ALL) {
        Ok(()) if copied_payload()? => return Ok(()),
        Ok(()) => return copy_file_data_synchronously(src, dst),
        Err(e) if is_metadata_unsupported(&e) => {}
        Err(e) => return Err(e),
    }

    // Bei erneutem Versuch eine ggf. teilweise erzeugte Zieldatei entfernen,
    // damit copyfile frisch schreiben kann.
    let _ = std::fs::remove_file(dst);
    match call(COPYFILE_DATA | COPYFILE_STAT) {
        Ok(()) if copied_payload()? => return Ok(()),
        Ok(()) => return copy_file_data_synchronously(src, dst),
        Err(e) if is_metadata_unsupported(&e) => {}
        Err(e) => return Err(e),
    }

    // Letzter Fallback: reine, synchron bestätigte Datenkopie.
    let _ = std::fs::remove_file(dst);
    copy_file_data_synchronously(src, dst)
}

/// Schreibt Nutzdaten ohne den macOS-`copyfile`-Schnellpfad. Besonders
/// `webdavfs` benötigt diesen Weg: Erst `sync_all` bestätigt, dass der Upload
/// zum Server übergeben wurde. Die Größenprüfung verhindert, dass ein Server
/// eine nur angelegte 0-Byte-Datei als erfolgreiche Kopie erscheinen lässt.
fn copy_file_data_synchronously(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::io::{BufReader, BufWriter, Write};

    let expected_len = std::fs::metadata(src)?.len();
    let input = std::fs::File::open(src)?;
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, input);
    let mut writer = BufWriter::with_capacity(1024 * 1024, output);
    let written = std::io::copy(&mut reader, &mut writer)?;
    writer.flush()?;
    let output = writer.into_inner().map_err(|e| e.into_error())?;
    // macOS' `webdavfs` schreibt die Daten korrekt zum Server, unterstützt
    // aber kein fsync und meldet dafür ENOTTY (os error 25). Die unmittelbar
    // folgende Größenprüfung bestätigt trotzdem den vollständigen Upload.
    // Andere Sync-Fehler bleiben echte Fehler und werden nicht verschluckt.
    match output.sync_all() {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(libc::ENOTTY) => {}
        Err(error) => return Err(error),
    }
    let actual_len = output.metadata()?.len();
    if written != expected_len || actual_len != expected_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            format!(
                "unvollständiger Schreibvorgang: erwartet {expected_len} Byte, geschrieben {written} Byte, Ziel {actual_len} Byte"
            ),
        ));
    }
    Ok(())
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
/// Netzlaufwerke (WebDAV, SMB), die einzelne Operationen mit Timeout
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
/// fehlern (z. B. os error 60 „Operation timed out" auf WebDAV) mit
/// exponentiellem Backoff. Bricht sofort ab, wenn der Job abgebrochen wurde
/// oder ein nicht-transienter Fehler auftritt.
fn copy_file_retry(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    force_synchronous_data_copy: bool,
    webdav_target: Option<&(String, PathBuf)>,
) -> std::io::Result<()> {
    // `webdavfs` kann unter Last ENFILE („Too many open files in system“)
    // melden, obwohl der DualBeam-Prozess selbst kaum Deskriptoren hält. Ein
    // direkter WebDAV-PUT umgeht den Kernel-Mount und schreibt die lokale
    // Quelle mit genau einem curl-Prozess zum Server.
    //
    // Wichtig: Bei einem erkannten WebDAV-Ziel darf es keinen stillen
    // Dateisystem-Fallback geben. Der macOS-Mount kann eine angelegte
    // 0-Byte-Cachedatei als Erfolg melden. Können URL oder Schlüsselbunddaten
    // nicht ermittelt werden, wird deshalb bewusst ein verständlicher Fehler
    // zurückgegeben, statt dem Benutzer eine scheinbar gelungene Kopie zu
    // zeigen.
    if let Some((source_url, mountpoint)) = webdav_target {
        return webdav_upload_file(src, dst, source_url, mountpoint, cancel).unwrap_or_else(|| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "WebDAV-Zugangsdaten konnten nicht aus dem macOS-Schlüsselbund gelesen werden. Bitte das Laufwerk neu verbinden.",
            ))
        });
    }
    const MAX_ATTEMPTS: u32 = 5;
    let mut attempt: u32 = 0;
    loop {
        let result = if force_synchronous_data_copy {
            copy_file_data_synchronously(src, dst)
        } else {
            copy_file_with_metadata(src, dst)
        };
        match result {
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
fn replace_file_after_copy(
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    force_synchronous_data_copy: bool,
) -> std::io::Result<()> {
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
    let pid = std::process::id();
    // Eindeutigen, noch unbelegten Tempnamen wählen: PID + laufende Nummer
    // verhindern Kollisionen mit anderen Instanzen und mit Altlasten früherer
    // Läufe – copy_file_retry würde eine vorhandene Datei sonst überschreiben.
    let mut temp = None;
    for _ in 0..1000 {
        let id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.dualbeam-{pid}-{id}.inprogress"));
        if !path_occupied_no_follow(&candidate) {
            temp = Some(candidate);
            break;
        }
    }
    let temp = temp.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "keine freie temporäre Zieldatei gefunden",
        )
    })?;

    if let Err(error) = copy_file_retry(src, &temp, cancel, force_synchronous_data_copy, None) {
        // Eine fehlgeschlagene Kopie darf nicht in der nächsten Sync-Vorschau
        // als vermeintliche Nutzdatei auftauchen.
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
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

/// Stellt sicher, dass `dst` ein echtes Verzeichnis ist – ohne Symlinks zu folgen.
///
/// Zeigt am Zielpfad ein Symlink auf ein Verzeichnis, würden `create_dir_all`
/// und `read_dir` durch den Link hindurch arbeiten und Daten an einer ganz
/// anderen Stelle im Dateisystem ablegen. Deshalb wird hier ausschließlich mit
/// `symlink_metadata` geprüft.
///
/// Rückgabe `false` bedeutet: Ziel ist belegt und darf nicht ersetzt werden →
/// überspringen.
fn ensure_dir_no_follow(dst: &Path, overwrite: bool) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(dst) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => {
            if !overwrite {
                return Ok(false);
            }
            remove_path(dst)?;
            std::fs::create_dir_all(dst)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(dst)?;
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

/// Für ein aktives WebDAV-Ziel darf der rekursive Kopierer keinen schreibenden
/// Systemaufruf auf dem `/Volumes`-Mount ausführen. Sonst erzeugt webdavfs
/// zusätzliche AppleDouble- bzw. 0-Byte-Platzhalter neben den direkten HTTP-
/// Uploads. Fehlen die Keychain-Zugangsdaten ausnahmsweise, bleibt der normale
/// Dateisystem-Fallback verfügbar.
fn ensure_copy_target_directory(
    dst: &Path,
    overwrite: bool,
    ctx: &JobCtx<'_>,
) -> std::io::Result<bool> {
    if let Some((source_url, mountpoint)) = ctx.webdav_target.as_ref() {
        match webdav_create_collection(source_url, mountpoint, dst, ctx.cancel) {
            Some(result) => result.map(|_| true),
            None => ensure_dir_no_follow(dst, overwrite),
        }
    } else {
        ensure_dir_no_follow(dst, overwrite)
    }
}

/// Vor einem direkten WebDAV-PUT werden fehlende Elternordner direkt auf dem
/// Server angelegt. Ein lokales `create_dir_all` auf dem webdavfs-Mount wäre
/// nur ein Cache-Eintrag und ist genau die Ursache für HTTP-409-Fehler bei
/// Synchronisationen einzelner Dateien.
fn ensure_copy_target_file_parent(dst: &Path, ctx: &JobCtx<'_>) -> std::io::Result<()> {
    if let Some((source_url, mountpoint)) = ctx.webdav_target.as_ref() {
        return webdav_create_parent_collections(source_url, mountpoint, dst, ctx.cancel)
            .unwrap_or_else(|| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "WebDAV-Zugangsdaten konnten nicht aus dem macOS-Schlüsselbund gelesen werden. Bitte das Laufwerk neu verbinden.",
                ))
            });
    }
    let parent = dst.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "ungültiger Zielpfad")
    })?;
    std::fs::create_dir_all(parent)
}

/// Ein Verzeichnis-Scan und das spätere Öffnen einer Datei sind nicht atomar.
/// Besonders Finder, Screenshot-Tools und Browser-Downloads können eine Datei
/// dazwischen entfernen. Nur wenn die Quelle inzwischen wirklich nicht mehr
/// existiert, wird ENOENT als überspringbarer Eintrag behandelt; Fehler des
/// Zielpfads bleiben damit weiterhin sichtbar.
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
    // Ein früherer Fehler kann einen internen Undo-Puffer auf einem
    // Objekt-Speicher hinterlassen haben. Er gehört nie zu Nutzdaten und darf
    // weder kopiert noch beim Verschieben als fehlende Nutzdatei gewertet
    // werden.
    if src.file_name().and_then(|name| name.to_str()) == Some(".DualBeamUndo") {
        return Ok(CopyOutcome::Copied);
    }
    if src
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_dualbeam_inprogress_name)
    {
        return Ok(CopyOutcome::Skipped);
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
        if path_occupied_no_follow(dst) {
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
                                (|| {
                                    if !ensure_copy_target_directory(dst, overwrite, ctx)? {
                                        return Ok(CopyOutcome::Skipped);
                                    }
                                    let mut outcome = CopyOutcome::Copied;
                                    // `ReadDir` hält auf NFS-/rclone-Mounts einen
                                    // Datei-Handle offen. Erst komplett einlesen
                                    // und den Iterator schließen, bevor rekursiv
                                    // weiterkopiert wird – sonst bleibt pro Ebene
                                    // ein Handle offen und tiefe Swift-Bäume enden
                                    // mit EMFILE ("Too many open files").
                                    let entries = std::fs::read_dir(src)?
                                        .map(|entry| {
                                            entry.map(|entry| (entry.path(), entry.file_name()))
                                        })
                                        .collect::<Result<Vec<_>, std::io::Error>>()?;
                                    for (from, name) in entries {
                                        let to = dst.join(name);
                                        if copy_recursive(&from, &to, overwrite, ctx)?
                                            == CopyOutcome::Skipped
                                        {
                                            outcome = CopyOutcome::Skipped;
                                        }
                                    }
                                    Ok(outcome)
                                })()
                            } else {
                                // WebDAV und SSHFS erhalten keinen
                                // `copyfile`-Schnellpfad, sondern einen
                                // synchron bestätigten Datenstrom.
                                ensure_copy_target_file_parent(dst, ctx)?;
                                copy_file_retry(
                                    src,
                                    dst,
                                    ctx.cancel,
                                    ctx.force_synchronous_data_copy(),
                                    ctx.webdav_target.as_ref(),
                                )
                                .map(|_| {
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
        let destination_ready = ensure_copy_target_directory(dst, overwrite, ctx)?;
        if !destination_ready {
            return Ok(CopyOutcome::Skipped);
        }
        let mut outcome = CopyOutcome::Copied;
        // Siehe oben: Den Verzeichnis-Handle vor dem rekursiven Abstieg
        // schließen. Das ist besonders wichtig für große Objekt-Speicherbäume.
        let entries = std::fs::read_dir(src)?
            .map(|entry| entry.map(|entry| (entry.path(), entry.file_name())))
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        for (from, name) in entries {
            let to = dst.join(name);
            let child_outcome = copy_recursive(&from, &to, overwrite, ctx)
                .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", from.display())))?;
            if child_outcome == CopyOutcome::Skipped {
                outcome = CopyOutcome::Skipped;
            }
        }
        Ok(outcome)
    } else {
        let replacing = path_occupied_no_follow(dst);
        if replacing && !overwrite {
            return Ok(CopyOutcome::Skipped);
        }
        ensure_copy_target_file_parent(dst, ctx)?;
        if ctx.webdav_target.is_some() {
            copy_file_retry(
                src,
                dst,
                ctx.cancel,
                ctx.force_synchronous_data_copy(),
                ctx.webdav_target.as_ref(),
            )
        } else if replacing {
            replace_file_after_copy(src, dst, ctx.cancel, ctx.force_synchronous_data_copy())
        } else {
            copy_file_retry(
                src,
                dst,
                ctx.cancel,
                ctx.force_synchronous_data_copy(),
                None,
            )
        }?;
        ctx.files_done += 1;
        ctx.emit(&src.to_string_lossy());
        Ok(CopyOutcome::Copied)
    }
}

/// SFTP-Dateien werden nicht durch den SSHFS-Mount geschrieben. Der Mount
/// bleibt ausschließlich die Navigationsebene; der macOS-OpenSSH-Client
/// überträgt die Inhalte direkt per SFTP und umgeht damit FUSE-Dateihandles.
fn copy_to_sftp_mount_with_native_client(
    ctx: &mut JobCtx<'_>,
    src: &Path,
    dst: &Path,
    overwrite: bool,
) -> Result<CopyOutcome, String> {
    if src
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_dualbeam_inprogress_name)
    {
        return Ok(CopyOutcome::Skipped);
    }
    let _ = ctx.app.emit(
        "job-progress",
        JobProgress {
            job_id: ctx.job_id.to_string(),
            done: ctx.done,
            total: ctx.total,
            files_done: ctx.files_done,
            transfer_percent: None,
            indeterminate: true,
            current: src.to_string_lossy().into_owned(),
            finished: false,
            cancelled: false,
            error: None,
        },
    );
    let mut transfer_percent = 0;
    let current = src.to_string_lossy().into_owned();
    let cancel = ctx.cancel.clone();
    let mut report = |event: remote::SftpCopyProgress| {
        match event {
            remote::SftpCopyProgress::Percent(percent) => transfer_percent = percent,
            remote::SftpCopyProgress::FileCopied(path) => {
                ctx.files_done += 1;
                let _ = ctx.app.emit(
                    "job-progress",
                    JobProgress {
                        job_id: ctx.job_id.to_string(),
                        done: ctx.done,
                        total: ctx.total,
                        files_done: ctx.files_done,
                        transfer_percent: Some(transfer_percent),
                        indeterminate: false,
                        current: path,
                        finished: false,
                        cancelled: false,
                        error: None,
                    },
                );
                return;
            }
        }
        let _ = ctx.app.emit(
            "job-progress",
            JobProgress {
                job_id: ctx.job_id.to_string(),
                done: ctx.done,
                total: ctx.total,
                files_done: ctx.files_done,
                transfer_percent: Some(transfer_percent),
                indeterminate: false,
                current: current.clone(),
                finished: false,
                cancelled: false,
                error: None,
            },
        );
    };
    remote::upload_to_sftp_mount(src, dst, overwrite, &cancel, &mut report)?;
    Ok(CopyOutcome::Copied)
}

#[tauri::command]
async fn run_job(
    app: AppHandle,
    job_id: String,
    kind: String,
    items: Vec<JobItem>,
    object_storage: Option<ObjectStorageTransferRequest>,
    remote_storage: Option<serde_json::Value>,
) -> Result<(), String> {
    // SFTP läuft über SSHFS. Das Feld bleibt nur für ältere WebView-Aufrufe
    // kompatibel und wird bewusst nicht mehr als alternativer Transferweg
    // ausgewertet.
    let _ = remote_storage;
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
        // Nur einmal je Auftrag die Mount-Tabelle abfragen. Der Wert bleibt
        // während des Kopierens gültig, da DualBeam Netzlaufwerke erst beim
        // App-Ende aushängt.
        let mounted_webdav = webdav_mounts();
        let mut ctx = JobCtx {
            app: &app2,
            job_id: &job_id2,
            cancel: &cancel2,
            done: 0,
            total,
            files_done: 0,
            deref_depth: 0,
            target_is_webdav: false,
            webdav_target: None,
            last_emit: Cell::new(Instant::now()),
            last_reported_done: Cell::new(u64::MAX),
        };
        ctx.emit("");
        if kind == "copy" {
            // Die Oberfläche liefert die Objekt-Speicher-Zuordnung aus
            // Komfortgründen mit. Verbindlich ist jedoch die aktive
            // Backend-Registrierung: virtuelle S3-/Swift-Pfade können nach
            // Refresh, Alias oder Symlink anders formatiert sein und wurden
            // dadurch bislang gelegentlich als gewöhnliche leere Ordner
            // kopiert. Erkennen wir für alle Elemente genau denselben aktiven
            // Objekt-Speicher, hat diese Zuordnung Vorrang.
            let inferred_object_storage = if items.is_empty() {
                None
            } else {
                let contexts: Option<Vec<_>> = items
                    .iter()
                    .map(|item| {
                        let source = expand_tilde(&item.src);
                        let destination = expand_tilde(&item.dst);
                        remote::object_storage_transfer_context(&source, &destination)
                    })
                    .collect();
                contexts.and_then(|contexts| {
                    let first = contexts.first()?;
                    contexts
                        .iter()
                        .all(|context| {
                            context.profile.id == first.profile.id
                                && context.mount_path == first.mount_path
                                && context.source_is_object_storage
                                    == first.source_is_object_storage
                        })
                        .then(|| ObjectStorageTransferRequest {
                            profile: first.profile.clone(),
                            mount_path: first.mount_path.to_string_lossy().into_owned(),
                            source_is_object_storage: first.source_is_object_storage,
                        })
                })
            };
            let object_storage = inferred_object_storage.or(object_storage);
            if let Some(request) = object_storage {
                let ObjectStorageTransferRequest {
                    profile,
                    mount_path,
                    source_is_object_storage,
                } = request;
                let mount_path = PathBuf::from(mount_path);
                for it in &items {
                    if cancel2.load(Ordering::SeqCst) {
                        break;
                    }
                    let src = expand_tilde(&it.src);
                    let dst = expand_tilde(&it.dst);
                    // Ein Objekt-Speicher darf nicht mittels rclone in einen
                    // macOS-WebDAV-Mount schreiben. Der Mount-Treiber erzeugt
                    // dabei unter Last 0-Byte-Platzhalter bzw. EMFILE. Für
                    // genau diese Richtung materialisieren wir das Objekt
                    // kurz lokal und lassen den bewährten WebDAV-PUT den
                    // zweiten, vollständig getrennten Übertragungsabschnitt
                    // übernehmen.
                    let webdav_target = source_is_object_storage
                        .then(|| best_webdav_mount(&mounted_webdav, &dst))
                        .flatten();
                    let transfer = if let Some(webdav_target) = webdav_target {
                        ctx.target_is_webdav = true;
                        ctx.webdav_target = Some(webdav_target);
                        let staged = remote::materialize_object_storage_path(&src)
                            .ok_or_else(|| "Objekt-Speicherquelle ist nicht aktiv".to_string())?;
                        let transfer = match staged {
                            Ok(staged) => {
                                let result = copy_recursive(&staged, &dst, it.overwrite, &mut ctx)
                                    .map(|_| ())
                                    .map_err(|error| format!("{}: {error}", src.display()));
                                remote::cleanup_object_storage_materialization(&staged);
                                result
                            }
                            Err(error) => Err(error),
                        };
                        ctx.target_is_webdav = false;
                        ctx.webdav_target = None;
                        transfer
                    } else {
                        remote::copy_object_storage(
                            &profile,
                            &mount_path,
                            source_is_object_storage,
                            &src,
                            &dst,
                            it.overwrite,
                            &cancel2,
                        )
                    };
                    if let Err(error) = transfer {
                        remote::log_object_storage_operation(&format!(
                            "job failed after direct transfer request: {error}"
                        ));
                        return Err(error);
                    }
                    if !cancel2.load(Ordering::SeqCst) {
                        ctx.done += 1;
                        // Der lokale WebDAV-Zwischenschritt zählt seine
                        // tatsächlichen Dateien bereits in `copy_recursive`.
                        if !source_is_object_storage
                            || best_webdav_mount(&mounted_webdav, &dst).is_none()
                        {
                            ctx.files_done += 1;
                        }
                        ctx.emit(&it.src);
                    }
                }
                remote::log_object_storage_operation(
                    "run_job completed direct object-storage copy",
                );
                return Ok(());
            }
        }
        for it in &items {
            if cancel2.load(Ordering::SeqCst) {
                break;
            }
            let src = expand_tilde(&it.src);
            let dst = expand_tilde(&it.dst);
            ctx.webdav_target = best_webdav_mount(&mounted_webdav, &dst);
            ctx.target_is_webdav = ctx.webdav_target.is_some();
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
            if remote::sftp_mount_root(&dst).is_some() {
                let outcome =
                    copy_to_sftp_mount_with_native_client(&mut ctx, &src, &dst, it.overwrite)
                        .map_err(|error| format!("{}: {error}", src.display()))?;
                if is_move {
                    remove_source_after_move(&src, outcome)?;
                }
                ctx.done += 1;
                ctx.emit(&it.src);
                continue;
            }
            let mut handled = false;
            if is_move && !path_occupied_no_follow(&dst) {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if rename_no_clobber(&src, &dst).is_ok() {
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
            transfer_percent: None,
            indeterminate: false,
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

/// Toleranz für den mtime-Vergleich. Netzlaufwerke (WebDAV) und FAT
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

/// Von DualBeam selbst erzeugte, unvollständige Ersetzungsdatei. Die genaue
/// PID-/Sequenzform verhindert, dass gewöhnliche Nutzdateien mit der Endung
/// `.inprogress` versehentlich ausgeblendet werden.
pub(crate) fn is_dualbeam_inprogress_name(name: &str) -> bool {
    let Some(body) = name.strip_suffix(".inprogress") else {
        return false;
    };
    let parsed = body
        .strip_prefix('.')
        .and_then(|body| body.rsplit_once(".dualbeam-"))
        .or_else(|| body.rsplit_once(".dualbeam-sftp-"));
    let Some((original, ids)) = parsed else {
        return false;
    };
    let Some((pid, sequence)) = ids.split_once('-') else {
        return false;
    };
    !original.is_empty() && pid.parse::<u32>().is_ok() && sequence.parse::<u64>().is_ok()
}

/// Kurzlebige Ordner von DualBeam und Trunk enthalten keine Nutzdaten und
/// dürfen nicht in eine Synchronisation geraten. Insbesondere kann ein früher
/// abgebrochener Löschvorgang `.DualBeamUndo` auf einem Netzlaufwerk
/// hinterlassen. Alle anderen versteckten Dateien (auch `.git` und
/// `.trunk`-Konfigurationen) bleiben ausdrücklich Teil der Synchronisation.
fn is_transient_trunk_path(rel: &Path) -> bool {
    if rel.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(is_dualbeam_inprogress_name)
    }) {
        return true;
    }
    let mut components = rel.components();
    let Some(first) = components.next().and_then(|part| part.as_os_str().to_str()) else {
        return false;
    };
    if first == ".DualBeamUndo" {
        return true;
    }
    if first != ".trunk" {
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
        if check_sync_preview_cancelled().is_err() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Synchronisationsvorschau abgebrochen",
            ));
        }
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
    webdav_target: Option<&WebDavListingContext>,
    verify_checksums: bool,
    out: &mut Vec<SyncEntry>,
) -> Result<(), String> {
    check_sync_preview_cancelled()?;
    // Effektive Quell-Metadaten bestimmen: Symlinks folgen, um mit einem ggf.
    // dereferenzierten Ziel (Netzlaufwerk ohne Symlink-Support) zu vergleichen.
    let is_symlink = link_meta.file_type().is_symlink();
    let followed = if is_symlink {
        std::fs::metadata(src_path).ok()
    } else {
        Some(link_meta.clone())
    };

    // Bei einem WebDAV-Ziel ist der Server die maßgebliche Quelle. Der
    // webdavfs-Mount kann eine vorhandene Datei noch als „nicht vorhanden"
    // melden; dann würde die Vorschau sie fälschlich bei jedem Lauf erneut
    // kopieren. Reguläre Dateien werden deshalb vor jedem lokalen `stat`
    // direkt per PROPFIND geprüft.
    let server_lookup = match (followed.as_ref(), webdav_target) {
        (Some(source), Some(context)) if !source.is_dir() => Some(
            webdav_server_file_metadata_result(context, dst_path)
                .map_err(|error| format!("WebDAV-Ziel am Server prüfen fehlgeschlagen: {error}"))?,
        ),
        _ => None,
    };
    if let (Some(source), Some(server_metadata)) = (followed.as_ref(), server_lookup) {
        match server_metadata {
            Some(target) => {
                let mtime_differs = target
                    .modified
                    .map(|mtime| effective_src_mtime_secs(source) > mtime + MTIME_TOLERANCE_SECS)
                    .unwrap_or(false);
                let metadata_differs = source.len() != target.size || mtime_differs;
                let checksum_differs = verify_checksums
                    && !metadata_differs
                    && !files_match_sha256(src_path, dst_path);
                if metadata_differs || checksum_differs {
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "update".into(),
                        is_dir: false,
                        size: source.len(),
                    });
                }
            }
            None => out.push(SyncEntry {
                rel: rel_str,
                action: "copy".into(),
                is_dir: false,
                size: source.len(),
            }),
        }
        return Ok(());
    }

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
                // Dieser Zweig ist der Fallback für lokale Ziele bzw. eine
                // nicht erreichbare WebDAV-Serverabfrage.
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
/// Normalisiert die vom Frontend gelieferte Größengrenze. `None` und `0`
/// bedeuten beide „keine Grenze“ – so bleibt ein leeres bzw. abgeschaltetes
/// Eingabefeld wirkungslos, statt versehentlich alles auszuschließen.
fn normalize_max_file_size(value: Option<u64>) -> Option<u64> {
    value.filter(|v| *v > 0)
}

/// Prüft, ob ein Verzeichnis keinerlei Einträge enthält. Lesefehler gelten als
/// „nicht leer“, damit im Zweifel der bisherige Weg (Teilbaum als Einheit)
/// gewählt wird und nichts stillschweigend unter den Tisch fällt.
fn dir_is_empty(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut it) => it.next().is_none(),
        Err(_) => false,
    }
}

// Acht Parameter: Pfade, Zaehler und Abbruchsignal des Vorschaudurchlaufs.
#[allow(clippy::too_many_arguments)]
fn preview_walk_src(
    src_root: &Path,
    dst_root: &Path,
    cur: &Path,
    ignore_patterns: &[String],
    webdav_target: Option<&WebDavListingContext>,
    verify_checksums: bool,
    max_file_size: Option<u64>,
    out: &mut Vec<SyncEntry>,
) -> Result<(), String> {
    check_sync_preview_cancelled()?;
    let entries = read_dir_retry(cur).map_err(|e| format!("Quelle lesen fehlgeschlagen: {e}"))?;
    for entry in entries {
        check_sync_preview_cancelled()?;
        let p = entry.path();
        let rel = match p.strip_prefix(src_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if is_transient_trunk_path(rel) || is_ignored_sync_path(rel, ignore_patterns) {
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
            preview_compare_file(
                rel_str,
                &p,
                &dst_path,
                &link_meta,
                webdav_target,
                verify_checksums,
                out,
            )?;
            continue;
        }
        if link_meta.is_dir() {
            if let Some(context) = webdav_target {
                let target_exists =
                    webdav_server_path_exists(context, &dst_path).map_err(|error| {
                        format!("WebDAV-Ziel am Server prüfen fehlgeschlagen: {error}")
                    })?;
                if target_exists {
                    // Der Server kennt den Ordner, auch wenn der lokale
                    // webdavfs-Cache ihn noch nicht aufgelistet hat. Die
                    // Kinder werden ihrerseits direkt am Server verglichen.
                    preview_walk_src(
                        src_root,
                        dst_root,
                        &p,
                        ignore_patterns,
                        webdav_target,
                        verify_checksums,
                        max_file_size,
                        out,
                    )?;
                } else if is_trunk_root(rel) || (max_file_size.is_some() && !dir_is_empty(&p)) {
                    preview_walk_src(
                        src_root,
                        dst_root,
                        &p,
                        ignore_patterns,
                        webdav_target,
                        verify_checksums,
                        max_file_size,
                        out,
                    )?;
                } else {
                    out.push(SyncEntry {
                        rel: rel_str,
                        action: "copy".into(),
                        is_dir: true,
                        size: 0,
                    });
                }
                continue;
            }
            match symlink_metadata_retry(&dst_path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && is_trunk_root(rel) => {
                    preview_walk_src(
                        src_root,
                        dst_root,
                        &p,
                        ignore_patterns,
                        webdav_target,
                        verify_checksums,
                        max_file_size,
                        out,
                    )?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Ganzer Teilbaum ist neu → als Einheit melden, nicht rekursieren.
                    //
                    // Ausnahme bei aktiver Größengrenze: Ein Verzeichniseintrag wird
                    // vom Kopierjob rekursiv übertragen – übergroße Dateien darin
                    // kämen also trotz Grenze mit. Deshalb wird hier einzeln
                    // aufgelöst. Ein leeres Quellverzeichnis hat keine Dateien, die
                    // der Rekursion Einträge liefern könnten; es wird weiterhin als
                    // Einheit gemeldet, damit es am Ziel angelegt wird.
                    if max_file_size.is_some() && !dir_is_empty(&p) {
                        preview_walk_src(
                            src_root,
                            dst_root,
                            &p,
                            ignore_patterns,
                            webdav_target,
                            verify_checksums,
                            max_file_size,
                            out,
                        )?;
                    } else {
                        out.push(SyncEntry {
                            rel: rel_str,
                            action: "copy".into(),
                            is_dir: true,
                            size: 0,
                        });
                    }
                }
                Err(e) => return Err(format!("Ziel-Metadaten lesen fehlgeschlagen: {e}")),
                Ok(d) if d.is_dir() => preview_walk_src(
                    src_root,
                    dst_root,
                    &p,
                    ignore_patterns,
                    webdav_target,
                    verify_checksums,
                    max_file_size,
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
        // Reguläre Datei. Übergroße Dateien werden auf Wunsch ausgelassen: Bei
        // langsamen Zielen (WebDAV, SMB) blockiert eine einzelne Riesendatei den
        // gesamten Abgleich. Die Grenze wirkt nur auf reguläre Dateien – Symlinks
        // tragen keine sinnvolle Größe.
        if let Some(limit) = max_file_size {
            if link_meta.len() > limit {
                continue;
            }
        }
        preview_compare_file(
            rel_str,
            &p,
            &dst_path,
            &link_meta,
            webdav_target,
            verify_checksums,
            out,
        )?;
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
    check_sync_preview_cancelled()?;
    let entries = read_dir_retry(cur).map_err(|e| format!("Ziel lesen fehlgeschlagen: {e}"))?;
    for entry in entries {
        check_sync_preview_cancelled()?;
        let p = entry.path();
        let rel = match p.strip_prefix(dst_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if is_transient_trunk_path(rel) || is_ignored_sync_path(rel, ignore_patterns) {
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
        // Präsenz in der Quelle OHNE Symlink-Auflösung prüfen: ein hängender
        // Symlink in der Quelle ist trotzdem ein Eintrag – sein Ziel-Gegenstück
        // darf nicht fälschlich zum Löschen vorgeschlagen werden.
        let src_present = path_occupied_no_follow(&src_root.join(rel));
        if !src_present && is_dir && is_trunk_root(rel) {
            preview_walk_dst(src_root, dst_root, &p, ignore_patterns, out)?;
        } else if !src_present {
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

#[derive(Clone)]
struct DirectSyncPathInfo {
    is_dir: bool,
    size: u64,
    mtime: i64,
}

fn should_skip_direct_sync_path(rel: &Path, ignore_patterns: &[String]) -> bool {
    is_transient_trunk_path(rel)
        || is_ignored_sync_path(rel, ignore_patterns)
        || rel
            .components()
            .any(|component| component.as_os_str() == ".DualBeamUndo")
}

fn collect_filesystem_sync_tree(
    root: &Path,
    ignore_patterns: &[String],
) -> Result<HashMap<String, DirectSyncPathInfo>, String> {
    let mut entries = HashMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        check_sync_preview_cancelled()?;
        let entry = entry.map_err(|error| format!("Verzeichnis lesen fehlgeschlagen: {error}"))?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative,
            Err(_) => continue,
        };
        if should_skip_direct_sync_path(relative, ignore_patterns) {
            if entry.file_type().is_dir() {
                continue;
            }
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("Metadaten lesen fehlgeschlagen: {error}"))?;
        if is_untransferable_file(&metadata) {
            continue;
        }
        entries.insert(
            relative.to_string_lossy().into_owned(),
            DirectSyncPathInfo {
                is_dir: metadata.is_dir() && !metadata.file_type().is_symlink(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
                mtime: file_mtime_secs(&metadata),
            },
        );
    }
    Ok(entries)
}

fn collect_direct_sync_tree(
    root: &Path,
    ignore_patterns: &[String],
) -> Result<HashMap<String, DirectSyncPathInfo>, String> {
    if let Some(result) = remote::list_object_storage_tree(root) {
        let mut entries = HashMap::new();
        for entry in result? {
            check_sync_preview_cancelled()?;
            let relative = PathBuf::from(&entry.name);
            if relative.as_os_str().is_empty()
                || should_skip_direct_sync_path(&relative, ignore_patterns)
            {
                continue;
            }
            entries.insert(
                entry.name,
                DirectSyncPathInfo {
                    is_dir: entry.is_dir,
                    size: if entry.is_dir { 0 } else { entry.size },
                    mtime: entry.mtime,
                },
            );
        }
        return Ok(entries);
    }
    collect_filesystem_sync_tree(root, ignore_patterns)
}

/// Direkte Einweg-Sync-Vorschau, sobald mindestens eine Seite S3 oder Swift
/// ist. Die Objektseite wird ausschließlich mit `rclone lsjson` erfasst; kein
/// NFS-Mount und keine Dateiübertragung werden für die Vorschau benötigt.
fn direct_object_storage_sync_preview(
    src_root: &Path,
    dst_root: &Path,
    delete_extra: bool,
    ignore_patterns: &[String],
    max_file_size: Option<u64>,
) -> Result<Vec<SyncEntry>, String> {
    let source = collect_direct_sync_tree(src_root, ignore_patterns)?;
    let destination = collect_direct_sync_tree(dst_root, ignore_patterns)?;
    let mut out = Vec::new();
    let mut source_paths: Vec<_> = source.keys().cloned().collect();
    source_paths.sort_by_key(|path| (path.matches('/').count(), path.clone()));
    let mut copied_directories: Vec<String> = Vec::new();
    for relative in source_paths {
        check_sync_preview_cancelled()?;
        if copied_directories.iter().any(|directory| {
            relative.starts_with(directory)
                && relative.as_bytes().get(directory.len()) == Some(&b'/')
        }) {
            continue;
        }
        let source_info = &source[&relative];
        let target = destination.get(&relative);
        if source_info.is_dir {
            match target {
                None => {
                    // Ein fehlender Teilbaum wird als ein Kopierauftrag gezeigt.
                    // Bei einer Dateigröße-Grenze lösen wir ihn einzeln auf,
                    // damit große Dateien nicht versehentlich mitkommen.
                    if max_file_size.is_none() {
                        copied_directories.push(relative.clone());
                        out.push(SyncEntry {
                            rel: relative,
                            action: "copy".into(),
                            is_dir: true,
                            size: 0,
                        });
                    }
                }
                Some(target) if !target.is_dir => out.push(SyncEntry {
                    rel: relative,
                    action: "update".into(),
                    is_dir: true,
                    size: 0,
                }),
                Some(_) => {}
            }
            continue;
        }
        if max_file_size.is_some_and(|limit| source_info.size > limit) {
            continue;
        }
        let action = match target {
            None => Some("copy"),
            Some(target)
                if target.is_dir
                    || source_info.size != target.size
                    || source_info.mtime > target.mtime + MTIME_TOLERANCE_SECS =>
            {
                Some("update")
            }
            Some(_) => None,
        };
        if let Some(action) = action {
            out.push(SyncEntry {
                rel: relative,
                action: action.into(),
                is_dir: false,
                size: source_info.size,
            });
        }
    }
    if delete_extra {
        let mut destination_paths: Vec<_> = destination.keys().cloned().collect();
        destination_paths.sort_by_key(|path| (path.matches('/').count(), path.clone()));
        let mut deleted_directories: Vec<String> = Vec::new();
        for relative in destination_paths {
            check_sync_preview_cancelled()?;
            if source.contains_key(&relative)
                || deleted_directories.iter().any(|directory| {
                    relative.starts_with(directory)
                        && relative.as_bytes().get(directory.len()) == Some(&b'/')
                })
            {
                continue;
            }
            let info = &destination[&relative];
            if info.is_dir {
                deleted_directories.push(relative.clone());
            }
            out.push(SyncEntry {
                rel: relative,
                action: "delete".into(),
                is_dir: info.is_dir,
                size: info.size,
            });
        }
    }
    Ok(out)
}

/// Berechnet die Unterschiede zwischen `src` und `dst` (einweg: src → dst).
/// Vergleich über Größe + Änderungszeit (mit Toleranz). Symlinks werden
/// dereferenziert verglichen; transiente Netzwerkfehler werden wiederholt und
/// führen im Ernstfall zum Abbruch statt zu falschen Zahlen.
#[tauri::command]
// Acht Parameter: beide Seiten der Synchronisation samt Optionen und Fenstergriff.
#[allow(clippy::too_many_arguments)]
async fn sync_preview(
    app: AppHandle,
    preview_id: String,
    src: String,
    dst: String,
    delete_extra: bool,
    ignore_patterns: Vec<String>,
    verify_checksums: bool,
    max_file_size: Option<u64>,
) -> Result<Vec<SyncEntry>, String> {
    if preview_id.is_empty() {
        return Err("Ungültige Vorschaukennung".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).insert(preview_id.clone(), cancel.clone());
    }
    // Der Verzeichnis-Abgleich kann auf langsamen Netzlaufwerken (WebDAV,
    // SMB) sehr lange dauern. Als synchroner Befehl liefe er auf dem Haupt-Thread
    // und würde die gesamte Oberfläche einfrieren (macOS-Beachball) – der
    // Vorbereitungs-Hinweis im Dialog könnte gar nicht erst gezeichnet werden.
    // Deshalb wird die eigentliche Arbeit auf einem Blocking-Thread ausgeführt,
    // sodass die UI weiterhin reagiert und den Hinweis anzeigt.
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_cancellable_preview(cancel, || {
            sync_preview_inner(
                &src,
                &dst,
                delete_extra,
                ignore_patterns,
                verify_checksums,
                normalize_max_file_size(max_file_size),
            )
        })
    })
    .await;
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).remove(&preview_id);
    }
    result.map_err(|e| e.to_string())?
}

fn sync_preview_inner(
    src: &str,
    dst: &str,
    delete_extra: bool,
    extra_ignore_patterns: Vec<String>,
    verify_checksums: bool,
    max_file_size: Option<u64>,
) -> Result<Vec<SyncEntry>, String> {
    check_sync_preview_cancelled()?;
    let src_root = expand_tilde(src);
    let dst_root = expand_tilde(dst);
    if remote::is_object_storage_mount(&src_root) || remote::is_object_storage_mount(&dst_root) {
        let ignore_patterns = sync_ignore_patterns(&src_root, extra_ignore_patterns);
        return direct_object_storage_sync_preview(
            &src_root,
            &dst_root,
            delete_extra,
            &ignore_patterns,
            max_file_size,
        );
    }
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
    let webdav_target = webdav_listing_context(&dst_root);

    // Quelle durchlaufen → copy/update (robust gegen transiente Netzwerkfehler).
    preview_walk_src(
        &src_root,
        &dst_root,
        &src_root,
        &ignore_patterns,
        webdav_target.as_ref(),
        verify_checksums,
        max_file_size,
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
    app: AppHandle,
    preview_id: String,
    left: String,
    right: String,
    ignore_patterns: Vec<String>,
    verify_checksums: bool,
    max_file_size: Option<u64>,
) -> Result<Vec<SyncEntry>, String> {
    if preview_id.is_empty() {
        return Err("Ungültige Vorschaukennung".into());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).insert(preview_id.clone(), cancel.clone());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_cancellable_preview(cancel, || {
            sync_two_way_preview_inner(
                &left,
                &right,
                ignore_patterns,
                verify_checksums,
                normalize_max_file_size(max_file_size),
            )
        })
    })
    .await;
    {
        let mgr: State<JobManager> = app.state();
        lock_safe(&mgr.cancels).remove(&preview_id);
    }
    result.map_err(|e| e.to_string())?
}

fn newer_sync_side(left_root: &Path, right_root: &Path, rel: &str) -> Option<&'static str> {
    let mtime = |path: PathBuf| -> Option<(bool, i64)> {
        if let Some(result) = remote::object_storage_entry(&path) {
            let entry = result.ok()??;
            return Some((entry.is_dir, entry.mtime));
        }
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.is_dir(), file_mtime_secs(&meta)))
    };
    let (left_is_dir, left_mtime) = mtime(left_root.join(rel))?;
    let (right_is_dir, right_mtime) = mtime(right_root.join(rel))?;
    if left_is_dir || right_is_dir {
        return None;
    }
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
    max_file_size: Option<u64>,
) -> Result<Vec<SyncEntry>, String> {
    check_sync_preview_cancelled()?;
    let left_root = expand_tilde(left);
    let right_root = expand_tilde(right);
    let left_to_right = sync_preview_inner(
        left,
        right,
        false,
        ignore_patterns.clone(),
        verify_checksums,
        max_file_size,
    )?;
    let right_to_left = sync_preview_inner(
        right,
        left,
        false,
        ignore_patterns,
        verify_checksums,
        max_file_size,
    )?;
    check_sync_preview_cancelled()?;
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

/// Ordnerüberwachung einrichten. Im Worker-Thread, weil sowohl die Prüfung auf
/// ein Verzeichnis als auch `watch()` auf hängenden Netzlaufwerken blockieren.
#[tauri::command]
async fn watch_path(app: AppHandle, pane_id: String, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || watch_path_blocking(app, pane_id, path))
        .await
        .map_err(|e| e.to_string())?
}

fn watch_path_blocking(app: AppHandle, pane_id: String, path: String) -> Result<(), String> {
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

/// Rekursive Suche. Läuft im Worker-Thread, damit die Oberfläche während der
/// Baumdurchquerung bedienbar bleibt.
#[tauri::command]
async fn search_in_dir(
    root: String,
    query: String,
    show_hidden: bool,
    max_results: usize,
) -> Result<Vec<Entry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        search_in_dir_blocking(root, query, show_hidden, max_results)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn search_in_dir_blocking(
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
async fn preview_info(path: String) -> Result<PreviewInfo, String> {
    tauri::async_runtime::spawn_blocking(move || preview_info_blocking(path))
        .await
        .map_err(|e| e.to_string())?
}

fn preview_info_blocking(path: String) -> Result<PreviewInfo, String> {
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

/// Vorschaubild erzeugen. Im Worker-Thread, weil `qlmanage` als eigener Prozess
/// gestartet wird und synchron auf dessen Ende gewartet wird.
#[tauri::command]
async fn read_image_thumb(path: String, size: u32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || read_image_thumb_blocking(path, size))
        .await
        .map_err(|e| e.to_string())?
}

fn read_image_thumb_blocking(path: String, size: u32) -> Result<String, String> {
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
    /// Bei Netzordnern wäre eine rekursive Summierung eine unbeschränkte
    /// Folge von Serveranfragen. `None` bedeutet daher bewusst „nicht
    /// ermittelt", nicht „leer".
    size: Option<u64>,
    file_count: Option<u64>,
    dir_count: Option<u64>,
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

        let (size, file_count, dir_count) = if meta.is_dir() && remote::is_remote_mount(&p) {
            // SSHFS, WebDAV, FTP/FTPS und die virtuellen Objekt-Speicher
            // müssen für jede Unterebene den Server abfragen. Eigenschaften
            // bleiben deshalb sofort verfügbar und zeigen nur verlässliche
            // Metadaten der gewählten Ebene an.
            (None, None, None)
        } else if meta.is_dir() {
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
            (Some(s), Some(fc), Some(dc))
        } else {
            (Some(meta.len()), Some(0), Some(0))
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

    let update_item = MenuItemBuilder::new(s("Nach Updates suchen …", "Check for Updates…"))
        .id("check-updates")
        .build(app)?;

    let app_menu = SubmenuBuilder::new(app, "DualBeam")
        .item(&about_item)
        .separator()
        .item(&update_item)
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

// Startet die Anwendung neu. Wird nach einem eingespielten Update gebraucht:
// Der Updater tauscht das Programmverzeichnis aus, der laufende Prozess arbeitet
// aber weiter mit dem alten Stand im Speicher.
#[tauri::command]
fn restart_application(app: tauri::AppHandle) {
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(JobManager::default())
        .manage(WatcherManager::default())
        .setup(|app| {
            promise_drag::init(app.handle());
            // Reste eines abgestürzten früheren Laufs wegräumen: verwaiste
            // Einhängeordner und liegengebliebene Protokolle.
            remote::cleanup_stale();
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
                    if id == "check-updates" {
                        // Gezielt an das vordere Fenster. Ein Rundruf an alle
                        // Fenster oeffnete bei mehreren offenen Fenstern
                        // ebenso viele Update-Dialoge.
                        let focused = app_handle
                            .webview_windows()
                            .into_values()
                            .find(|w| w.is_focused().unwrap_or(false));
                        match focused {
                            Some(window) => {
                                let _ = window.emit("dualbeam://check-updates", ());
                            }
                            None => {
                                let _ = app_handle.emit("dualbeam://check-updates", ());
                            }
                        }
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
            restart_application,
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
            run_network_delete,
            stage_delete_for_undo,
            undo_staged_delete,
            finalize_staged_delete,
            cleanup_expired_undo,
            force_delete_admin,
            path_exists,
            navigation_root,
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
            remote::remote_host_keys,
            remote::remote_trust_host,
            remote::save_remote_password,
            remote::load_remote_password,
            remote::mount_remote,
            remote::unmount_remote,
            remote::remote_mounts,
            object_storage::save_object_storage_secret,
            object_storage::has_object_storage_secret,
            object_storage::forget_object_storage_secret,
            object_storage::import_remotedesk_object_storage_profiles,
            object_storage::mount_object_storage,
            promise_drag::start_promise_drag,
            promise_drag::resolve_promise_drop,
            promise_drag::list_open_with_apps,
            promise_drag::open_with_app,
            promise_drag::choose_application_dialog,
            promise_drag::set_default_application_for,
            rdp::rdp_available,
            rdp::rdp_profiles,
            rdp::rdp_connect,
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
            // Beim Beenden konsequent alle Netzwerk-Laufwerke lösen. Die
            // gespeicherten Lesezeichen und Schlüsselbund-Zugänge bleiben
            // dabei erhalten, lokale Laufwerke sind nicht betroffen.
            if matches!(_event, tauri::RunEvent::Exit) {
                unmount_all_network_volumes();
            }
        });
}

#[cfg(all(test, target_os = "macos"))]
mod copy_tests {
    use super::{
        bookmark_url_from_mount_source, copy_file_with_metadata, count_delete_entries,
        delete_error_message, destination_is_within_source, is_dualbeam_inprogress_name,
        is_network_fstype, is_protected_admin_root, is_retryable_remove_error,
        is_time_machine_path, is_transient_trunk_path, is_untransferable_file, mount_fs_types,
        normalize_max_file_size, parse_mount_url, percent_encode_segment, preview_walk_src,
        remove_source_after_move, replace_file_after_copy, search_in_dir_blocking,
        should_skip_direct_sync_path, statfs_fstype, sync_preview_inner,
        sync_two_way_preview_inner, webdav_host_from_url, webdav_http_date_epoch,
        webdav_propfind_content_length, webdav_propfind_last_modified, webdav_remote_url,
        zip_extract_inner, CopyOutcome,
    };
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixDatagram;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

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
    fn percent_encode_keeps_unreserved_and_escapes_rest() {
        assert_eq!(percent_encode_segment("abcXYZ-._~"), "abcXYZ-._~");
        assert_eq!(percent_encode_segment("a b"), "a%20b");
        assert_eq!(percent_encode_segment("f/o"), "f%2Fo");
        assert_eq!(percent_encode_segment("Ä"), "%C3%84");
        assert_eq!(percent_encode_segment("100%"), "100%25");
    }

    #[test]
    fn ordinary_inprogress_file_is_not_a_time_machine_backup() {
        assert!(!is_time_machine_path(
            Path::new("/tmp/upload.dualbeam-40316-0.inprogress"),
            &[]
        ));
        assert!(is_dualbeam_inprogress_name(
            ".upload.zip.dualbeam-40316-0.inprogress"
        ));
        assert!(is_dualbeam_inprogress_name(
            "upload.zip.dualbeam-sftp-40316-0.inprogress"
        ));
        assert!(is_transient_trunk_path(Path::new(
            "nested/.upload.zip.dualbeam-40316-0.inprogress"
        )));
        assert!(!is_dualbeam_inprogress_name("upload.inprogress"));
        assert!(!is_dualbeam_inprogress_name(
            ".upload.zip.dualbeam-user-copy.inprogress"
        ));
        assert!(is_time_machine_path(
            Path::new("/Volumes/Backup/Backups.backupdb/Mac/Latest"),
            &[]
        ));
    }

    #[test]
    fn webdav_url_translates_directory_with_trailing_slash() {
        let url = webdav_remote_url(
            "https://webdav.example.com/",
            std::path::Path::new("/Volumes/webdav.example.com"),
            std::path::Path::new("/Volumes/webdav.example.com/Fotos/Neuer Ordner"),
            true,
        )
        .unwrap();
        assert_eq!(url, "https://webdav.example.com/Fotos/Neuer%20Ordner/");
    }

    #[test]
    fn webdav_url_translates_file_without_trailing_slash() {
        let url = webdav_remote_url(
            "https://webdav.example.com",
            std::path::Path::new("/Volumes/dav"),
            std::path::Path::new("/Volumes/dav/a/b.txt"),
            false,
        )
        .unwrap();
        assert_eq!(url, "https://webdav.example.com/a/b.txt");
    }

    #[test]
    fn webdav_propfind_reads_file_length() {
        let response = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:getcontentlength>4871923</d:getcontentlength></d:prop></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(webdav_propfind_content_length(response), Some(4_871_923));
    }

    #[test]
    fn webdav_propfind_reads_server_modified_time() {
        let response = r#"<d:prop xmlns:d="DAV:"><d:getlastmodified>Thu, 01 Jan 1970 00:00:00 GMT</d:getlastmodified></d:prop>"#;
        assert_eq!(webdav_propfind_last_modified(response), Some(0));
        assert_eq!(
            webdav_http_date_epoch("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(1_445_412_480)
        );
    }

    #[test]
    fn webdav_url_rejects_mount_root() {
        // Ohne relatives Segment darf keine URL entstehen (Schutz vor dem
        // versehentlichen Löschen des gesamten Laufwerks).
        assert!(webdav_remote_url(
            "https://webdav.example.com/",
            std::path::Path::new("/Volumes/dav"),
            std::path::Path::new("/Volumes/dav"),
            true,
        )
        .is_none());
    }

    #[test]
    fn webdav_host_is_extracted() {
        assert_eq!(
            webdav_host_from_url("https://webdav.example.com/remote.php").as_deref(),
            Some("webdav.example.com")
        );
        assert_eq!(
            webdav_host_from_url("https://user@host.example:8443/x").as_deref(),
            Some("host.example")
        );
        assert_eq!(webdav_host_from_url("not-a-url").as_deref(), None);
    }

    #[test]
    fn count_delete_entries_gives_up_after_deadline() {
        // Auf langsamen Netzlaufwerken darf die Vorschau den Loeschvorgang
        // nicht aufhalten: Ist das Zeitbudget aufgebraucht, bricht sie ab.
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let root = tmp_path("deadline-tree");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/f.txt"), b"1").unwrap();
        let expired = Instant::now() - Duration::from_secs(1);
        let err = count_delete_entries(&root, &cancel, expired).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        // Der Baum bleibt vom Zaehlen unberuehrt.
        assert!(root.join("a/f.txt").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn busy_errors_are_retryable_and_get_own_message() {
        let busy = std::io::Error::from_raw_os_error(libc::EBUSY);
        let not_empty = std::io::Error::from_raw_os_error(libc::ENOTEMPTY);
        let denied = std::io::Error::from_raw_os_error(libc::EACCES);
        assert!(is_retryable_remove_error(&busy));
        assert!(is_retryable_remove_error(&not_empty));
        assert!(!is_retryable_remove_error(&denied));

        // Belegte Eintraege bekommen eine Kennung, die die Oberflaeche
        // uebersetzt - statt des unverstaendlichen "Resource busy".
        let msg = delete_error_message(std::path::Path::new("/Volumes/dav/Ordner"), &busy);
        assert_eq!(msg, "NETWORK_BUSY\u{1f}/Volumes/dav/Ordner");
        // Alle anderen Fehler bleiben im bisherigen Format.
        let other = delete_error_message(std::path::Path::new("/Volumes/dav/x"), &denied);
        assert!(other.starts_with("/Volumes/dav/x: "));
        assert!(!other.contains("NETWORK_BUSY"));
    }

    #[test]
    fn count_delete_entries_counts_every_node() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_secs(30);
        // Einzelne Datei => 1
        let file = tmp_path("solo.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(count_delete_entries(&file, &cancel, deadline).unwrap(), 1);

        // Ordnerbaum: root + a + a/b + a/b/f.txt + c.txt = 5 Knoten
        let root = tmp_path("tree");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), b"1").unwrap();
        std::fs::write(root.join("c.txt"), b"2").unwrap();
        assert_eq!(count_delete_entries(&root, &cancel, deadline).unwrap(), 5);

        // Nicht existierender Pfad => 0
        let gone = tmp_path("nope");
        std::fs::remove_dir_all(gone.parent().unwrap()).ok();
        assert_eq!(count_delete_entries(&gone, &cancel, deadline).unwrap(), 0);
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

        replace_file_after_copy(&src, &dst, &AtomicBool::new(false), false)
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
        preview_walk_src(&src, &dst, &src, &[], None, false, None, &mut entries).unwrap();
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
        std::fs::write(src.join(".DS_Store"), b"finder metadata").unwrap();
        std::fs::write(src.join("._Urlaub.pdf"), b"apple double").unwrap();
        std::fs::write(src.join(".trunk").join("trunk.yaml"), b"keep config").unwrap();
        std::fs::write(src.join(".trunk").join("logs").join("active"), b"ephemeral").unwrap();
        std::fs::write(src.join("upload.inprogress"), b"user content").unwrap();
        std::fs::write(
            src.join(".upload.zip.dualbeam-40316-0.inprogress"),
            b"partial internal copy",
        )
        .unwrap();

        let entries = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec![],
            false,
            None,
        )
        .unwrap();
        assert!(entries.iter().any(|entry| entry.rel == ".hidden"));
        assert!(entries.iter().any(|entry| entry.rel == ".DS_Store"));
        assert!(entries.iter().any(|entry| entry.rel == "._Urlaub.pdf"));
        assert!(entries.iter().any(|entry| entry.rel == ".trunk/trunk.yaml"));
        assert!(entries.iter().any(|entry| entry.rel == "upload.inprogress"));
        assert!(!entries
            .iter()
            .any(|entry| entry.rel == ".upload.zip.dualbeam-40316-0.inprogress"));
        assert!(!entries
            .iter()
            .any(|entry| entry.rel.starts_with(".trunk/logs")));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn direct_object_storage_sync_includes_hidden_files() {
        assert!(!should_skip_direct_sync_path(Path::new(".env"), &[]));
        assert!(!should_skip_direct_sync_path(Path::new(".DS_Store"), &[]));
        assert!(!should_skip_direct_sync_path(
            Path::new("._Urlaub.pdf"),
            &[]
        ));
        assert!(should_skip_direct_sync_path(
            Path::new(".DualBeamUndo/backup"),
            &[]
        ));
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
            None,
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
            None,
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
            None,
        )
        .unwrap();
        assert!(without_checksums.is_empty());

        let with_checksums = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            false,
            vec![],
            true,
            None,
        )
        .unwrap();
        assert!(with_checksums
            .iter()
            .any(|entry| entry.rel == "same-size.txt"));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn skips_files_above_the_size_limit() {
        let root = tmp_path("size-limit-root");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("small.txt"), vec![b'a'; 10]).unwrap();
        std::fs::write(src.join("huge.bin"), vec![b'b'; 5000]).unwrap();

        let entries = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec![],
            false,
            Some(1000),
        )
        .unwrap();
        assert!(entries.iter().any(|entry| entry.rel == "small.txt"));
        assert!(!entries.iter().any(|entry| entry.rel == "huge.bin"));

        // Ohne Grenze muss dieselbe Datei wieder auftauchen – sonst wäre die
        // Filterung nicht an die Einstellung gebunden.
        let all = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec![],
            false,
            None,
        )
        .unwrap();
        assert!(all.iter().any(|entry| entry.rel == "huge.bin"));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn size_limit_resolves_new_subtrees_file_by_file() {
        // Ein neuer Unterbaum wird sonst als ein einziger Verzeichniseintrag
        // gemeldet und vom Kopierjob rekursiv übertragen – die Grenze bliebe
        // wirkungslos. Mit Grenze muss er einzeln aufgelöst werden.
        let root = tmp_path("size-limit-subtree");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("neu")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("neu").join("klein.txt"), vec![b'a'; 10]).unwrap();
        std::fs::write(src.join("neu").join("gross.bin"), vec![b'b'; 5000]).unwrap();

        let entries = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec![],
            false,
            Some(1000),
        )
        .unwrap();
        assert!(entries.iter().any(|entry| entry.rel == "neu/klein.txt"));
        assert!(!entries.iter().any(|entry| entry.rel == "neu/gross.bin"));
        // Kein gebündelter Verzeichniseintrag, der die große Datei mitnähme.
        assert!(!entries
            .iter()
            .any(|entry| entry.rel == "neu" && entry.is_dir));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn size_limit_still_creates_empty_directories() {
        let root = tmp_path("size-limit-empty-dir");
        let src = root.join("src");
        let dst = root.join("dst");
        std::fs::create_dir_all(src.join("leer")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        let entries = sync_preview_inner(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            true,
            vec![],
            false,
            Some(1000),
        )
        .unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.rel == "leer" && entry.is_dir));

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn treats_zero_size_limit_as_no_limit() {
        // Ein leeres bzw. abgeschaltetes Eingabefeld darf nicht dazu führen,
        // dass jede Datei als „zu groß" gilt.
        assert_eq!(normalize_max_file_size(None), None);
        assert_eq!(normalize_max_file_size(Some(0)), None);
        assert_eq!(normalize_max_file_size(Some(42)), Some(42));
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
            None,
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

        let results = search_in_dir_blocking(
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

    // Belegt die Gleichwertigkeit der Kernel-Abfrage mit der bisherigen Quelle:
    // Für jeden eingehängten Mountpoint muss `statfs` denselben Dateisystemtyp
    // melden wie die Textausgabe von `/sbin/mount`. Gilt das für alle lokal
    // vorhandenen Typen, gilt es auch für Netzwerktypen wie webdav, die auf
    // einem Testrechner selten eingehängt sind.
    #[test]
    fn statfs_matches_mount_table() {
        let table = mount_fs_types();
        assert!(!table.is_empty(), "Mount-Tabelle unerwartet leer");

        let mut checked = 0usize;
        for (mountpoint, expected) in &table {
            let Some(measured) = statfs_fstype(Path::new(mountpoint)) else {
                continue;
            };
            assert_eq!(
                measured,
                expected.to_ascii_lowercase(),
                "Abweichung bei {mountpoint}: statfs meldet {measured}, /sbin/mount meldet {expected}"
            );
            checked += 1;
        }
        assert!(checked > 0, "kein Mountpoint konnte geprüft werden");
    }

    // Ein Löschziel kann bereits verschwunden sein. Dann muss der Typ des
    // nächstgelegenen vorhandenen Elternverzeichnisses gelten – sonst fiele der
    // Netzwerkschutz genau dann aus, wenn er gebraucht wird.
    #[test]
    fn statfs_falls_back_to_parent_directory() {
        let root = statfs_fstype(Path::new("/")).expect("Wurzel muss einen Typ liefern");
        let missing = statfs_fstype(Path::new("/dualbeam-gibt-es-nicht/auch-nicht"))
            .expect("Rückfall auf die Wurzel erwartet");
        assert_eq!(root, missing);
    }

    // statfs liefert kleingeschrieben; is_network_fstype vergleicht exakt.
    #[test]
    fn network_fstypes_are_recognised() {
        for fstype in ["webdav", "smbfs", "nfs", "afpfs", "ftp", "cifs"] {
            assert!(
                is_network_fstype(fstype),
                "{fstype} muss als Netzwerk gelten"
            );
        }
        for fstype in ["apfs", "hfs", "exfat", "msdos"] {
            assert!(!is_network_fstype(fstype), "{fstype} ist kein Netzwerk");
        }
    }
}

/// Prüfungen für die Sammelabfrage von Dateiwerten am WebDAV-Server.
///
/// Grundlage ist eine echte Antwort von pCloud: gemischte Namensraumpräfixe
/// (`D:` für die Struktur, `lp1:`/`lp2:` für die Eigenschaften), Attribute im
/// `response`-Tag und ein selbstschließendes `resourcetype` bei Dateien. Ein
/// selbst erdachtes XML hätte genau die Eigenheiten verfehlt, die den Parser
/// schwierig machen – der erste Auswerteversuch scheiterte an eben diesen
/// Attributen.
#[cfg(all(test, target_os = "macos"))]
mod webdav_listing_tests {
    use super::{
        percent_decode_segment, webdav_href_file_name, webdav_propfind_entries, xml_unescape,
    };

    /// Nachbildung einer pCloud-Antwort auf `PROPFIND /Screenshots/` mit
    /// `Depth: 1`: das Verzeichnis selbst, zwei Dateien und ein Unterordner.
    const PCLOUD_LISTING: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
<D:response xmlns:lp1="DAV:" xmlns:lp2="http://apache.org/dav/props/">
<D:href>/Screenshots/</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype><D:collection/></lp1:resourcetype>
<lp1:creationdate>2026-09-02T10:29:51Z</lp1:creationdate>
<lp1:getlastmodified>Wed, 02 Sep 2026 10:29:51 GMT</lp1:getlastmodified>
<D:getcontenttype>httpd/unix-directory</D:getcontenttype>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
<D:response xmlns:lp1="DAV:" xmlns:lp2="http://apache.org/dav/props/">
<D:href>/Screenshots/Hardware-Test-Icon.png</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype/>
<lp1:creationdate>2026-09-02T10:24:53Z</lp1:creationdate>
<lp1:getcontentlength>6397</lp1:getcontentlength>
<lp1:getlastmodified>Wed, 02 Sep 2026 10:24:53 GMT</lp1:getlastmodified>
<lp2:executable>F</lp2:executable>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
<D:response xmlns:lp1="DAV:">
<D:href>/Screenshots/Bild%20mit%20Leerzeichen%20%26%20Zeichen.png</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype/>
<lp1:creationdate>2026-09-02T11:00:00Z</lp1:creationdate>
<lp1:getcontentlength>396345</lp1:getcontentlength>
<lp1:getlastmodified>Wed, 02 Sep 2026 11:00:00 GMT</lp1:getlastmodified>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
<D:response xmlns:lp1="DAV:">
<D:href>/Screenshots/Unterordner/</D:href>
<D:propstat>
<D:prop>
<lp1:resourcetype><D:collection/></lp1:resourcetype>
<lp1:getlastmodified>Wed, 02 Sep 2026 09:00:00 GMT</lp1:getlastmodified>
</D:prop>
<D:status>HTTP/1.1 200 OK</D:status>
</D:propstat>
</D:response>
</D:multistatus>"#;

    #[test]
    fn liest_alle_dateien_eines_verzeichnisses() {
        let entries = webdav_propfind_entries(PCLOUD_LISTING);
        // Nur die beiden Dateien: das Verzeichnis selbst und der Unterordner
        // sind keine Einträge, für die Größe oder Datum ergänzt werden müssten.
        assert_eq!(entries.len(), 2, "Ordner dürfen nicht enthalten sein");
        assert!(!entries.contains_key("Screenshots"));
        assert!(!entries.contains_key("Unterordner"));

        let icon = entries
            .get("Hardware-Test-Icon.png")
            .expect("Datei fehlt in der Auswertung");
        assert_eq!(icon.size, 6397);
        assert!(icon.modified.is_some(), "Änderungsdatum fehlt");
        assert!(icon.created.is_some(), "Erstellungsdatum fehlt");
    }

    #[test]
    fn loest_prozentkodierung_im_dateinamen_auf() {
        let entries = webdav_propfind_entries(PCLOUD_LISTING);
        // Der Server liefert den Namen kodiert; verglichen wird später mit dem
        // Klarnamen aus dem Verzeichniseintrag.
        let key = "Bild mit Leerzeichen & Zeichen.png";
        assert!(
            entries.contains_key(key),
            "erwarteter Schlüssel {key:?} fehlt, vorhanden: {:?}",
            entries.keys().collect::<Vec<_>>()
        );
        assert_eq!(entries[key].size, 396345);
    }

    #[test]
    fn ordnet_jedem_eintrag_die_eigenen_werte_zu() {
        // Der eigentliche Grund für die Zerlegung in Blöcke: würde man im
        // gesamten Text nach dem ersten Treffer suchen, bekäme jede Datei die
        // Werte des zuerst genannten Eintrags.
        let entries = webdav_propfind_entries(PCLOUD_LISTING);
        let a = entries["Hardware-Test-Icon.png"].size;
        let b = entries["Bild mit Leerzeichen & Zeichen.png"].size;
        assert_ne!(a, b, "beide Dateien haben dieselbe Größe bekommen");
    }

    #[test]
    fn liefert_nichts_bei_unbrauchbarer_antwort() {
        // Abgeschnittene oder fremde Antworten dürfen nicht in einen Absturz
        // münden; die Anzeige behält dann schlicht die Werte des Systems.
        for body in ["", "<html>Fehler</html>", "<D:multistatus><D:response>"] {
            assert!(
                webdav_propfind_entries(body).is_empty(),
                "unerwartete Einträge bei {body:?}"
            );
        }
    }

    #[test]
    fn erkennt_ordner_auch_mit_gemeldeter_laenge() {
        // Manche Server melden für Ordner zusätzlich eine Länge. Ohne die
        // Auswertung von `collection` liefe ein Ordner dann als Datei mit und
        // bekäme beim Auffüllen falsche Werte zugewiesen.
        let xml = r#"<D:multistatus xmlns:D="DAV:">
<D:response>
<D:href>/pfad/Ordner/</D:href>
<D:propstat><D:prop>
<D:resourcetype><D:collection/></D:resourcetype>
<D:getcontentlength>0</D:getcontentlength>
<D:getlastmodified>Wed, 02 Sep 2026 09:00:00 GMT</D:getlastmodified>
</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
</D:response>
</D:multistatus>"#;
        let entries = webdav_propfind_entries(xml);
        assert!(
            entries.is_empty(),
            "Ordner wurde als Datei geführt: {:?}",
            entries.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn liest_werte_trotz_zusaetzlichem_404_abschnitt() {
        // Server antworten häufig mit zwei `propstat`-Abschnitten: einem mit
        // den vorhandenen Eigenschaften (200) und einem mit den nicht
        // unterstützten (404). Die Auswertung darf sich davon nicht beirren
        // lassen.
        let xml = r#"<D:multistatus xmlns:D="DAV:">
<D:response>
<D:href>/pfad/Datei.bin</D:href>
<D:propstat><D:prop>
<D:resourcetype/>
<D:getcontentlength>12345</D:getcontentlength>
<D:getlastmodified>Wed, 02 Sep 2026 09:00:00 GMT</D:getlastmodified>
</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
<D:propstat><D:prop>
<D:creationdate/>
<D:getcontentlanguage/>
</D:prop><D:status>HTTP/1.1 404 Not Found</D:status></D:propstat>
</D:response>
</D:multistatus>"#;
        let entries = webdav_propfind_entries(xml);
        let file = entries.get("Datei.bin").expect("Datei fehlt");
        assert_eq!(file.size, 12345);
        assert!(file.modified.is_some(), "Änderungsdatum fehlt");
        // Das Erstellungsdatum war leer – es darf schlicht fehlen, ohne dass
        // der ganze Eintrag verworfen wird.
        assert!(file.created.is_none());
    }

    #[test]
    fn loest_xml_entitaeten_auf() {
        assert_eq!(xml_unescape("a &amp; b"), "a & b");
        assert_eq!(xml_unescape("&lt;tag&gt;"), "<tag>");
        assert_eq!(xml_unescape("&quot;x&apos;y&quot;"), "\"x'y\"");
        assert_eq!(xml_unescape("&#38;"), "&");
        assert_eq!(xml_unescape("&#x26;"), "&");
        // Unbekanntes bleibt unverändert – ein merkwürdiger Name ist besser
        // als ein fehlender Eintrag.
        assert_eq!(xml_unescape("&unbekannt;"), "&unbekannt;");
        assert_eq!(xml_unescape("100% & mehr"), "100% & mehr");
    }

    #[test]
    fn dekodiert_prozentsequenzen() {
        assert_eq!(percent_decode_segment("Bild%20mit%20Leer"), "Bild mit Leer");
        assert_eq!(percent_decode_segment("Gr%C3%BC%C3%9Fe"), "Grüße");
        assert_eq!(percent_decode_segment("ohne"), "ohne");
        // Ungültige oder unvollständige Sequenzen bleiben stehen, statt den
        // Namen zu verwerfen.
        assert_eq!(percent_decode_segment("100%"), "100%");
        assert_eq!(percent_decode_segment("50%zz"), "50%zz");
        assert_eq!(percent_decode_segment("a%2"), "a%2");
    }

    /// Prüft den vollständigen Weg gegen einen tatsächlich eingehängten
    /// WebDAV-Server: Kontext beschaffen, URL bilden, `PROPFIND` absetzen,
    /// Antwort auswerten.
    ///
    /// Standardmäßig übersprungen, weil dafür ein Mount und Zugangsdaten im
    /// Schlüsselbund nötig sind. Aufruf:
    /// `cargo test --lib gegen_echten_server -- --ignored --nocapture`
    #[test]
    #[ignore = "benötigt einen eingehängten WebDAV-Server"]
    fn gegen_echten_server() {
        use super::{webdav_listing_context, webdav_server_directory_entries};
        use std::path::Path;

        let mount = std::env::var("DUALBEAM_WEBDAV_MOUNT")
            .unwrap_or_else(|_| "/Volumes/ewebdav.pcloud.com".to_string());
        let directory = Path::new(&mount);
        assert!(directory.is_dir(), "{mount} ist nicht eingehängt");

        let context = webdav_listing_context(directory).expect("kein WebDAV-Kontext");

        // Ein Verzeichnis mit Dateien suchen: die Wurzel eines Mounts enthält
        // oft ausschließlich Ordner, dort wäre nichts zu vergleichen.
        let mut ziel = directory.to_path_buf();
        let mut entries = webdav_server_directory_entries(&context, &ziel)
            .expect("keine Serverantwort für die Wurzel");
        if entries.is_empty() {
            let unterordner = std::fs::read_dir(directory)
                .expect("Wurzel nicht lesbar")
                .flatten()
                .filter(|e| e.path().is_dir())
                .find_map(|e| {
                    let pfad = e.path();
                    let gefunden = webdav_server_directory_entries(&context, &pfad)
                        .filter(|treffer| !treffer.is_empty())?;
                    Some((pfad, gefunden))
                });
            let (pfad, gefunden) = unterordner.expect("kein Verzeichnis mit Dateien gefunden");
            ziel = pfad;
            entries = gefunden;
        }
        println!("geprüftes Verzeichnis: {}", ziel.display());
        let directory = ziel.as_path();

        // Gegenprobe mit dem Dateisystem: Was der Server meldet, muss zu dem
        // passen, was der Treiber im selben Verzeichnis zeigt.
        let mut geprueft = 0usize;
        for eintrag in std::fs::read_dir(directory)
            .expect("Verzeichnis nicht lesbar")
            .flatten()
        {
            let name = eintrag.file_name().to_string_lossy().into_owned();
            let Some(server) = entries.get(&name) else {
                continue;
            };
            if let Ok(meta) = eintrag.metadata() {
                if meta.is_file() && meta.len() > 0 {
                    assert_eq!(meta.len(), server.size, "Größe weicht ab bei {name}");
                    geprueft += 1;
                }
            }
        }
        println!(
            "{} Servereinträge, {geprueft} gegen das Dateisystem geprüft",
            entries.len()
        );
        assert!(geprueft > 0, "keine Datei konnte gegengeprüft werden");
    }

    /// Belegt den Kern des Entwurfs: Sind die Werte des Treibers vollständig,
    /// darf die Anzeige keinen einzigen zusätzlichen Serverzugriff kosten.
    ///
    /// Aufruf:
    /// `cargo test --lib normalfall_ohne_zusatzabfrage -- --ignored --nocapture`
    #[test]
    #[ignore = "benötigt einen eingehängten WebDAV-Server"]
    fn normalfall_ohne_zusatzabfrage() {
        use super::list_dir_blocking;
        use std::path::Path;

        let mount = std::env::var("DUALBEAM_WEBDAV_MOUNT")
            .unwrap_or_else(|_| "/Volumes/ewebdav.pcloud.com".to_string());
        let verzeichnis = Path::new(&mount).join("Screenshots");
        if !verzeichnis.is_dir() {
            println!("{} fehlt – übersprungen", verzeichnis.display());
            return;
        }

        let start = std::time::Instant::now();
        let eintraege =
            list_dir_blocking(verzeichnis.to_string_lossy().into_owned(), true).expect("Listing");
        let dauer = start.elapsed();

        let dateien: Vec<_> = eintraege
            .iter()
            .filter(|e| !e.is_dir && !e.is_symlink)
            .collect();
        assert!(!dateien.is_empty(), "keine Dateien im Prüfverzeichnis");
        for eintrag in &dateien {
            assert!(
                eintrag.mtime > 0,
                "Änderungsdatum fehlt bei {}",
                eintrag.name
            );
            assert!(
                !eintrag.mode_str.is_empty(),
                "Rechte fehlen bei {}",
                eintrag.name
            );
        }
        println!("{} Dateien in {dauer:?}", dateien.len());
        // Ein Roundtrip zu pCloud liegt bei mehreren hundert Millisekunden.
        // Bleibt die Anzeige deutlich darunter, hat keine Serverabfrage
        // stattgefunden.
        assert!(
            dauer < std::time::Duration::from_millis(150),
            "Anzeige dauerte {dauer:?} – vermutlich lief eine unnötige Serverabfrage"
        );
    }

    /// Der eigentliche Zweck der Änderung: fehlende Werte werden aus der
    /// Serverauskunft wiederhergestellt.
    ///
    /// Der Störfall lässt sich nicht auf Zuruf herbeiführen – er entsteht durch
    /// einen Zwischenspeicher, den der Treiber selbst verwaltet. Deshalb wird
    /// hier ein echtes Listing genommen und künstlich um genau die Werte
    /// gebracht, die `webdavfs` in jenem Fall verschluckt.
    ///
    /// Aufruf:
    /// `cargo test --lib stellt_fehlende_werte_wieder_her -- --ignored --nocapture`
    #[test]
    #[ignore = "benötigt einen eingehängten WebDAV-Server"]
    fn stellt_fehlende_werte_wieder_her() {
        use super::{list_dir_blocking, repair_webdav_entries};
        use std::path::Path;

        let mount = std::env::var("DUALBEAM_WEBDAV_MOUNT")
            .unwrap_or_else(|_| "/Volumes/ewebdav.pcloud.com".to_string());
        let verzeichnis = Path::new(&mount).join("Screenshots");
        if !verzeichnis.is_dir() {
            println!("{} fehlt – übersprungen", verzeichnis.display());
            return;
        }

        // Zwei getrennte Abfragen statt einer Kopie: `Entry` soll nicht allein
        // für diese Prüfung klonbar werden müssen.
        let pfad = verzeichnis.to_string_lossy().into_owned();
        let echt = list_dir_blocking(pfad.clone(), true).expect("Listing");
        let mut beschaedigt = list_dir_blocking(pfad, true).expect("Listing");
        for eintrag in beschaedigt.iter_mut() {
            if !eintrag.is_dir && !eintrag.is_symlink {
                eintrag.size = 0;
                eintrag.mtime = 0;
                eintrag.birth_time = 0;
                eintrag.owner.clear();
                eintrag.group.clear();
                eintrag.mode_str.clear();
            }
        }

        repair_webdav_entries(&mut beschaedigt, &verzeichnis);

        let mut wiederhergestellt = 0usize;
        for (vorher, nachher) in echt.iter().zip(beschaedigt.iter()) {
            if vorher.is_dir || vorher.is_symlink || vorher.size == 0 {
                continue;
            }
            assert_eq!(
                nachher.size, vorher.size,
                "Größe nicht wiederhergestellt bei {}",
                vorher.name
            );
            assert!(
                nachher.mtime > 0,
                "Datum fehlt weiterhin bei {}",
                vorher.name
            );
            assert_eq!(
                nachher.mode_str, vorher.mode_str,
                "Rechte weichen ab bei {}",
                vorher.name
            );
            assert_eq!(
                nachher.owner, vorher.owner,
                "Eigentümer weicht ab bei {}",
                vorher.name
            );
            wiederhergestellt += 1;
        }
        println!("{wiederhergestellt} Einträge wiederhergestellt");
        assert!(wiederhergestellt > 0, "nichts zu prüfen");
    }

    #[test]
    fn holt_den_dateinamen_aus_dem_href() {
        assert_eq!(
            webdav_href_file_name("/Screenshots/Bild.png").as_deref(),
            Some("Bild.png")
        );
        // Vollständige URL statt Pfad – beides ist nach RFC 4918 zulässig.
        assert_eq!(
            webdav_href_file_name("https://example.org/pfad/Datei%20A.txt").as_deref(),
            Some("Datei A.txt")
        );
        // Ein Ordner endet auf einen Schrägstrich.
        assert_eq!(
            webdav_href_file_name("/Screenshots/Unterordner/").as_deref(),
            Some("Unterordner")
        );
        assert_eq!(webdav_href_file_name("/"), None);
        assert_eq!(webdav_href_file_name(""), None);
    }
}
