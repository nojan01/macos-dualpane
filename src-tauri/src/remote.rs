//! Netzlaufwerke über rclone: SFTP sowie FTP mit und ohne TLS.
//!
//! macOS bringt für SFTP und FTPS kein eigenes Dateisystem mit. Diese Lücke
//! schließt das mitgelieferte rclone: Es hängt das Ziel über den NFS-Client von
//! macOS ein. Dadurch wird weder eine Kernel-Erweiterung noch ein
//! Administratorkennwort gebraucht, und das Laufwerk verhält sich danach wie
//! ein ganz gewöhnlicher Ordner. Alle übrigen Befehle der App brauchen deshalb
//! keinerlei Sonderbehandlung für Netzlaufwerke.
//!
//! Zugangsdaten erreichen rclone ausschließlich über Umgebungsvariablen. Auf
//! der Kommandozeile stünden sie in der Prozessliste und wären damit für jedes
//! andere Programm des Benutzers lesbar.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Schlüsselbund-Dienst für die Kennwörter der Netzlaufwerke. Bewusst getrennt
/// von dem der rsync-Synchronisation, damit beide unabhängig voneinander
/// widerrufen werden können.
const KEYCHAIN_SERVICE: &str = "com.nojan.dualbeam.remote";

/// Name, unter dem das Ziel innerhalb von rclone geführt wird. Er taucht nur in
/// den Umgebungsvariablen auf und ist für den Benutzer nie sichtbar.
const RCLONE_REMOTE: &str = "DUALBEAM";

/// Wie lange nach dem Start von rclone auf das fertige Laufwerk gewartet wird.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(45);

/// Zeitlimit für das Abfragen der Hostschlüssel. `ssh-keyscan` wartet sonst bei
/// einem stillen Ziel sehr lange.
const KEYSCAN_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteProtocol {
    Sftp,
    /// FTP ohne jede Verschlüsselung.
    Ftp,
    /// FTP, das die Verbindung nach dem Verbindungsaufbau auf TLS hebt
    /// (AUTH TLS, üblicherweise Port 21). Das ist die verbreitete Variante.
    FtpsExplicit,
    /// FTP, das von der ersten Sekunde an TLS spricht (üblicherweise Port 990).
    FtpsImplicit,
}

impl RemoteProtocol {
    fn default_port(self) -> u16 {
        match self {
            Self::Sftp => 22,
            Self::Ftp | Self::FtpsExplicit => 21,
            Self::FtpsImplicit => 990,
        }
    }

    /// Für rclone ist FTPS kein eigener Typ, sondern FTP mit gesetzter
    /// TLS-Option.
    fn rclone_type(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Ftp | Self::FtpsExplicit | Self::FtpsImplicit => "ftp",
        }
    }

    /// Nur unverschlüsseltes FTP überträgt Kennwort und Inhalte im Klartext.
    pub fn is_encrypted(self) -> bool {
        !matches!(self, Self::Ftp)
    }

    fn scheme(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::FtpsExplicit | Self::FtpsImplicit => "ftps",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSpec {
    pub protocol: RemoteProtocol,
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub username: String,
    /// Pfad auf dem Server. Leer bedeutet die Wurzel des Zugangs.
    #[serde(default)]
    pub path: String,
    /// Anzeigename des Laufwerks. Leer bedeutet: aus dem Host ableiten.
    #[serde(default)]
    pub label: String,
}

impl RemoteSpec {
    fn port_or_default(&self) -> u16 {
        self.port.unwrap_or_else(|| self.protocol.default_port())
    }

    /// Kennzeichnung für den Schlüsselbund und die Anzeige. Enthält nie das
    /// Kennwort.
    fn descriptor(&self) -> String {
        format!(
            "{}://{}@{}:{}",
            self.protocol.scheme(),
            self.username,
            self.host,
            self.port_or_default()
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyReport {
    pub host: String,
    pub port: u16,
    /// Fingerabdrücke in der Form „ED25519 SHA256:…“, zur Anzeige im Dialog.
    pub fingerprints: Vec<String>,
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMountInfo {
    pub path: String,
    pub label: String,
    pub descriptor: String,
}

struct ActiveMount {
    path: PathBuf,
    label: String,
    descriptor: String,
    child: Child,
    log: PathBuf,
}

fn registry() -> &'static Mutex<Vec<ActiveMount>> {
    static REGISTRY: OnceLock<Mutex<Vec<ActiveMount>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// Prüfungen
// ---------------------------------------------------------------------------

/// Hostname oder IP-Adresse. Bewusst eng gefasst: Der Wert landet in einer
/// Umgebungsvariablen und in einem `ssh-keyscan`-Aufruf.
pub fn valid_host(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    // Eine IPv6-Adresse darf zusätzlich Doppelpunkte enthalten.
    let bare = value.trim_start_matches('[').trim_end_matches(']');
    if bare.contains(':') {
        return bare.parse::<IpAddr>().is_ok();
    }
    value
        .split('.')
        .all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        && !value.starts_with('.')
        && !value.ends_with('.')
}

/// Benutzername. Leerzeichen und Steuerzeichen bleiben draußen, sonst ist der
/// Zeichenvorrat großzügig — Anmeldenamen in der Form `konto@domain` sind bei
/// gehosteten Servern üblich.
pub fn valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+' | '$'))
}

/// Pfad auf dem Server. Steuerzeichen würden die Ausgabe von rclone verfälschen,
/// `..` könnte aus dem freigegebenen Bereich hinausführen.
pub fn valid_remote_path(value: &str) -> bool {
    value.len() <= 1024
        && !value.chars().any(char::is_control)
        && !value.split('/').any(|part| part == "..")
}

/// Macht aus einem Wunschnamen einen Ordnernamen, der gefahrlos als
/// Einhängepunkt taugt. Gibt `None`, wenn nichts Brauchbares übrig bleibt.
pub fn sanitize_label(value: &str) -> Option<String> {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Zu lange Namen werden gekürzt statt abgelehnt: Ein Hostname darf ruhig
    // länger sein, als ein Ordnername sinnvoll ist. Nach dem Kürzen noch einmal
    // säubern, damit am Ende kein Leerzeichen oder Punkt stehen bleibt.
    let short: String = cleaned.trim().chars().take(60).collect();
    let trimmed = short.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn validate(spec: &RemoteSpec, allow_insecure: bool) -> Result<(), String> {
    if !valid_host(&spec.host) {
        return Err("err.remote.host".into());
    }
    if !valid_username(&spec.username) {
        return Err("err.remote.username".into());
    }
    if !valid_remote_path(&spec.path) {
        return Err("err.remote.path".into());
    }
    if let Some(port) = spec.port {
        if port == 0 {
            return Err("err.remote.port".into());
        }
    }
    // Unverschlüsseltes FTP gibt Kennwort und Inhalte im Klartext preis. Es
    // bleibt deshalb — wie bei den übrigen offenen Protokollen der App — auf
    // direkt adressierte Rechner im eigenen Netz beschränkt.
    if !spec.protocol.is_encrypted() {
        if !allow_insecure {
            return Err("err.network.insecureConfirm".into());
        }
        let bare = spec.host.trim_start_matches('[').trim_end_matches(']');
        let ip = bare
            .parse::<IpAddr>()
            .map_err(|_| "err.network.localIpOnly".to_string())?;
        if !crate::is_local_network_address(ip) {
            return Err("err.network.localIpOnly".into());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Ablageorte
// ---------------------------------------------------------------------------

fn app_dir() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir().ok_or_else(|| "err.remote.noAppDir".to_string())?;
    let dir = base.join("DualBeam").join("Remote");
    std::fs::create_dir_all(&dir).map_err(|_| "err.remote.noAppDir".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// Übergeordneter Ordner aller eingehängten Netzlaufwerke.
///
/// `/Volumes` scheidet aus: Dort darf ohne Administratorrechte kein Ordner
/// angelegt werden.
pub fn mount_root() -> Result<PathBuf, String> {
    let dir = app_dir()?.join("Volumes");
    std::fs::create_dir_all(&dir).map_err(|_| "err.remote.noAppDir".to_string())?;
    Ok(dir)
}

/// Eigene Liste bekannter Hostschlüssel.
///
/// `~/.ssh/known_hosts` wird bewusst nicht angefasst: Diese Datei gehört dem
/// Benutzer, und ein Dateimanager hat darin nichts zu schreiben.
fn known_hosts_file() -> Result<PathBuf, String> {
    let path = app_dir()?.join("known_hosts");
    if !path.exists() {
        std::fs::write(&path, b"").map_err(|_| "err.remote.noAppDir".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(path)
}

/// Pfad zum mitgelieferten rclone.
///
/// Im fertigen Programmpaket legt Tauri die Datei neben das Hauptprogramm. Beim
/// Entwickeln liegt sie noch unter `src-tauri/binaries` mit angehängtem
/// Zielkürzel.
fn rclone_executable() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| "err.remote.rcloneMissing".to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "err.remote.rcloneMissing".to_string())?;
    let triple = if cfg!(target_arch = "x86_64") {
        "x86_64-apple-darwin"
    } else {
        "aarch64-apple-darwin"
    };
    let candidates = [
        dir.join("rclone"),
        dir.join(format!("rclone-{triple}")),
        // Beim Entwickeln liegt das Programm zwei Ebenen über target/debug.
        dir.join("../../binaries").join(format!("rclone-{triple}")),
    ];
    candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| path.to_path_buf())
        .ok_or_else(|| "err.remote.rcloneMissing".into())
}

// ---------------------------------------------------------------------------
// Hostschlüssel
// ---------------------------------------------------------------------------

fn run_with_timeout(mut child: Child, limit: Duration) -> Result<std::process::Output, String> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|_| "err.remote.keyscan".to_string())
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("err.remote.keyscan".into());
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("err.remote.keyscanTimeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Holt sämtliche Hostschlüssel des Servers.
///
/// Es müssen wirklich alle Typen sein: rclone handelt nicht zwingend denselben
/// Typ aus wie das `ssh` der Kommandozeile. Steht nur ein Teil in der Liste,
/// scheitert die Verbindung mit „key mismatch“, obwohl der Server unverändert
/// ist.
fn scan_host_keys(host: &str, port: u16) -> Result<Vec<String>, String> {
    let child = Command::new("/usr/bin/ssh-keyscan")
        .args(["-p", &port.to_string(), "-T", "10", "--", host])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "err.remote.keyscan".to_string())?;
    let out = run_with_timeout(child, KEYSCAN_TIMEOUT)?;
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    if lines.is_empty() {
        return Err("err.remote.noHostKey".into());
    }
    Ok(lines)
}

/// Rechnet die Rohschlüssel in lesbare Fingerabdrücke um, damit der Benutzer
/// sie mit der Angabe seines Anbieters vergleichen kann.
fn fingerprints(lines: &[String]) -> Vec<String> {
    let mut child = match Command::new("/usr/bin/ssh-keygen")
        .args(["-l", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let out = match run_with_timeout(child, Duration::from_secs(10)) {
        Ok(out) => out,
        Err(_) => return Vec::new(),
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            // Format: "<bits> <SHA256:…> <host> (<TYP>)"
            let mut parts = line.split_whitespace();
            let _bits = parts.next()?;
            let digest = parts.next()?;
            let kind = line
                .rsplit('(')
                .next()
                .map(|rest| rest.trim_end_matches(')'))
                .unwrap_or("")
                .to_string();
            Some(format!("{kind} {digest}"))
        })
        .collect()
}

fn host_pattern(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn is_trusted(host: &str, port: u16) -> Result<bool, String> {
    let file = known_hosts_file()?;
    let out = Command::new("/usr/bin/ssh-keygen")
        .args(["-F", &host_pattern(host, port), "-f"])
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "err.remote.keyscan".to_string())?;
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

/// Fragt die Hostschlüssel ab und meldet, ob dieser Server bereits bekannt ist.
#[tauri::command]
pub async fn remote_host_keys(host: String, port: Option<u16>) -> Result<HostKeyReport, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<HostKeyReport, String> {
        if !valid_host(&host) {
            return Err("err.remote.host".into());
        }
        let port = port.unwrap_or(22);
        if port == 0 {
            return Err("err.remote.port".into());
        }
        let trusted = is_trusted(&host, port)?;
        let lines = scan_host_keys(&host, port)?;
        Ok(HostKeyReport {
            host,
            port,
            fingerprints: fingerprints(&lines),
            trusted,
        })
    })
    .await
    .map_err(|_| "err.remote.keyscan".to_string())?
}

/// Übernimmt die Schlüssel des Servers in die Liste der bekannten Hosts.
#[tauri::command]
pub async fn remote_trust_host(host: String, port: Option<u16>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        if !valid_host(&host) {
            return Err("err.remote.host".into());
        }
        let port = port.unwrap_or(22);
        let lines = scan_host_keys(&host, port)?;
        let file = known_hosts_file()?;
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        // Ein früherer Eintrag desselben Servers muss weichen, sonst stünden
        // widersprüchliche Schlüssel nebeneinander und die Verbindung schlüge
        // mit „key mismatch“ fehl.
        let pattern = host_pattern(&host, port);
        let mut kept: Vec<&str> = existing
            .lines()
            .filter(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return false;
                }
                line.split_whitespace()
                    .next()
                    .map(|hosts| !hosts.split(',').any(|entry| entry == pattern))
                    .unwrap_or(true)
            })
            .collect();
        for line in &lines {
            kept.push(line);
        }
        let mut body = kept.join("\n");
        body.push('\n');
        std::fs::write(&file, body).map_err(|_| "err.remote.noAppDir".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    })
    .await
    .map_err(|_| "err.remote.keyscan".to_string())?
}

// ---------------------------------------------------------------------------
// Schlüsselbund
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn save_remote_password(spec: RemoteSpec, password: String) -> Result<(), String> {
    if password.is_empty() {
        return Err("err.remote.emptyPassword".into());
    }
    if !valid_host(&spec.host) || !valid_username(&spec.username) {
        return Err("err.remote.host".into());
    }
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            &spec.descriptor(),
            password.as_bytes(),
        )
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = spec;
        Err("err.remote.keychainUnavailable".into())
    }
}

#[tauri::command]
pub fn load_remote_password(spec: RemoteSpec) -> Result<Option<String>, String> {
    if !valid_host(&spec.host) || !valid_username(&spec.username) {
        return Err("err.remote.host".into());
    }
    #[cfg(target_os = "macos")]
    {
        match security_framework::passwords::get_generic_password(
            KEYCHAIN_SERVICE,
            &spec.descriptor(),
        ) {
            Ok(raw) => String::from_utf8(raw)
                .map(Some)
                .map_err(|_| "err.remote.badKeychainEntry".to_string()),
            // Kein Eintrag ist im Dialog kein Fehler, sondern der Normalfall
            // beim ersten Verbinden.
            Err(_) => Ok(None),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = spec;
        Err("err.remote.keychainUnavailable".into())
    }
}

// ---------------------------------------------------------------------------
// Einhängen
// ---------------------------------------------------------------------------

/// Verschleiert das Kennwort so, wie rclone es erwartet.
///
/// Die Übergabe läuft über die Standardeingabe. Als Aufrufargument stünde das
/// Kennwort in der Prozessliste und wäre für jedes andere Programm sichtbar.
fn obscure(rclone: &Path, password: &str) -> Result<String, String> {
    let mut child = Command::new(rclone)
        .args(["obscure", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "err.remote.rcloneMissing".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(password.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("err.remote.obscure".into());
        }
    }
    let out = run_with_timeout(child, Duration::from_secs(15))?;
    if !out.status.success() {
        return Err("err.remote.obscure".into());
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() {
        return Err("err.remote.obscure".into());
    }
    Ok(value)
}

/// Ist dieser Pfad gerade ein Einhängepunkt?
///
/// Der Vergleich der Geräte-IDs mit dem übergeordneten Ordner ist zuverlässiger
/// als das Durchsuchen der Ausgabe von `mount`: Er kommt ohne Textzerlegung aus
/// und funktioniert auch bei Namen mit Leerzeichen.
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let here = match std::fs::metadata(path) {
        Ok(meta) => meta.dev(),
        Err(_) => return false,
    };
    match path.parent().and_then(|p| std::fs::metadata(p).ok()) {
        Some(parent) => parent.dev() != here,
        None => false,
    }
}

fn unique_mount_dir(label: &str) -> Result<(PathBuf, String), String> {
    let root = mount_root()?;
    for attempt in 0..50 {
        let name = if attempt == 0 {
            label.to_string()
        } else {
            format!("{label} {}", attempt + 1)
        };
        let candidate = root.join(&name);
        if is_mount_point(&candidate) {
            continue;
        }
        // Ein leerer Ordner aus einem früheren Lauf darf weiterverwendet
        // werden; ein gefüllter bleibt unangetastet.
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok((candidate, name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let empty = std::fs::read_dir(&candidate)
                    .map(|mut rd| rd.next().is_none())
                    .unwrap_or(false);
                if empty {
                    return Ok((candidate, name));
                }
            }
            Err(_) => return Err("err.remote.mountDir".into()),
        }
    }
    Err("err.remote.mountDir".into())
}

fn env_key(option: &str) -> String {
    format!("RCLONE_CONFIG_{RCLONE_REMOTE}_{}", option.to_uppercase())
}

/// Baut die Umgebung, mit der rclone das Ziel kennt. Sichtbar wird davon nichts:
/// Umgebungsvariablen eines fremden Prozesses kann ein anderer Benutzer nicht
/// auslesen, die Kommandozeile dagegen schon.
fn rclone_env(
    spec: &RemoteSpec,
    obscured: &str,
    known_hosts: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = vec![
        (env_key("type"), spec.protocol.rclone_type().to_string()),
        (env_key("host"), spec.host.clone()),
        (env_key("user"), spec.username.clone()),
        (env_key("pass"), obscured.to_string()),
        (env_key("port"), spec.port_or_default().to_string()),
    ];
    match spec.protocol {
        RemoteProtocol::Sftp => {
            if let Some(file) = known_hosts {
                env.push((
                    env_key("known_hosts_file"),
                    file.to_string_lossy().into_owned(),
                ));
            }
        }
        RemoteProtocol::FtpsImplicit => env.push((env_key("tls"), "true".into())),
        RemoteProtocol::FtpsExplicit => env.push((env_key("explicit_tls"), "true".into())),
        RemoteProtocol::Ftp => {}
    }
    env
}

fn remote_argument(spec: &RemoteSpec) -> String {
    let path = spec.path.trim();
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        format!("{RCLONE_REMOTE}:")
    } else {
        format!("{RCLONE_REMOTE}:/{path}")
    }
}

/// Hängt ein Netzlaufwerk ein und liefert den Pfad, unter dem es erreichbar ist.
#[tauri::command]
pub async fn mount_remote(
    spec: RemoteSpec,
    password: String,
    allow_insecure: bool,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || mount_blocking(spec, password, allow_insecure))
        .await
        .map_err(|_| "err.remote.mountFailed".to_string())?
}

fn mount_blocking(
    spec: RemoteSpec,
    password: String,
    allow_insecure: bool,
) -> Result<String, String> {
    validate(&spec, allow_insecure)?;
    if password.is_empty() {
        return Err("err.remote.emptyPassword".into());
    }
    let rclone = rclone_executable()?;

    // Ohne geprüften Hostschlüssel keine SSH-Verbindung: Sonst ließe sich ein
    // Server unbemerkt austauschen und das Kennwort mitlesen.
    let known_hosts = if spec.protocol == RemoteProtocol::Sftp {
        let port = spec.port_or_default();
        if !is_trusted(&spec.host, port)? {
            return Err("err.remote.hostKeyUnknown".into());
        }
        Some(known_hosts_file()?)
    } else {
        None
    };

    let label = sanitize_label(if spec.label.trim().is_empty() {
        &spec.host
    } else {
        &spec.label
    })
    .ok_or_else(|| "err.remote.label".to_string())?;

    let obscured = obscure(&rclone, &password)?;
    let (mount_dir, final_label) = unique_mount_dir(&label)?;

    // Der Name enthält den Anzeigenamen, weil er innerhalb des Ordners eindeutig
    // ist. Sonst würden sich mehrere gleichzeitige Einhängungen gegenseitig ins
    // selbe Protokoll schreiben.
    let log_path = app_dir()?.join(format!("mount-{final_label}.log"));
    let log = std::fs::File::create(&log_path).map_err(|_| "err.remote.mountFailed".to_string())?;
    let log_err = log
        .try_clone()
        .map_err(|_| "err.remote.mountFailed".to_string())?;

    let cache_dir = app_dir()?.join("cache");
    let mut command = Command::new(&rclone);
    command
        .arg("nfsmount")
        .arg(remote_argument(&spec))
        .arg(&mount_dir)
        // Ohne Zwischenspeicher lehnt der NFS-Weg jedes Schreiben ab.
        .args(["--vfs-cache-mode", "full"])
        .args(["--dir-cache-time", "20s"])
        .args(["--volname", &final_label])
        .arg("--cache-dir")
        .arg(&cache_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    for (key, value) in rclone_env(&spec, &obscured, known_hosts.as_deref()) {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .map_err(|_| "err.remote.mountFailed".to_string())?;

    let deadline = Instant::now() + MOUNT_TIMEOUT;
    loop {
        if is_mount_point(&mount_dir) {
            break;
        }
        // Beendet sich rclone vorher, ist die Ursache im Protokoll zu finden.
        if let Ok(Some(_)) = child.try_wait() {
            let _ = std::fs::remove_dir(&mount_dir);
            return Err(mount_failure_message(&log_path));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir(&mount_dir);
            return Err("err.remote.mountTimeout".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let path = mount_dir.to_string_lossy().into_owned();
    if let Ok(mut list) = registry().lock() {
        list.push(ActiveMount {
            path: mount_dir,
            label: final_label,
            descriptor: spec.descriptor(),
            child,
            log: log_path,
        });
    }
    Ok(path)
}

/// Übersetzt die Protokollausgabe von rclone in eine der bekannten Kennungen.
/// Der Rohtext ist englisch und technisch; er hilft dem Benutzer nicht weiter.
fn mount_failure_message(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lower = text.to_lowercase();
    if lower.contains("knownhosts") || lower.contains("key mismatch") {
        return "err.remote.hostKeyChanged".into();
    }
    if lower.contains("permission denied") || (lower.contains("auth") && lower.contains("fail")) {
        return "err.remote.auth".into();
    }
    if lower.contains("no such host") || lower.contains("connection refused") {
        return "err.remote.unreachable".into();
    }
    if lower.contains("i/o timeout") || lower.contains("timed out") {
        return "err.remote.unreachable".into();
    }
    if lower.contains("certificate") {
        return "err.remote.certificate".into();
    }
    "err.remote.mountFailed".into()
}

// ---------------------------------------------------------------------------
// Aushängen
// ---------------------------------------------------------------------------

/// Liegt dieser Pfad unterhalb des eigenen Ordners für Netzlaufwerke?
///
/// Beide Seiten werden aufgelöst, weil der Aufrufer den Pfad meist schon durch
/// `canonicalize` geschickt hat. Unter macOS zeigt `/Users` auf
/// `/System/Volumes/Data/Users`; ein reiner Textvergleich ginge dann daneben.
pub fn is_remote_mount(path: &Path) -> bool {
    let Ok(root) = mount_root() else {
        return false;
    };
    let real_root = std::fs::canonicalize(&root).unwrap_or(root);
    let real_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real_path.starts_with(&real_root) && real_path != real_root
}

/// Hängt ein selbst eingehängtes Netzlaufwerk aus und beendet rclone.
/// Wird sowohl vom eigenen Befehl als auch vom allgemeinen Auswerfen benutzt.
pub fn unmount_owned(path: &Path) -> Result<(), String> {
    unmount_path(path)?;
    release(path);
    Ok(())
}

fn unmount_path(path: &Path) -> Result<(), String> {
    if !is_mount_point(path) {
        return Ok(());
    }
    // Kein erzwungenes Aushängen: Ein noch laufender Schreibvorgang würde sonst
    // mitten im Übertragen abgeschnitten.
    let out = Command::new("/sbin/umount")
        .arg(path)
        .output()
        .map_err(|_| "err.remote.unmountFailed".to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let out = Command::new("/usr/sbin/diskutil")
        .arg("unmount")
        .arg(path)
        .output()
        .map_err(|_| "err.remote.unmountFailed".to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if text.contains("busy") {
        return Err("err.remote.busy".into());
    }
    Err("err.remote.unmountFailed".into())
}

/// Hängt ein Netzlaufwerk aus und beendet den zugehörigen rclone-Prozess.
#[tauri::command]
pub async fn unmount_remote(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let target = PathBuf::from(&path);
        if !is_remote_mount(&target) {
            return Err("err.remote.notOurs".into());
        }
        unmount_owned(&target)
    })
    .await
    .map_err(|_| "err.remote.unmountFailed".to_string())?
}

/// Beendet den rclone-Prozess eines bereits ausgehängten Laufwerks und räumt
/// Ordner sowie Protokoll weg.
fn release(path: &Path) {
    let Ok(mut list) = registry().lock() else {
        return;
    };
    // Der Aufrufer hat den Pfad möglicherweise schon aufgelöst, in der Liste
    // steht dagegen die ursprüngliche Schreibweise. Deshalb beide Seiten
    // vergleichbar machen.
    let wanted = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let same = |candidate: &Path| {
        std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf()) == wanted
    };
    let Some(index) = list.iter().position(|entry| same(&entry.path)) else {
        // Nicht in der Liste: dann bleibt nur der leere Ordner zu entfernen.
        let _ = std::fs::remove_dir(path);
        return;
    };
    let mut entry = list.remove(index);
    // rclone beendet sich nach dem Aushängen von selbst. Kurz warten ist
    // freundlicher, als es sofort abzuschießen.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match entry.child.try_wait() {
            Ok(Some(_)) => break,
            _ if Instant::now() >= deadline => {
                let _ = entry.child.kill();
                let _ = entry.child.wait();
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let _ = std::fs::remove_file(&entry.log);
    let _ = std::fs::remove_dir(&entry.path);
}

/// Alle derzeit von DualBeam eingehängten Netzlaufwerke.
pub fn active_mounts() -> Vec<RemoteMountInfo> {
    let Ok(list) = registry().lock() else {
        return Vec::new();
    };
    list.iter()
        .filter(|entry| is_mount_point(&entry.path))
        .map(|entry| RemoteMountInfo {
            path: entry.path.to_string_lossy().into_owned(),
            label: entry.label.clone(),
            descriptor: entry.descriptor.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn remote_mounts() -> Vec<RemoteMountInfo> {
    active_mounts()
}

/// Hängt beim Beenden der App alles wieder aus. Bliebe ein Laufwerk stehen,
/// zeigte der Finder eine tote Freigabe, die sich nur noch von Hand lösen ließe.
pub fn unmount_all() {
    let paths: Vec<PathBuf> = match registry().lock() {
        Ok(list) => list.iter().map(|entry| entry.path.clone()).collect(),
        Err(_) => return,
    };
    for path in paths {
        let _ = unmount_path(&path);
        release(&path);
    }
}

/// Räumt Reste eines abgestürzten früheren Laufs weg: Ordner, die niemand mehr
/// eingehängt hat, und liegengebliebene Protokolle.
pub fn cleanup_stale() {
    let Ok(root) = mount_root() else {
        return;
    };
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !is_mount_point(&path) {
                let _ = std::fs::remove_dir(&path);
            }
        }
    }
    let Ok(dir) = app_dir() else {
        return;
    };
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("mount-") && name.ends_with(".log") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_checked() {
        assert!(valid_host("example.com"));
        assert!(valid_host("sftp.hidrive.ionos.com"));
        assert!(valid_host("192.168.1.10"));
        assert!(valid_host("[fe80::1]"));
        assert!(!valid_host(""));
        assert!(!valid_host("has space.com"));
        assert!(!valid_host("-leading.com"));
        assert!(!valid_host("trailing-.com"));
        assert!(!valid_host("double..dot"));
        assert!(!valid_host("semi;colon.com"));
    }

    #[test]
    fn usernames_are_checked() {
        assert!(valid_username("jano150"));
        assert!(valid_username("konto@example.com"));
        assert!(!valid_username(""));
        assert!(!valid_username("mit leerzeichen"));
        assert!(!valid_username("kom,ma"));
        assert!(!valid_username("doppel:punkt"));
    }

    #[test]
    fn parent_traversal_is_rejected() {
        assert!(valid_remote_path("/users/jano150"));
        assert!(valid_remote_path(""));
        assert!(!valid_remote_path("/users/../etc"));
        assert!(!valid_remote_path("..")); 
        assert!(!valid_remote_path("/users/\u{7}/x"));
    }

    #[test]
    fn labels_become_safe_folder_names() {
        assert_eq!(sanitize_label("HiDrive"), Some("HiDrive".into()));
        assert_eq!(sanitize_label("a/b"), Some("a-b".into()));
        assert_eq!(sanitize_label(".."), None);
        assert_eq!(sanitize_label("   "), None);
        assert_eq!(sanitize_label("mit Leerzeichen"), Some("mit Leerzeichen".into()));
    }

    fn spec(protocol: RemoteProtocol) -> RemoteSpec {
        RemoteSpec {
            protocol,
            host: "example.com".into(),
            port: None,
            username: "user".into(),
            path: "/data".into(),
            label: String::new(),
        }
    }

    #[test]
    fn plain_ftp_stays_on_the_local_network() {
        let mut s = spec(RemoteProtocol::Ftp);
        // Ohne ausdrückliche Zustimmung gar nicht.
        assert_eq!(
            validate(&s, false).unwrap_err(),
            "err.network.insecureConfirm"
        );
        // Mit Zustimmung, aber öffentlicher Name: weiterhin nicht.
        assert_eq!(validate(&s, true).unwrap_err(), "err.network.localIpOnly");
        s.host = "192.168.1.5".into();
        assert!(validate(&s, true).is_ok());
    }

    #[test]
    fn encrypted_protocols_need_no_confirmation() {
        assert!(validate(&spec(RemoteProtocol::Sftp), false).is_ok());
        assert!(validate(&spec(RemoteProtocol::FtpsExplicit), false).is_ok());
        assert!(validate(&spec(RemoteProtocol::FtpsImplicit), false).is_ok());
    }

    #[test]
    fn default_ports_match_the_protocol() {
        assert_eq!(RemoteProtocol::Sftp.default_port(), 22);
        assert_eq!(RemoteProtocol::Ftp.default_port(), 21);
        assert_eq!(RemoteProtocol::FtpsExplicit.default_port(), 21);
        assert_eq!(RemoteProtocol::FtpsImplicit.default_port(), 990);
    }

    #[test]
    fn tls_variants_map_to_the_right_option() {
        let implicit = rclone_env(&spec(RemoteProtocol::FtpsImplicit), "x", None);
        assert!(implicit.contains(&("RCLONE_CONFIG_DUALBEAM_TLS".into(), "true".into())));
        let explicit = rclone_env(&spec(RemoteProtocol::FtpsExplicit), "x", None);
        assert!(explicit.contains(&("RCLONE_CONFIG_DUALBEAM_EXPLICIT_TLS".into(), "true".into())));
        let plain = rclone_env(&spec(RemoteProtocol::Ftp), "x", None);
        assert!(!plain.iter().any(|(key, _)| key.contains("TLS")));
    }

    #[test]
    fn credentials_never_reach_the_command_line() {
        let env = rclone_env(&spec(RemoteProtocol::Sftp), "geheim", None);
        assert!(env.contains(&("RCLONE_CONFIG_DUALBEAM_PASS".into(), "geheim".into())));
        // Das Ziel-Argument enthält ausschließlich Name und Pfad.
        let argument = remote_argument(&spec(RemoteProtocol::Sftp));
        assert_eq!(argument, "DUALBEAM:/data");
        assert!(!argument.contains("geheim"));
    }

    #[test]
    fn empty_path_means_the_root_of_the_account() {
        let mut s = spec(RemoteProtocol::Sftp);
        s.path = String::new();
        assert_eq!(remote_argument(&s), "DUALBEAM:");
        s.path = "/".into();
        assert_eq!(remote_argument(&s), "DUALBEAM:");
    }

    #[test]
    fn known_hosts_pattern_includes_unusual_ports() {
        assert_eq!(host_pattern("example.com", 22), "example.com");
        assert_eq!(host_pattern("example.com", 2222), "[example.com]:2222");
    }
}
