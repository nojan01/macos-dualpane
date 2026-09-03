//! Netzlaufwerke über SSHFS (SFTP) sowie rclone (FTP/FTPS).
//!
//! SFTP verwendet SSHFS und damit das echte SFTP-Protokoll als FUSE-
//! Dateisystem. FTP und FTPS verbleiben auf ihrem bestehenden rclone-Weg.
//!
//! Zugangsdaten erreichen rclone ausschließlich über Umgebungsvariablen. Auf
//! der Kommandozeile stünden sie in der Prozessliste und wären damit für jedes
//! andere Programm des Benutzers lesbar.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::object_storage::{ObjectStorageProfile, ObjectStorageProtocol};

/// Schlüsselbund-Dienst für die Kennwörter der Netzlaufwerke.
const KEYCHAIN_SERVICE: &str = "com.nojan.dualbeam.remote";

/// Name, unter dem das Ziel innerhalb von rclone geführt wird. Er taucht nur in
/// den Umgebungsvariablen auf und ist für den Benutzer nie sichtbar.
const RCLONE_REMOTE: &str = "DUALBEAM";

/// Wie lange nach dem Start von rclone auf das fertige Laufwerk gewartet wird.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(45);

/// Die Anmeldung wird vor dem NFS-Mount geprüft. Ohne diese Vorprüfung kann
/// macOS den Mountpunkt schon anlegen, während der erste Verzeichniszugriff
/// bei einem abgelehnten SMB-/WebDAV-Kennwort unbegrenzt auf rclone wartet.
const REMOTE_VERIFY_TIMEOUT: Duration = Duration::from_secs(20);

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
    /// SMB/CIFS — das Protokoll von Windows-Freigaben, NAS-Geräten und Samba.
    ///
    /// Bewusst nicht über den Finder (`mount volume`): Der liefert für SMB nur
    /// den nichtssagenden Sammelfehler -5016 („Server antwortet nicht"), noch
    /// bevor er die Anmeldung versucht. Gemessen am selben Server meldet rclone
    /// dagegen genau, was fehlt. Über rclone liegt die Freigabe zudem im
    /// eigenen Ordner der App, womit Aushängen, Löschschutz und Übertragungen
    /// unverändert greifen.
    Smb,
    /// WebDAV über HTTPS — Nextcloud, ownCloud, pCloud, Fastmail und andere.
    ///
    /// Lief früher über den Finder (`mount volume`). Der fragt Benutzer und
    /// Kennwort in einem eigenen Fenster ab, weshalb DualBeam dafür keine
    /// Felder anbieten konnte: kein Anbieter, kein Adresspfad, kein
    /// Lesezeichen, ein wirkungsloses Zahnrad. Über rclone liegt die Freigabe
    /// wie alle anderen im Ordner der App, womit Aushängen, Löschschutz und
    /// Übertragungen unverändert greifen.
    Webdav,
}

impl RemoteProtocol {
    fn default_port(self) -> u16 {
        match self {
            Self::Sftp => 22,
            Self::Ftp | Self::FtpsExplicit => 21,
            Self::FtpsImplicit => 990,
            Self::Smb => 445,
            // WebDAV ist bei DualBeam immer TLS-gesichert; unverschlüsseltes
            // HTTP auf Port 80 überträgt das Kennwort im Klartext.
            Self::Webdav => 443,
        }
    }

    /// Für rclone ist FTPS kein eigener Typ, sondern FTP mit gesetzter
    /// TLS-Option.
    fn rclone_type(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Ftp | Self::FtpsExplicit | Self::FtpsImplicit => "ftp",
            Self::Smb => "smb",
            Self::Webdav => "webdav",
        }
    }

    /// Nur unverschlüsseltes FTP überträgt Kennwort und Inhalte im Klartext.
    ///
    /// SMB zählt hier als geschützt: Seit NTLMv2 wandert das Kennwort nie über
    /// die Leitung, der Server stellt eine Rechenaufgabe. Die Inhalte sind
    /// damit noch nicht verschlüsselt — darauf weist der Dialog hin. Eine
    /// Beschränkung auf blanke IP-Adressen wäre hier verfehlt, weil
    /// NAS-Geräte fast immer über ihren Namen angesprochen werden.
    pub fn is_encrypted(self) -> bool {
        !matches!(self, Self::Ftp)
    }

    fn scheme(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::FtpsExplicit | Self::FtpsImplicit => "ftps",
            Self::Smb => "smb",
            Self::Webdav => "webdav",
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
    /// Windows-Domäne oder Arbeitsgruppe. Nur für SMB, sonst leer. Wer sich
    /// als `DOMAENE\benutzer` anmeldet, trägt hier `DOMAENE` ein.
    #[serde(default)]
    pub domain: String,
    /// Pfad, der bei WebDAV noch zur Adresse gehört, nicht zum Inhalt.
    ///
    /// Nextcloud und ownCloud stellen ihre Dateien unter einem festen
    /// Unterpfad bereit (`/remote.php/dav/files/name`); pCloud und Fastmail
    /// antworten direkt auf der Wurzel. Getrennt von `path` gehalten, damit
    /// der Ordner innerhalb der Freigabe frei wählbar bleibt: Beides in ein
    /// Feld zu werfen hieße, dass ein Wechsel des Startordners die Adresse
    /// zerstört. Bei allen anderen Protokollen leer.
    #[serde(default)]
    pub base_path: String,
    /// Anbieterkennung für WebDAV (`nextcloud`, `owncloud`, `fastmail`,
    /// `sharepoint`, `other`). rclone passt danach sein Verhalten an, etwa
    /// stückweises Hochladen bei Nextcloud. Leer bedeutet `other`.
    #[serde(default)]
    pub vendor: String,
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
    /// Abweichender sichtbarer Startpfad eines S3-/Swift-Dateiraums.
    /// `path` bleibt der technische Mountpunkt für Aushängen und Transfers.
    pub home_path: Option<String>,
    pub label: String,
    pub descriptor: String,
}

/// Laufende Rückmeldung eines direkten SFTP-Uploads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpCopyProgress {
    Percent(u8),
    FileCopied(String),
}

struct ActiveMount {
    path: PathBuf,
    /// Für direkte Objekt-Speicher kann der sichtbare Start innerhalb der
    /// technischen Wurzel liegen (z. B. der Container „default").
    object_home: Option<PathBuf>,
    label: String,
    descriptor: String,
    rc_socket: PathBuf,
    child: Option<Child>,
    log: PathBuf,
    /// Die verifizierte Spezifikation eines klassischen rclone-Mounts.
    /// Objekt-Speicher verwendet stattdessen `object_profile`.
    remote_spec: Option<RemoteSpec>,
    /// S3/Swift werden nicht mehr über NFS eingehängt. Das Profil bleibt nur
    /// flüchtig im Speicher, damit die App alle Dateioperationen unmittelbar
    /// gegen die Objekt-Speicher-API ausführen kann.
    object_profile: Option<ObjectStorageProfile>,
    /// Das bereits verschleierte Kennwort dieses Mounts.
    ///
    /// Objekt-Speicher hält sein komplettes Profil im Speicher und kann
    /// deshalb jederzeit unmittelbar mit dem Server sprechen. Für WebDAV, SMB
    /// und FTP gilt hier dasselbe: Ohne diese Ablage müsste jeder
    /// Verzeichniswechsel das Kennwort erneut aus dem Schlüsselbund holen –
    /// und ginge leer aus, sobald dort keins hinterlegt ist, obwohl der
    /// laufende Mount durchgehend damit arbeitet.
    ///
    /// Der Wert ist verschleiert, wie rclone ihn erwartet, und lebt nur so
    /// lange wie der Mount.
    obscured_password: Option<String>,
    /// Das kurzlebige SSH_ASKPASS-Hilfsprogramm eines SSHFS-Mounts. Es enthält
    /// kein Kennwort; dieses liegt ausschließlich in der Umgebung des SSHFS-
    /// Prozesses. Der Pfad wird beim Aushängen wieder entfernt.
    sshfs_askpass: Option<PathBuf>,
}

fn registry() -> &'static Mutex<Vec<ActiveMount>> {
    static REGISTRY: OnceLock<Mutex<Vec<ActiveMount>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

static RC_SOCKET_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn rc_socket_path() -> Result<PathBuf, String> {
    let sequence = RC_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Kurzer Name: macOS begrenzt Unix-Domain-Socket-Pfade.
    Ok(app_dir()?.join(format!("rc-{}-{sequence}.sock", std::process::id())))
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
    value.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && !part.starts_with('-')
            && !part.ends_with('-')
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) && !value.starts_with('.')
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
    // Der Adresspfad wird Teil der URL. Ein Fragezeichen oder Rautenzeichen
    // schnitte alles Folgende ab, ein Doppelpunkt-Doppelschrägstrich führte auf
    // einen fremden Server — die Verbindung ginge dann stillschweigend woanders
    // hin, als der Benutzer eingetragen hat.
    if !spec.base_path.is_empty() {
        let base = spec.base_path.trim();
        if !valid_remote_path(base) || base.contains(['?', '#', '\\']) || base.contains("//") {
            return Err("err.remote.basePath".into());
        }
    }
    // Ohne Freigabe zeigt SMB nur auf die Liste der Freigaben. Der Finder kann
    // daraus eine Auswahl anbieten, ein Einhängepunkt lässt sich daraus nicht
    // bilden. Genau daran scheiterte bisher „smb://server" ohne Zusatz.
    if spec.protocol == RemoteProtocol::Smb && spec.path.trim().trim_matches('/').is_empty() {
        return Err("err.remote.shareMissing".into());
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
    // Auf case-insensitiven Dateisystemen (macOS-Standard) kann derselbe Ordner
    // bereits mit anderer Schreibweise bestehen – ältere Versionen legten
    // „dualbeam“ klein an. `create_dir_all` übernimmt ihn dann stillschweigend,
    // der hier zusammengebaute Pfad behält aber die geschriebene Schreibweise.
    // Pfade, die durch `canonicalize` laufen, tragen dagegen die echte
    // Schreibweise. Die Oberfläche vergleicht Mount-Pfade zeichenweise; beide
    // Varianten nebeneinander lassen diesen Vergleich scheitern. Deshalb wird
    // immer die echte Schreibweise geliefert.
    Ok(std::fs::canonicalize(&dir).unwrap_or(dir))
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
        // Testbinäre liegen noch eine Ebene tiefer, in target/debug/deps.
        // Ohne diesen Ort ließe sich kein Praxistest gegen einen echten
        // Server fahren.
        dir.join("../../../binaries")
            .join(format!("rclone-{triple}")),
    ];
    candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| path.to_path_buf())
        .ok_or_else(|| "err.remote.rcloneMissing".into())
}

/// SSHFS ist der echte Dateisystem-Client für den SFTP-Pfad. Anders als ein
/// rclone-NFS-Adapter spricht er direkt SSH/SFTP und stellt die entfernte
/// Wurzel als FUSE-Dateisystem bereit.
fn sshfs_executable() -> Result<PathBuf, String> {
    [
        PathBuf::from("/usr/local/bin/sshfs"),
        PathBuf::from("/opt/homebrew/bin/sshfs"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| "SSHFS ist nicht installiert. Bitte SSHFS für macFUSE installieren.".to_string())
}

/// Der SSHFS-Quellbezeichner bewahrt absichtlich die Schreibweise des
/// Profilpfads: `host:default` ist relativ zum SFTP-Home, `host:/default`
/// ist ein ausdrücklich absoluter Serverpfad.
fn sshfs_source(spec: &RemoteSpec) -> String {
    let host = spec.host.trim_start_matches('[').trim_end_matches(']');
    let address = if host.contains(':') {
        format!("{}@[{host}]", spec.username)
    } else {
        format!("{}@{host}", spec.username)
    };
    let path = spec.path.trim();
    let path = if path.is_empty() || path == "/" {
        "/"
    } else {
        path
    };
    format!("{address}:{path}")
}

/// SSHFS zerlegt `-o` zuerst selbst und übergibt die verbliebene Option danach
/// an OpenSSH. Leerzeichen benötigen deshalb zwei Backslashes: SSHFS entfernt
/// den ersten, OpenSSH verarbeitet den zweiten.
fn ssh_option_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace(' ', "\\\\ ")
}

/// OpenSSH zerlegt Werte aus `-o` erneut nach seiner Konfigurationssyntax.
/// Der gesamte Pfad muss deshalb auch dann in Anführungszeichen stehen, wenn
/// er bereits als einzelnes Prozessargument übergeben wurde.
fn openssh_option_path(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

/// OpenSSH nimmt Passwörter in einem nichtinteraktiven Prozess über
/// SSH_ASKPASS entgegen. Das Hilfsprogramm enthält dabei bewusst nur den
/// Verweis auf eine Prozessumgebung und nie das Kennwort selbst.
fn create_sshfs_askpass() -> Result<PathBuf, String> {
    let sequence = RC_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = app_dir()?.join(format!("sshfs-askpass-{}-{sequence}", std::process::id()));
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf '%s\\n' \"$DUALBEAM_SSHFS_PASSWORD\"\n",
    )
    .map_err(|_| "err.remote.mountFailed".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "err.remote.mountFailed".to_string())?;
    }
    Ok(path)
}

fn create_sftp_batch_file(script: &str) -> Result<PathBuf, String> {
    let sequence = RC_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = app_dir()?.join(format!("sftp-batch-{}-{sequence}", std::process::id()));
    std::fs::write(&path, script)
        .map_err(|error| format!("SFTP-Befehlsdatei konnte nicht erstellt werden: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("SFTP-Befehlsdatei konnte nicht geschützt werden: {error}"))?;
    }
    Ok(path)
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

/// Vergleicht ausschließlich Schlüsseltyp und öffentlichen Schlüssel – der
/// Host-Alias vor dem Schlüsselmaterial darf zwischen `ssh-keyscan` und
/// `ssh-keygen -F` abweichen. Kommentare und die `# Host`-Zeilen von
/// `ssh-keygen -F` werden ignoriert.
fn host_key_material(line: &str) -> Option<(&str, &str)> {
    let mut fields = line.split_whitespace();
    let _hosts = fields.next()?;
    let kind = fields.next()?;
    let key = fields.next()?;
    kind.starts_with("ssh-").then_some((kind, key))
}

fn known_host_matches_scan(known_hosts_output: &str, scanned: &[String]) -> bool {
    let scanned: Vec<_> = scanned
        .iter()
        .filter_map(|line| host_key_material(line))
        .collect();
    !scanned.is_empty()
        && known_hosts_output
            .lines()
            .filter_map(host_key_material)
            .any(|stored| scanned.contains(&stored))
}

/// Ein vorhandener Eintrag allein genügt nicht. Ist der Server-Schlüssel
/// ausgetauscht worden, muss der Dialog die neuen Fingerabdrücke zeigen und
/// eine erneute ausdrückliche Bestätigung verlangen.
fn trusted_host_matches_current_scan(
    host: &str,
    port: u16,
    scanned: &[String],
) -> Result<bool, String> {
    let file = known_hosts_file()?;
    let out = Command::new("/usr/bin/ssh-keygen")
        .args(["-F", &host_pattern(host, port), "-f"])
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "err.remote.keyscan".to_string())?;
    Ok(known_host_matches_scan(
        &String::from_utf8_lossy(&out.stdout),
        scanned,
    ))
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
        let lines = scan_host_keys(&host, port)?;
        let trusted = trusted_host_matches_current_scan(&host, port, &lines)?;
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
    remote_password(&spec)
}

/// Liest ein bereits gespeichertes Kennwort. Neben dem Dialog verwendet auch
/// der direkte rclone-Löschweg diese Funktion, damit das Geheimnis dafür nie
/// über die WebView zurückgegeben werden muss.
fn remote_password(spec: &RemoteSpec) -> Result<Option<String>, String> {
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
pub(crate) fn is_mount_point(path: &Path) -> bool {
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

/// Ein NFS-Mountpunkt kann bereits im Kernel erscheinen, obwohl rclone den
/// ersten Verzeichniszugriff noch nicht bedienen kann. Die Oberfläche würde
/// dann den frisch gemeldeten Mount öffnen, `read_dir` bekäme EIO und fiele
/// auf das Home-Verzeichnis zurück. Erst ein erfolgreicher Root-Listing-Zugriff
/// macht das Laufwerk für die Navigation wirklich bereit.
fn is_mount_ready(path: &Path) -> bool {
    is_mount_point(path) && std::fs::read_dir(path).is_ok()
}

/// Prüft Zugang und gewählten Startpfad unmittelbar mit rclone, bevor der
/// NFS-Mount angelegt wird. Dadurch kann ein falsches Kennwort nicht zu einem
/// Mountpunkt führen, dessen erster `read_dir`-Aufruf blockiert.
fn verify_rclone_connection(
    rclone: &Path,
    spec: &RemoteSpec,
    obscured_password: &str,
    known_hosts: Option<&Path>,
) -> Result<(), String> {
    let mut command = Command::new(rclone);
    command
        // `lsf` prüft Anmeldung und Leserechte, ohne die JSON-Metadaten
        // anzufordern. Gerade einige SMB-/Samba-Server beantworten `lsjson`
        // vor einem Mount unvollständig, obwohl die normale Freigabe danach
        // problemlos lesbar ist.
        .arg("lsf")
        .arg("--max-depth")
        .arg("1")
        .arg("--format")
        .arg("p")
        .arg("--contimeout")
        .arg("8s")
        .arg("--timeout")
        .arg("12s")
        .arg("--retries")
        .arg("1")
        .arg("--low-level-retries")
        .arg("1")
        .arg(remote_argument(spec))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in rclone_env(spec, obscured_password, known_hosts) {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "err.remote.mountFailed".to_string())?;
    let deadline = Instant::now() + REMOTE_VERIFY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|_| "err.remote.mountFailed".to_string())?;
                return if output.status.success() {
                    Ok(())
                } else {
                    Err(rclone_failure_message(&String::from_utf8_lossy(
                        &output.stderr,
                    )))
                };
            }
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("err.remote.mountFailed".into());
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("err.remote.mountTimeout".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// SMB-Server unterscheiden sich darin, welche Metadaten sie vor dem ersten
/// eigentlichen Mount preisgeben. Manche NAS-/Samba-Versionen lehnen `lsf`
/// mit einem unspezifischen Fehler ab, obwohl derselbe Zugang im NFS-Mount
/// danach sofort funktioniert. Authentifizierungs-, Erreichbarkeits- und
/// Zeitlimitfehler bleiben dagegen verbindlich: Sie würden einen Mount nur
/// warten lassen und müssen weiterhin sofort an die Oberfläche gehen.
fn may_continue_after_verification_error(protocol: RemoteProtocol, error: &str) -> bool {
    protocol == RemoteProtocol::Smb && error == "err.remote.mountFailed"
}

pub(crate) fn unique_mount_dir(label: &str) -> Result<(PathBuf, String), String> {
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
    // WebDAV kennt weder `host` noch `port`, sondern ausschliesslich eine
    // vollstaendige Adresse. Deshalb ein eigener Zweig statt eines Zusatzes zur
    // gemeinsamen Grundliste: Ein `host`, das das Backend nicht auswertet,
    // wuerde stillschweigend ignoriert — die Verbindung ginge dann auf eine
    // leere Adresse und scheiterte ohne erkennbaren Grund.
    if spec.protocol == RemoteProtocol::Webdav {
        let mut env = vec![
            (env_key("type"), spec.protocol.rclone_type().to_string()),
            (env_key("url"), webdav_url(spec)),
            (env_key("user"), spec.username.clone()),
            (env_key("pass"), obscured.to_string()),
        ];
        let vendor = spec.vendor.trim();
        env.push((
            env_key("vendor"),
            if vendor.is_empty() { "other" } else { vendor }.to_string(),
        ));
        return env;
    }

    let mut env = vec![
        (env_key("type"), spec.protocol.rclone_type().to_string()),
        (env_key("host"), spec.host.clone()),
        (env_key("user"), spec.username.clone()),
        (env_key("pass"), obscured.to_string()),
        (env_key("port"), spec.port_or_default().to_string()),
    ];
    match spec.protocol {
        RemoteProtocol::Sftp => {
            // Einige reine SFTP-Dienste (u. a. Infomaniak Swiss Backup)
            // unterstützen SETSTAT für Änderungszeiten nicht. Der Upload ist
            // bereits erfolgt, rclone wertet den optionalen Zeitstempel aber
            // sonst als Fehler und macOS zeigt nur noch EIO an.
            env.push((env_key("set_modtime"), "false".into()));
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
        RemoteProtocol::Smb => {
            if !spec.domain.trim().is_empty() {
                env.push((env_key("domain"), spec.domain.trim().to_string()));
            }
        }
        // Oben bereits vollstaendig behandelt und zurueckgegeben.
        RemoteProtocol::Webdav => {}
    }
    env
}

/// Setzt die Adresse zusammen, unter der rclone den WebDAV-Dienst anspricht.
///
/// Der Standardport bleibt weg, weil manche Dienste — pCloud darunter — bei
/// `https://host:443/` mit einer Umleitung antworten, die rclone als Fehler
/// wertet. Ein abweichender Port wird dagegen ausdrücklich genannt.
fn webdav_url(spec: &RemoteSpec) -> String {
    let host = spec.host.trim().trim_end_matches('/');
    let port = spec.port_or_default();
    let authority = if port == RemoteProtocol::Webdav.default_port() {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    let base = spec.base_path.trim().trim_matches('/');
    if base.is_empty() {
        format!("https://{authority}")
    } else {
        format!("https://{authority}/{base}")
    }
}

fn remote_argument(spec: &RemoteSpec) -> String {
    let path = spec.path.trim();
    if path.is_empty() || path == "/" {
        format!("{RCLONE_REMOTE}:")
    } else if spec.protocol == RemoteProtocol::Smb || spec.protocol == RemoteProtocol::Webdav {
        // Bei SMB ist der erste Pfadteil kein Ordner, sondern die Freigabe.
        // Die Wurzel des Zugangs ist die Liste der Freigaben, deshalb darf hier
        // kein führender Schrägstrich stehen.
        //
        // Bei WebDAV gilt dasselbe aus anderem Grund: Dort ist die Wurzel die
        // konfigurierte Adresse samt Adresspfad. Ein führender Schrägstrich
        // ließe rclone am Adresspfad vorbeigreifen.
        format!(
            "{RCLONE_REMOTE}:{path}",
            path = path.trim_start_matches('/')
        )
    } else if spec.protocol == RemoteProtocol::Sftp && !path.starts_with('/') {
        // Beim SFTP-Backend ist ein Pfad ohne führenden Slash relativ zum
        // angemeldeten SFTP-Home. Anbieter wie Infomaniak stellen dort etwa
        // den Container `default` bereit. `DUALBEAM:/default` würde hingegen
        // einen absoluten Serverpfad verlangen und am SFTP-Home vorbeigehen.
        format!("{RCLONE_REMOTE}:{path}")
    } else {
        format!(
            "{RCLONE_REMOTE}:/{path}",
            path = path.trim_start_matches('/')
        )
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

    let (mount_dir, final_label) = unique_mount_dir(&label)?;
    // Der Name enthält den Anzeigenamen, weil er innerhalb des Ordners eindeutig
    // ist. Sonst würden sich mehrere gleichzeitige Einhängungen gegenseitig ins
    // selbe Protokoll schreiben.
    let log_path = app_dir()?.join(format!("mount-{final_label}.log"));
    let log = std::fs::File::create(&log_path).map_err(|_| "err.remote.mountFailed".to_string())?;
    let log_err = log
        .try_clone()
        .map_err(|_| "err.remote.mountFailed".to_string())?;

    if spec.protocol == RemoteProtocol::Sftp {
        let sshfs = sshfs_executable()?;
        let askpass = create_sshfs_askpass()?;
        let source = sshfs_source(&spec);
        let known_hosts = known_hosts
            .as_ref()
            .expect("SFTP besitzt nach der Host-Key-Prüfung eine known_hosts-Datei");
        let mut command = Command::new(&sshfs);
        command
            // Im Vordergrund bleibt der Kindprozess der App zugeordnet und
            // wird beim Aushängen zuverlässig beendet.
            .arg("-f")
            .arg(&source)
            .arg(&mount_dir)
            .args(["-p", &spec.port_or_default().to_string()])
            .args(["-o", "StrictHostKeyChecking=yes"])
            .args([
                "-o",
                &format!("UserKnownHostsFile={}", ssh_option_path(known_hosts)),
            ])
            // Ausschließlich DualBeams bestätigte Schlüsseldatei verwenden;
            // globale Systemschlüssel könnten sonst für dieselbe Adresse einen
            // unpassenden, alten Eintrag beisteuern.
            .args(["-o", "GlobalKnownHostsFile=/dev/null"])
            .args(["-o", "reconnect"])
            // Sehr kurze Metadaten-Caches verhindern veraltete Verzeichnis-
            // listen, ohne die Dateiübertragung selbst künstlich zu drosseln.
            .args(["-o", "dcache_timeout=1"])
            .args(["-o", "entry_timeout=1"])
            .args(["-o", "attr_timeout=1"])
            .args(["-o", "auto_cache"])
            // SSHFS dokumentiert dies für SFTP-Server, die beim Anlegen einer
            // Datei mit einem nicht-null Dateimodus fälschlich scheitern. Der
            // Server erhält beim Create dann Modus 0 und die Rechte werden
            // anschließend wie üblich gesetzt.
            .args(["-o", "workaround=createmode"])
            .args(["-o", &format!("volname={final_label}")])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .env("SSH_ASKPASS", &askpass)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "dualbeam-sshfs")
            .env("DUALBEAM_SSHFS_PASSWORD", &password);
        let mut child = command
            .spawn()
            .map_err(|_| "SFTP-Dateisystem konnte nicht gestartet werden".to_string())?;
        let deadline = Instant::now() + MOUNT_TIMEOUT;
        loop {
            if is_mount_ready(&mount_dir) {
                break;
            }
            if let Ok(Some(_)) = child.try_wait() {
                let _ = std::fs::remove_file(&askpass);
                let _ = std::fs::remove_dir(&mount_dir);
                return Err(sshfs_failure_message(&log_path));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&askpass);
                let _ = std::fs::remove_dir(&mount_dir);
                return Err("err.remote.mountTimeout".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let path = mount_dir.to_string_lossy().into_owned();
        if let Ok(mut list) = registry().lock() {
            list.push(ActiveMount {
                path: mount_dir,
                object_home: None,
                label: final_label,
                descriptor: spec.descriptor(),
                rc_socket: PathBuf::new(),
                child: Some(child),
                log: log_path,
                remote_spec: Some(spec),
                object_profile: None,
                obscured_password: None,
                sshfs_askpass: Some(askpass),
            });
        }
        return Ok(path);
    }

    let rclone = rclone_executable()?;
    let obscured = obscure(&rclone, &password)?;
    if let Err(error) = verify_rclone_connection(
        &rclone,
        &spec,
        &obscured,
        known_hosts.as_deref(),
    ) {
        if !may_continue_after_verification_error(spec.protocol, &error) {
            let _ = std::fs::remove_dir(&mount_dir);
            return Err(error);
        }
    }
    let rc_socket = rc_socket_path()?;
    let _ = std::fs::remove_file(&rc_socket);
    let rc_addr = format!("unix://{}", rc_socket.display());

    let cache_dir = app_dir()?.join("cache");
    let dir_cache_time = "20s";
    let mut command = Command::new(&rclone);
    command
        .arg("nfsmount")
        .arg(remote_argument(&spec))
        .arg(&mount_dir)
        // Ohne Zwischenspeicher lehnt der NFS-Weg jedes Schreiben ab.
        .args(["--vfs-cache-mode", "full"])
        .args(["--dir-cache-time", dir_cache_time])
        .args(["--rc", "--rc-addr", &rc_addr])
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
        if is_mount_ready(&mount_dir) {
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
            object_home: None,
            label: final_label,
            descriptor: spec.descriptor(),
            rc_socket,
            child: Some(child),
            log: log_path,
            remote_spec: Some(spec.clone()),
            object_profile: None,
            obscured_password: Some(obscured.clone()),
            sshfs_askpass: None,
        });
    }
    Ok(path)
}

/// Öffnet ein S3- oder Swift-Profil als direkten DualBeam-Dateiraum.
///
/// Anders als SFTP und FTPS wird hierfür ausdrücklich kein NFS-Mount erzeugt:
/// Verzeichnisliste, Kopieren, Löschen und Anlegen verwenden rclone direkt
/// gegen die Objekt-Speicher-API. Der lokale Pfad ist lediglich eine stabile
/// Kennung für die beiden Dateifenster und enthält niemals Nutzdaten.
pub fn mount_object_storage(
    profile: &ObjectStorageProfile,
    secret: &str,
) -> Result<String, String> {
    let rclone = rclone_executable()?;
    let label = sanitize_label(&profile.name).ok_or_else(|| "err.remote.label".to_string())?;
    // Diese Werte werden nur in die Umgebung des kurzlebigen rclone-Prozesses
    // gesetzt. Anders als Einträge in einer rclone.conf erwartet rclone bei
    // RCLONE_CONFIG_* den Rohwert; ein vorheriges `rclone obscure` führt bei
    // S3 zu SignatureDoesNotMatch und bei Swift zu einer abgelehnten Anmeldung.
    let env = object_storage_env(profile, secret);
    // Der Einhängepunkt ist die Wurzel oder der im Profil gewählte Bucket bzw.
    // Container. Ein Präfix wird wie bei WebDAV im Dateifenster gewählt und
    // kann dann als konkretes Ziel eines Syncprofils gespeichert werden.
    let argument = object_storage_argument(profile);

    // Ein nfsmount wird schon erfolgreich in das Dateisystem eingehängt, wenn
    // der erste Zugriff auf den Objekt-Speicher später mit einem 403 scheitert.
    // Das hinterlässt ein scheinbar leeres Laufwerk; jede Synchronisation endet
    // dann lediglich mit dem irreführenden macOS-Fehler "Not a directory".
    // Die Verbindung deshalb vor dem Einhängen einmal abfragen.
    verify_object_storage_connection(&rclone, &argument, &env)?;
    let descriptor = format!(
        "{}://{}",
        match profile.protocol {
            ObjectStorageProtocol::S3 => "s3",
            ObjectStorageProtocol::Swift => "swift",
        },
        profile.id
    );
    let (mount_dir, final_label) = unique_mount_dir(&label)?;
    let rc_socket = rc_socket_path()?;
    let _ = std::fs::remove_file(&rc_socket);
    let log_path = app_dir()?.join(format!("mount-{final_label}.log"));
    // Ein leeres Protokoll ist hilfreich für die Aufräumlogik und macht den
    // direkten Dateiraum von den klassischen rclone-NFS-Mounts unterscheidbar.
    std::fs::File::create(&log_path).map_err(|_| "err.remote.mountFailed".to_string())?;
    // Der aktive Dateiraum verwendet unverändert das im Profil gespeicherte
    // Remote-Ziel. Die Navigationsbegrenzung arbeitet ausschließlich mit dem
    // lokalen Mountpunkt und darf dieses Ziel nicht umschreiben.
    let object_home = mount_dir.clone();
    let path = object_home.to_string_lossy().into_owned();
    if let Ok(mut list) = registry().lock() {
        list.push(ActiveMount {
            path: mount_dir,
            object_home: Some(object_home),
            label: final_label,
            descriptor,
            rc_socket,
            child: None,
            log: log_path,
            remote_spec: None,
            object_profile: Some(profile.clone()),
            obscured_password: None,
            sshfs_askpass: None,
        });
    }
    Ok(path)
}

/// Baut die ausschließlich für einen rclone-Prozess gültige Objekt-Speicher-
/// Konfiguration. Sie wird beim Einhängen und beim direkten Löschen verwendet;
/// Zugangsdaten erscheinen dadurch weder auf der Kommandozeile noch in einer
/// rclone-Konfigurationsdatei.
fn object_storage_env(profile: &ObjectStorageProfile, secret: &str) -> Vec<(String, String)> {
    match profile.protocol {
        ObjectStorageProtocol::S3 => vec![
            (env_key("type"), "s3".to_string()),
            (env_key("provider"), "Other".to_string()),
            (env_key("env_auth"), "false".to_string()),
            (
                env_key("access_key_id"),
                profile.access_key.trim().to_string(),
            ),
            (env_key("secret_access_key"), secret.to_string()),
            (env_key("region"), profile.region.trim().to_string()),
            (env_key("endpoint"), profile.endpoint.trim().to_string()),
            (env_key("force_path_style"), profile.path_style.to_string()),
        ],
        ObjectStorageProtocol::Swift => {
            let identity_path = profile.swift_identity_path.trim();
            let endpoint = profile.endpoint.trim().trim_end_matches('/');
            let auth_url = if endpoint.ends_with(identity_path) {
                endpoint.to_string()
            } else {
                format!("{endpoint}{identity_path}")
            };
            vec![
                (env_key("type"), "swift".to_string()),
                (env_key("env_auth"), "false".to_string()),
                (env_key("user"), profile.username.trim().to_string()),
                (env_key("key"), secret.to_string()),
                (env_key("auth"), auth_url),
                (env_key("tenant"), profile.swift_project.trim().to_string()),
                (
                    env_key("domain"),
                    profile.swift_user_domain.trim().to_string(),
                ),
                (
                    env_key("tenant_domain"),
                    profile.swift_project_domain.trim().to_string(),
                ),
                (env_key("region"), profile.region.trim().to_string()),
            ]
        }
    }
}

/// rclone-Ziel für den im Profil gewählten Bucket/Container. Ein leerer Wert
/// ist absichtlich die Wurzel aller Buckets bzw. Container und wird beim
/// direkten Löschen unten aus Sicherheitsgründen niemals akzeptiert.
fn object_storage_argument(profile: &ObjectStorageProfile) -> String {
    let container = profile.container.trim_matches('/');
    let container = match profile.protocol {
        ObjectStorageProtocol::S3 => match container.split_once('/') {
            Some((bucket, path)) => format!("{}/{}", bucket.to_ascii_lowercase(), path),
            None => container.to_ascii_lowercase(),
        },
        ObjectStorageProtocol::Swift => container.to_string(),
    };
    if container.is_empty() {
        format!("{RCLONE_REMOTE}:")
    } else {
        format!("{RCLONE_REMOTE}:{container}")
    }
}

/// Löscht Elemente eines S3- oder Swift-Mounts direkt über rclone statt über
/// das NFS-Dateisystem. Für einen Ordner bedeutet das einen `purge`-Aufruf auf
/// dem Objekt-Speicher: rclone kann dabei die Backend-Batch-API verwenden bzw.
/// mehrere Objektlöschungen parallel ausführen. Das vermeidet die bislang
/// tausenden sequentiellen `unlink`-Aufrufe des macOS-NFS-Clients.
pub fn purge_object_storage(
    profile: &ObjectStorageProfile,
    _mount_path: &Path,
    paths: &[PathBuf],
    directory_paths: &[PathBuf],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    if paths.is_empty() || cancel.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    let expected = format!(
        "{}://{}",
        match profile.protocol {
            ObjectStorageProtocol::S3 => "s3",
            ObjectStorageProtocol::Swift => "swift",
        },
        profile.id
    );
    // Genau wie beim direkten Kopieren wird der Schutzbeweis aus den aktiven
    // Pfaden abgeleitet, nicht aus einem von der WebView gelieferten
    // Mount-String. Virtuelle Präfixe existieren nicht lokal und macOS kann
    // Schreibweisen des übergeordneten App-Support-Pfads normalisieren.
    let contexts: Option<Vec<_>> = paths
        .iter()
        .map(|path| object_storage_mount_context(path))
        .collect();
    let Some(contexts) = contexts else {
        object_storage_copy_log("delete rejected: path is not in an active object-storage mount");
        return Err("err.remote.notOurs".into());
    };
    let Some(first) = contexts.first() else {
        return Ok(());
    };
    if first.descriptor != expected
        || contexts
            .iter()
            .any(|context| context.descriptor != first.descriptor)
    {
        object_storage_copy_log("delete rejected: object-storage profiles do not match");
        return Err("err.remote.notOurs".into());
    }
    let profile = first.profile.clone();
    let mount_path = first.mount_path.clone();
    crate::object_storage::validate(&profile)?;
    object_storage_copy_log(&format!(
        "delete start profile={} paths={}",
        profile.id,
        paths.len()
    ));
    let secret = crate::object_storage::object_storage_secret(&profile.id)?;
    let rclone = rclone_executable()?;
    let base = object_storage_argument(&profile);
    // Eine Sync-Vorschau kann sowohl einen Ordner als auch dessen Kinder
    // enthalten. Sobald der Ordner per `purge` gelöscht wird, wären die
    // nachfolgenden Einzel-Löschungen nur noch irreführende „not found“-Fehler.
    // Deshalb behalten wir nur die obersten ausgewählten Teilbäume.
    let mut targets: Vec<(PathBuf, String, bool)> = Vec::new();
    for path in paths {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let context =
            object_storage_mount_context(path).ok_or_else(|| "err.remote.notOurs".to_string())?;
        let relative = context
            .real_path
            .strip_prefix(&mount_path)
            .map_err(|_| "Ungültiger Objekt-Speicherpfad".to_string())?;
        let mut parts = Vec::new();
        for component in relative.components() {
            match component {
                std::path::Component::Normal(part) => parts.push(
                    part.to_str()
                        .ok_or_else(|| "Ungültiger Objekt-Speicherpfad".to_string())?,
                ),
                std::path::Component::CurDir => {}
                _ => return Err("Ungültiger Objekt-Speicherpfad".into()),
            }
        }
        // Die Wurzel darf nie per „purge“ gelöscht werden: Bei einem leeren
        // Containerfeld wäre das sonst die Liste aller Container des Kontos.
        if parts.is_empty() {
            return Err(
                "Das Stammverzeichnis des Objekt-Speichers kann nicht gelöscht werden".into(),
            );
        }
        let target = format!("{}/{}", base.trim_end_matches('/'), parts.join("/"));
        // Die Pane übergibt die beim Listing bekannte Ordnerart. Bei virtuellen
        // Objekt-Speicher-Präfixen ist das verlässlicher als `stat` über den
        // NFS-Mount. Ein übergebener Hinweis wird nur für tatsächlich
        // ausgewählte Pfade akzeptiert; er kann somit keinen fremden Pfad als
        // rekursives Löschziel markieren.
        let listed_as_dir = directory_paths.iter().any(|directory| directory == path);
        // Wenn ein selektierter Pfad weitere selektierte Pfade enthält, muss er
        // ebenfalls ein Ordner sein. Dadurch bleibt eine Sync-Löschung mit
        // Ordner und Kindern stets bei einem einzigen `rclone purge`.
        let contains_selected_child = paths
            .iter()
            .any(|other| other != path && other.starts_with(path));
        let is_dir = listed_as_dir
            || contains_selected_child
            || match object_storage_path_is_dir(path) {
                Some(result) => result?,
                None => std::fs::symlink_metadata(path)
                    .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
                    .unwrap_or(false),
            };
        if targets
            .iter()
            .any(|(selected, _, selected_is_dir)| *selected_is_dir && path.starts_with(selected))
        {
            continue;
        }
        if is_dir {
            targets.retain(|(selected, _, _)| !selected.starts_with(path));
        }
        targets.push((path.clone(), target, is_dir));
    }
    for (_, target, is_dir) in targets {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let mut command = Command::new(&rclone);
        command
            .arg(if is_dir { "purge" } else { "deletefile" })
            .args([
                "--checkers",
                "32",
                "--contimeout",
                "15s",
                "--timeout",
                "2m",
                "--retries",
                "3",
            ])
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (key, value) in object_storage_env(&profile, &secret) {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "err.remote.mountFailed".to_string())?;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let output = child
                        .wait_with_output()
                        .map_err(|_| "Objekt-Speicher-Löschen fehlgeschlagen".to_string())?;
                    if status.success() {
                        object_storage_copy_log("delete completed");
                        break;
                    }
                    let error = object_storage_copy_error(&output.stderr, &secret).replacen(
                        "Objekt-Speicher-Kopie",
                        "Objekt-Speicher-Löschen",
                        1,
                    );
                    object_storage_copy_log(&format!("delete failed ({status}): {error}"));
                    return Err(error);
                }
                Ok(None) if cancel.load(std::sync::atomic::Ordering::SeqCst) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    object_storage_copy_log("delete cancelled");
                    return Ok(());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return Err("Objekt-Speicher-Löschen fehlgeschlagen".into()),
            }
        }
    }
    Ok(())
}

/// Kopiert ein Element eines S3-/Swift-Laufwerks direkt mit rclone. Der
/// sichtbare NFS-Mount bleibt für Navigation und Auswahl bestehen; beim
/// Datentransfer umgehen wir ihn aber. Der NFS-Adapter kann bei tiefen
/// Objektbäumen auf macOS fälschlich EMFILE ("Too many open files") melden,
/// obwohl die Prozesslimits nicht ausgeschöpft sind.
pub fn copy_object_storage(
    profile: &ObjectStorageProfile,
    mount_path: &Path,
    source_is_object_storage: bool,
    source: &Path,
    destination: &Path,
    force_overwrite: bool,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    object_storage_copy_log(&format!(
        "start profile={} direction={} source={} destination={}",
        profile.id,
        if source_is_object_storage {
            "download"
        } else {
            "upload"
        },
        source.display(),
        destination.display()
    ));
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        object_storage_copy_log("cancelled before rclone start");
        return Ok(());
    }
    let expected = format!(
        "{}://{}",
        match profile.protocol {
            ObjectStorageProtocol::S3 => "s3",
            ObjectStorageProtocol::Swift => "swift",
        },
        profile.id
    );
    // Den Einhängepunkt nicht nochmals als Text vergleichen: macOS liefert
    // ihn für denselben Ordner teilweise mit anderer Schreibweise zurück.
    // Der konkrete Quell- bzw. Zielpfad wird dagegen hier erneut gegen die
    // aktive Registrierung aufgelöst. Damit bleibt der Sicherheitsnachweis
    // erhalten und ist zugleich robust gegenüber Pfadnormalisierung.
    let object_path = if source_is_object_storage {
        source
    } else {
        destination
    };
    let active = match object_storage_mount_context(object_path) {
        Some(active) => active,
        None if profile.protocol == ObjectStorageProtocol::S3 => {
            // Ein S3-Dateiraum ist absichtlich kein echtes macOS-Dateisystem.
            // Nach einem App-Neustart kann daher noch ein lokaler, leerer
            // Kennungspfad bestehen, obwohl dessen flüchtige Registrierung
            // verloren gegangen ist. Ausschließlich für S3 stellen wir die
            // Registrierung hier aus dem zugehörigen Profil wieder her und
            // bilden den relativen Pfad auf den aktiven Dateiraum ab. Swift
            // behält seinen bewährten unveränderten Pfad.
            reconnect_s3_copy_context(profile, mount_path, object_path)?.ok_or_else(|| {
                object_storage_copy_log(
                    "rejected: S3 source or destination is outside the declared mount path",
                );
                "err.remote.notOurs".to_string()
            })?
        }
        None => {
            object_storage_copy_log(
                "rejected: source or destination is not an active object-storage path",
            );
            return Err("err.remote.notOurs".into());
        }
    };
    if active.descriptor != expected {
        object_storage_copy_log(&format!(
            "rejected: requested profile does not match active descriptor {}",
            active.descriptor
        ));
        return Err("err.remote.notOurs".into());
    }
    let profile = active.profile;
    crate::object_storage::validate(&profile)?;
    let relative = active
        .real_path
        .strip_prefix(&active.mount_path)
        .map_err(|_| "Ungültiger Objekt-Speicherpfad".to_string())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| "Ungültiger Objekt-Speicherpfad".to_string())?,
            ),
            std::path::Component::CurDir => {}
            _ => return Err("Ungültiger Objekt-Speicherpfad".into()),
        }
    }
    if parts.is_empty() {
        object_storage_copy_log("rejected: requested copy of the object-storage root");
        return Err("Das Stammverzeichnis des Objekt-Speichers kann nicht kopiert werden".into());
    }
    let remote_path = format!(
        "{}/{}",
        object_storage_argument(&profile).trim_end_matches('/'),
        parts.join("/")
    );
    let secret = crate::object_storage::object_storage_secret(&profile.id)?;
    let rclone = rclone_executable()?;
    let (from, to) = if source_is_object_storage {
        (remote_path, destination.to_string_lossy().into_owned())
    } else {
        (source.to_string_lossy().into_owned(), remote_path)
    };
    object_storage_copy_log(&format!("rclone copyto from={from} to={to}"));
    let mut command = Command::new(&rclone);
    command
        .arg("copyto")
        .arg(from)
        .arg(to)
        .args([
            "--checkers",
            "1",
            "--contimeout",
            "15s",
            "--timeout",
            "2m",
            "--retries",
            "3",
            "--low-level-retries",
            "2",
            // Eine vom Benutzer ausgelöste Kopie ist ein bewusster Auftrag,
            // kein inkrementeller Sync. rclone darf sie daher nicht wegen
            // zufällig gleicher Größe/Zeitstempel still überspringen.
            "--ignore-times",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Bei einer einzelnen Datei bleibt auch mit „Schnell“ ein HTTP-Stream
    // aktiv. Bei Ordnern nutzt rclone diese Parallelität für mehrere Dateien;
    // damit ist der Schalter wirksam, ohne WebDAV mit vielen Prüfungen zu
    // belasten (die Checkers bleiben bewusst bei 1).
    command.args(["--transfers", &profile.parallel_transfers.to_string()]);
    // Beim Herunterladen auf ein WebDAV-Laufwerk darf rclone nicht erst in
    // eine temporäre Datei schreiben und diese anschließend umbenennen. Das
    // macOS-WebDAV-Dateisystem verweigert diesen zweiten Schritt je nach
    // Server mit einem generischen Fehler. Der Inhalt wird daher direkt in
    // die Zieldatei geschrieben. Eine abgebrochene Übertragung kann nur diese
    // eine Datei unvollständig hinterlassen, nie eine vorhandene Datei still
    // durch einen leeren Platzhalter ersetzen.
    if source_is_object_storage {
        command.arg("--inplace");
    }
    let _ = force_overwrite; // Teil der stabilen IPC-Form für Kopierjobs.
    for (key, value) in object_storage_env(&profile, &secret) {
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|error| {
        object_storage_copy_log(&format!("could not start rclone: {error}"));
        "Objekt-Speicher-Kopie konnte nicht gestartet werden".to_string()
    })?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().map_err(|error| {
                    object_storage_copy_log(&format!("could not collect rclone output: {error}"));
                    "Objekt-Speicher-Kopie fehlgeschlagen".to_string()
                })?;
                if status.success() {
                    object_storage_copy_log("rclone reported success; verifying destination");
                    // Für einen nachträglich wiederhergestellten S3-Mount
                    // zeigt `source` bzw. `destination` noch auf dessen alte
                    // lokale Kennung. Die Bestätigung muss dagegen immer den
                    // tatsächlich aktiven Objekt-Speicherpfad abfragen.
                    let verified = if source_is_object_storage {
                        verify_object_storage_copy(true, &active.real_path, destination)
                    } else {
                        verify_object_storage_copy(false, source, &active.real_path)
                    };
                    match &verified {
                        Ok(()) => object_storage_copy_log("copy completed and verified"),
                        Err(error) => {
                            object_storage_copy_log(&format!("verification failed: {error}"))
                        }
                    }
                    return verified;
                }
                let error = object_storage_copy_error(&output.stderr, &secret);
                object_storage_copy_log(&format!("rclone failed ({status}): {error}"));
                return Err(error);
            }
            Ok(None) if cancel.load(std::sync::atomic::Ordering::SeqCst) => {
                let _ = child.kill();
                let _ = child.wait();
                object_storage_copy_log("cancelled while rclone was running");
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                object_storage_copy_log(&format!("could not query rclone status: {error}"));
                return Err("Objekt-Speicher-Kopie fehlgeschlagen".into());
            }
        }
    }
}

/// Protokolliert ausschließlich Diagnoseinformationen zu direkten S3-/Swift-
/// Kopien. Das Log liegt im privaten App-Ordner und enthält nie Zugangsdaten;
/// es erleichtert die Diagnose von Serverantworten, die rclone sonst nur über
/// stderr ausgibt.
fn object_storage_copy_log(message: &str) {
    let Ok(path) = app_dir().map(|dir| dir.join("object-storage-copy.log")) else {
        return;
    };
    let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let _ = writeln!(log, "{timestamp} {message}");
}

/// Ergänzt das private, zugangsdatenfreie Objekt-Speicher-Protokoll um einen
/// Eintrag aus dem Job-Dispatcher. So lässt sich unterscheiden, ob ein Fehler
/// im direkten rclone-Transfer oder erst in einer nachgelagerten UI-Aktion
/// entstanden ist.
pub fn log_object_storage_operation(message: &str) {
    object_storage_copy_log(message);
}

/// rclone kann bei einem falschen virtuellen Pfad mit Erfolg enden, ohne eine
/// Datei zu übertragen. Für reguläre Dateien bestätigen wir daher den
/// tatsächlichen Zielzustand, bevor der Job der Oberfläche als fertig gilt.
fn verify_object_storage_copy(
    source_is_object_storage: bool,
    source: &Path,
    destination: &Path,
) -> Result<(), String> {
    if source_is_object_storage {
        let Some(result) = object_storage_entry(source) else {
            return Err("Objekt-Speicher-Quelle ist nicht verfügbar".into());
        };
        let Some(source_entry) = result? else {
            return Err("Objekt-Speicher-Quelle wurde nicht gefunden".into());
        };
        if !source_entry.is_dir {
            let target_size = std::fs::metadata(destination)
                .map(|metadata| metadata.len())
                .map_err(|error| format!("Kopierziel wurde nicht angelegt: {error}"))?;
            if target_size != source_entry.size {
                return Err(format!(
                    "Kopierziel ist unvollständig: erwartet {} Byte, gefunden {target_size} Byte",
                    source_entry.size
                ));
            }
        }
    } else if let Ok(source_meta) = std::fs::metadata(source) {
        if !source_meta.is_dir() {
            let Some(result) = object_storage_entry(destination) else {
                return Err("Objekt-Speicher-Ziel ist nicht verfügbar".into());
            };
            let Some(target) = result? else {
                return Err("Kopierziel wurde nicht im Objekt-Speicher angelegt".into());
            };
            if target.is_dir || target.size != source_meta.len() {
                return Err(format!(
                    "Kopierziel ist unvollständig: erwartet {} Byte, gefunden {} Byte",
                    source_meta.len(),
                    target.size
                ));
            }
        }
    }
    Ok(())
}

/// rclone nennt die tatsächliche Ursache (z. B. einen abgelehnten WebDAV-
/// Schreibzugriff). Diese Information ist für eine Fehlerdiagnose notwendig,
/// darf aber nie das Schlüsselbund-Geheimnis enthalten.
fn object_storage_copy_error(stderr: &[u8], secret: &str) -> String {
    let message = String::from_utf8_lossy(stderr)
        .replace(secret, "[geschützt]")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let message: String = message.chars().take(1_500).collect();
    if message.is_empty() {
        "Objekt-Speicher-Kopie fehlgeschlagen".into()
    } else {
        format!("Objekt-Speicher-Kopie fehlgeschlagen: {message}")
    }
}

/// Kopiert zwischen einer lokalen Seite und einem aktiven SFTP-Mount direkt
/// mit rclone. Der macOS-NFS-Adapter von rclone kennt kein exklusives Anlegen
/// (`O_EXCL`), das `copyfile` für reguläre Finder-Kopien verwendet. Bei
/// verschachtelten Ordnern kann er dadurch außerdem Elternordner nur im Cache
/// sehen und mit „Permission denied“ abbrechen. FTP und FTPS verwenden
/// weiterhin unverändert ihren Dateisystemweg.
#[cfg(test)]
#[allow(dead_code)]
// Acht Parameter: Verbindungsdaten, Quelle, Ziel und Fortschritts-Rueckmeldung.
// Ein Zusammenfassen in eine Struktur wuerde die Aufrufstellen nicht klarer
// machen, da jeder Wert nur hier gebraucht wird.
#[allow(clippy::too_many_arguments)]
pub fn copy_sftp_storage(
    requested_spec: &RemoteSpec,
    mount_path: &Path,
    source_is_remote: bool,
    source: &Path,
    destination: &Path,
    force_overwrite: bool,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(SftpCopyProgress),
) -> Result<(), String> {
    if requested_spec.protocol != RemoteProtocol::Sftp {
        return Err("err.remote.notOurs".into());
    }
    validate(requested_spec, false)?;
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }

    let expected = requested_spec.descriptor();
    let real_mount = std::fs::canonicalize(mount_path).unwrap_or_else(|_| mount_path.to_path_buf());
    // Nicht die von der WebView gelieferte SFTP-Spezifikation verwenden:
    // Maßgeblich ist ausschließlich die beim Einhängen geprüfte Variante.
    let spec = registry()
        .lock()
        .ok()
        .and_then(|list| {
            list.iter().find_map(|entry| {
                let entry_mount =
                    std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
                (entry_mount == real_mount && entry.descriptor == expected)
                    .then(|| entry.remote_spec.clone())
                    .flatten()
            })
        })
        .filter(|spec| {
            spec.protocol == RemoteProtocol::Sftp
                && spec.host == requested_spec.host
                && spec.username == requested_spec.username
                && spec.port == requested_spec.port
                && spec.path == requested_spec.path
        })
        .ok_or_else(|| "err.remote.notOurs".to_string())?;

    let remote_path = if source_is_remote {
        sftp_transfer_target(source, &real_mount, &spec)?
    } else {
        sftp_transfer_target(destination, &real_mount, &spec)?
    };
    let source_meta = std::fs::symlink_metadata(source)
        .map_err(|error| format!("{}: {error}", source.display()))?;
    let source_is_dir = source_meta.is_dir() && !source_meta.file_type().is_symlink();
    let password = remote_password(&spec)?.ok_or_else(|| "err.remote.emptyPassword".to_string())?;
    let rclone = rclone_executable()?;
    let known_hosts = if !is_trusted(&spec.host, spec.port_or_default())? {
        return Err("err.remote.hostKeyUnknown".into());
    } else {
        Some(known_hosts_file()?)
    };
    let obscured = obscure(&rclone, &password)?;
    let (from, to) = if source_is_remote {
        (
            remote_path.clone(),
            destination.to_string_lossy().into_owned(),
        )
    } else {
        (source.to_string_lossy().into_owned(), remote_path.clone())
    };
    let mut command = Command::new(&rclone);
    command
        .arg(if source_is_dir { "copy" } else { "copyto" })
        .args([
            "--transfers",
            "4",
            "--checkers",
            "1",
            "--contimeout",
            "15s",
            "--timeout",
            "2m",
            "--retries",
            "3",
            "--low-level-retries",
            "2",
            // Manche eingeschränkten SFTP-Server können keine zuverlässigen
            // Verzeichnis-Zeitstempel setzen. Das ist für eine Kopie ohne
            // Bedeutung und darf keinen ansonsten erfolgreichen Transfer
            // scheitern lassen.
            "--no-update-dir-modtime",
            // rclone schreibt mit diesen Optionen etwa zweimal pro Sekunde
            // eine einzeilige Byte-/Prozent-Statistik nach stderr. Dadurch
            // bleibt die DualBeam-Statusleiste auch bei einem großen einzelnen
            // Ordner sichtbar aktiv.
            "--stats",
            "500ms",
            "--stats-one-line",
            "--stats-log-level",
            "NOTICE",
            // Der für SFTP vorgesehene rclone-Modus schreibt direkt unter
            // dem endgültigen Dateinamen. Damit entfällt der nachgelagerte
            // atomare Rename der .partial-Datei, den einzelne Server mit
            // „object not found“ quittieren. Dies ist kein Fallback.
            "--inplace",
        ])
        .arg(&from)
        .arg(&to)
        // INFO-Meldungen enthalten pro bestätigter Datei "Copied (...)".
        // So kann DualBeam die Dateizählung bereits während eines großen
        // Ordnertransfers aktualisieren.
        .arg("-v")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Eine bewusste Kopie darf bei einem bestehenden Ziel nicht wegen gleicher
    // Zeitstempel ausgelassen werden. Bei Konflikten hat die Oberfläche
    // `force_overwrite` bereits geprüft; für neue Dateien ist das Flag egal.
    if force_overwrite {
        command.arg("--ignore-times");
    }
    for (key, value) in rclone_env(&spec, &obscured, known_hosts.as_deref()) {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "SFTP-Kopie konnte nicht gestartet werden".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "SFTP-Kopie konnte nicht gestartet werden".to_string())?;
    let (log_tx, log_rx) = mpsc::channel::<String>();
    let log_reader = std::thread::spawn(move || {
        let mut collected = Vec::new();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let _ = log_tx.send(line.clone());
            collected.push(line);
        }
        collected.join("\n")
    });
    let mut last_percent = 0_u8;
    progress(SftpCopyProgress::Percent(0));
    loop {
        while let Ok(line) = log_rx.try_recv() {
            report_sftp_copy_log_line(progress, &mut last_percent, &line);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // Nach dem Prozessende wird stderr geschlossen; der Reader
                // liefert dadurch sowohl die letzte 100%-Statistik als auch
                // eine mögliche Fehlermeldung vollständig ab.
                let stderr = log_reader.join().unwrap_or_default();
                // Der letzte INFO-Eintrag kann zwischen Poll und Jobende
                // eintreffen. Er gehört ebenfalls in die Dateizählung.
                while let Ok(line) = log_rx.try_recv() {
                    report_sftp_copy_log_line(progress, &mut last_percent, &line);
                }
                if status.success() {
                    progress(SftpCopyProgress::Percent(100));
                    return Ok(());
                }
                return Err(sftp_copy_error(stderr.as_bytes(), &password));
            }
            Ok(None) if cancel.load(std::sync::atomic::Ordering::SeqCst) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = log_reader.join();
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err("SFTP-Kopie fehlgeschlagen".into()),
        }
    }
}

/// Ordnet einen lokalen Pfad einem aktiven rclone-Mount zu (WebDAV, SMB, FTP,
/// FTPS). SFTP bleibt bewusst ausgenommen: Dort überträgt bereits der native
/// OpenSSH-Weg. Objekt-Speicher haben ihren eigenen direkten Zugang.
#[derive(Clone)]
pub struct RcloneTransferContext {
    pub mount_path: PathBuf,
    pub spec: RemoteSpec,
    /// Das beim Einhängen abgelegte, verschleierte Kennwort. Ist es vorhanden,
    /// braucht kein Folgeaufruf den Schlüsselbund.
    pub obscured_password: Option<String>,
}

/// Findet den passenden rclone-Mount zu einem Pfad. Auch ein noch nicht
/// angelegtes Ziel bleibt eindeutig zuordenbar, weil der Pfad ohne
/// Existenzprüfung normalisiert wird.
/// Entscheidet, ob ein eingehängtes Laufwerk über den direkten rclone-Weg
/// bedient werden darf.
///
/// Ausgenommen bleiben zwei Gruppen, die längst einen eigenen, erprobten Weg
/// zum Server haben und hier nicht angetastet werden:
///
/// * **Objekt-Speicher** (S3, Openstack Swift) – geht über
///   [`copy_object_storage`].
/// * **SFTP** – geht über [`upload_to_sftp_mount`] mit dem OpenSSH-Client.
///
/// Übrig bleiben WebDAV, SMB und FTP: genau die Laufwerke, die bisher über den
/// zwischengespeicherten Mount liefen.
fn is_rclone_transfer_candidate(
    object_profile: Option<&ObjectStorageProfile>,
    remote_spec: Option<&RemoteSpec>,
) -> bool {
    object_profile.is_none()
        && remote_spec.is_some_and(|spec| spec.protocol != RemoteProtocol::Sftp)
}

pub fn rclone_transfer_context(path: &Path) -> Option<RcloneTransferContext> {
    let list = registry().lock().ok()?;
    // Ohne ein passendes Laufwerk ist der Pfadabgleich zwecklos.
    if !list.iter().any(|entry| {
        is_rclone_transfer_candidate(entry.object_profile.as_ref(), entry.remote_spec.as_ref())
    }) {
        return None;
    }
    let real_path = canonicalize_with_missing_suffix(path);
    list.iter()
        .filter_map(|entry| {
            let spec = entry.remote_spec.as_ref()?;
            is_rclone_transfer_candidate(entry.object_profile.as_ref(), Some(spec))
                .then_some((entry, spec))
        })
        .filter_map(|(entry, spec)| {
            let mount = canonicalize_with_missing_suffix(&entry.path);
            real_path
                .starts_with(&mount)
                .then_some(RcloneTransferContext {
                    mount_path: entry.path.clone(),
                    spec: spec.clone(),
                    obscured_password: entry.obscured_password.clone(),
                })
        })
        // Bei verschachtelten Mounts gewinnt der spezifischste.
        .max_by_key(|context| context.mount_path.as_os_str().len())
}

/// Baut aus einem lokalen Mount-Pfad das rclone-Ziel, zum Beispiel
/// `DUALBEAM:ordner/datei.txt`. Ein Pfad außerhalb des Mounts wird abgelehnt.
fn rclone_transfer_target(
    path: &Path,
    mount_path: &Path,
    spec: &RemoteSpec,
) -> Result<String, String> {
    let real_path = canonicalize_with_missing_suffix(path);
    let real_mount = canonicalize_with_missing_suffix(mount_path);
    let relative = real_path
        .strip_prefix(&real_mount)
        .map_err(|_| "Ungültiger Netzlaufwerkspfad".to_string())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| "Ungültiger Netzlaufwerkspfad".to_string())?,
            ),
            std::path::Component::CurDir => {}
            _ => return Err("Ungültiger Netzlaufwerkspfad".into()),
        }
    }
    let base = remote_argument(spec);
    if parts.is_empty() {
        return Ok(base);
    }
    Ok(format!(
        "{}/{}",
        base.trim_end_matches('/'),
        parts.join("/")
    ))
}

/// Überträgt zwischen einem rclone-Laufwerk und der lokalen Platte, ohne den
/// eingehängten Pfad zu benutzen.
///
/// Der eingehängte NFS-Pfad führt einen Zwischenspeicher mit
/// `--dir-cache-time`. Wer darüber schreibt oder liest, sieht deshalb je nach
/// Zeitpunkt einen veralteten Stand: Eine eben geschriebene Datei fehlt noch im
/// Verzeichnis, eine direkt gelöschte erscheint weiterhin. Genau daraus
/// entstanden die sprunghaften Fehler („manchmal geht es, manchmal nicht").
/// rclone spricht hier stattdessen unmittelbar mit dem Server – derselbe
/// Grundsatz, nach dem SFTP und Objekt-Speicher bereits arbeiten.
#[allow(clippy::too_many_arguments)]
pub fn copy_rclone_storage(
    context: &RcloneTransferContext,
    source_is_remote: bool,
    source: &Path,
    destination: &Path,
    force_overwrite: bool,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(SftpCopyProgress),
) -> Result<(), String> {
    let spec = &context.spec;
    if spec.protocol == RemoteProtocol::Sftp {
        return Err("err.remote.notOurs".into());
    }
    // Ein aktiver Mount ist der Beleg, dass etwa unsicheres FTP zuvor bewusst
    // bestätigt wurde. Ein Profil allein darf keinen direkten Netzaufruf lösen.
    validate(spec, true)?;
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    // Der Mount wird erneut gegen die aktive Registrierung geprüft. Damit kann
    // ein von außen gelieferter Pfad keinen fremden Zugang ansprechen.
    let expected = spec.descriptor();
    let real_mount = std::fs::canonicalize(&context.mount_path)
        .unwrap_or_else(|_| context.mount_path.clone());
    let verified = registry()
        .lock()
        .ok()
        .map(|list| {
            list.iter().any(|entry| {
                let entry_mount =
                    std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
                entry_mount == real_mount && entry.descriptor == expected
            })
        })
        .unwrap_or(false);
    if !verified {
        return Err("err.remote.notOurs".into());
    }

    let remote_side = if source_is_remote { source } else { destination };
    let remote_path = rclone_transfer_target(remote_side, &context.mount_path, spec)?;
    let local_side = if source_is_remote { destination } else { source };

    // Die Art des Quellobjekts entscheidet über `copy` (Ordner) oder `copyto`
    // (Einzeldatei). Bei einer entfernten Quelle liefert der Mount die Auskunft
    // zuverlässig, weil Lesen aus dem Zwischenspeicher unkritisch ist.
    let source_is_dir = std::fs::symlink_metadata(source)
        .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        .unwrap_or(false);

    // Wie beim Lesen: Das beim Einhängen abgelegte Kennwort hat Vorrang, damit
    // ein fehlender Schlüsselbund-Eintrag die Übertragung nicht verhindert.
    let password = match context.obscured_password.as_deref() {
        Some(value) if !value.is_empty() => String::new(),
        _ => remote_password(spec)?.ok_or_else(|| "err.remote.emptyPassword".to_string())?,
    };
    let rclone = rclone_executable()?;
    let obscured = match context.obscured_password.as_deref() {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => obscure(&rclone, &password)?,
    };
    let (from, to) = if source_is_remote {
        (remote_path, local_side.to_string_lossy().into_owned())
    } else {
        (local_side.to_string_lossy().into_owned(), remote_path)
    };

    let mut command = Command::new(&rclone);
    command
        .arg(if source_is_dir { "copy" } else { "copyto" })
        .args([
            "--transfers",
            "4",
            "--checkers",
            "1",
            "--contimeout",
            "15s",
            "--timeout",
            "2m",
            "--retries",
            "3",
            // Mehrere Aktionen kurz hintereinander lassen Anbieter wie pCloud
            // drosseln. Mit Wartezeit zwischen den Versuchen läuft der Vorgang
            // durch, statt mit einem Verbindungsfehler abzubrechen.
            "--retries-sleep",
            "3s",
            "--low-level-retries",
            "10",
            "--no-update-dir-modtime",
            // Etwa zweimal je Sekunde eine einzeilige Statistik: So bleibt die
            // Fortschrittsanzeige auch bei einer großen Einzeldatei aktiv.
            "--stats",
            "500ms",
            "--stats-one-line",
            "--stats-log-level",
            "NOTICE",
        ])
        .arg(&from)
        .arg(&to)
        // INFO-Zeilen enthalten je bestätigter Datei „Copied (…)". Damit zählt
        // die Oberfläche schon während eines großen Ordners mit.
        .arg("-v")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Eine bewusste Kopie darf bei bestehendem Ziel nicht wegen gleicher
    // Zeitstempel ausgelassen werden.
    if force_overwrite {
        command.arg("--ignore-times");
    }
    for (key, value) in rclone_env(spec, &obscured, None) {
        command.env(key, value);
    }

    let mut child = command
        .spawn()
        .map_err(|_| "Übertragung konnte nicht gestartet werden".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Übertragung konnte nicht gestartet werden".to_string())?;
    let (log_tx, log_rx) = mpsc::channel::<String>();
    let log_reader = std::thread::spawn(move || {
        let mut collected = Vec::new();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let _ = log_tx.send(line.clone());
            collected.push(line);
        }
        collected.join("\n")
    });
    let mut last_percent = 0_u8;
    progress(SftpCopyProgress::Percent(0));
    loop {
        while let Ok(line) = log_rx.try_recv() {
            report_sftp_copy_log_line(progress, &mut last_percent, &line);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr = log_reader.join().unwrap_or_default();
                while let Ok(line) = log_rx.try_recv() {
                    report_sftp_copy_log_line(progress, &mut last_percent, &line);
                }
                if status.success() {
                    progress(SftpCopyProgress::Percent(100));
                    // Der Mount zeigt sonst bis zum Ablauf von `dir-cache-time`
                    // den alten Stand. Ohne dieses Verwerfen wäre eine eben
                    // übertragene Datei im Fenster noch nicht zu sehen.
                    if !source_is_remote {
                        let touched = [destination.to_path_buf()];
                        refresh_mount_after_direct_delete(&context.mount_path, &touched);
                    }
                    return Ok(());
                }
                return Err(sftp_client_error(stderr.as_bytes(), &password));
            }
            Ok(None) if cancel.load(std::sync::atomic::Ordering::SeqCst) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = log_reader.join();
                return Ok(());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err("Übertragung fehlgeschlagen".into()),
        }
    }
}

/// Hält fest, dass das direkte Lesen nicht geklappt hat und deshalb der
/// eingehängte Pfad gelesen wird. Die Meldung landet nur im Protokoll – für
/// die Bedienung ist der Rückfall unauffällig.
fn log_direct_listing_fallback(art: &str, path: &Path, reason: &str) {
    eprintln!(
        "[dualbeam] direktes Lesen ({art}) fehlgeschlagen fuer {}: {reason} – lese vom eingehaengten Pfad",
        path.display()
    );
}

/// Setzt einen rclone-Befehl unmittelbar gegen den Server ab, ohne den
/// eingehängten Pfad zu berühren.
///
/// Gegenstück zu `direct_object_storage_command`, nur für die Laufwerke mit
/// [`RemoteSpec`] statt einem Objekt-Speicher-Profil.
fn direct_rclone_command(
    spec: &RemoteSpec,
    obscured_password: Option<&str>,
    args: &[String],
) -> Result<std::process::Output, String> {
    let rclone = rclone_executable()?;
    // Bevorzugt das beim Einhängen abgelegte Kennwort. Der Schlüsselbund ist
    // nur der Notnagel: Wer beim Verbinden „nicht speichern" gewählt hat, hat
    // dort keinen Eintrag – der Mount läuft trotzdem, weil rclone das Kennwort
    // beim Start bekam. Ohne diese Ablage wäre jeder Folgeaufruf wertlos.
    let obscured = match obscured_password {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => {
            let password =
                remote_password(spec)?.ok_or_else(|| "err.remote.emptyPassword".to_string())?;
            obscure(&rclone, &password)?
        }
    };
    let mut command = Command::new(&rclone);
    command.args(args).stdin(Stdio::null());
    for (key, value) in rclone_env(spec, &obscured, None) {
        command.env(key, value);
    }
    command
        .output()
        .map_err(|_| "Das Netzlaufwerk konnte nicht abgefragt werden".to_string())
}

/// Liest ein Verzeichnis eines rclone-Laufwerks direkt vom Server.
///
/// Liefert `None`, wenn der Pfad zu keinem solchen Laufwerk gehört – dann
/// bleibt es beim gewöhnlichen Lesen von der Platte.
///
/// Warum nicht einfach den eingehängten Pfad auflisten: Der Mount führt einen
/// Verzeichnis-Zwischenspeicher. Wird daneben unmittelbar am Server gearbeitet
/// – und genau das tun Löschen, Kopieren und Abgleich –, zeigt er weiterhin
/// den alten Stand. Gemessen blieben sechs gelöschte Dateien auch nach zwei
/// Minuten sichtbar. Objekt-Speicher liest aus demselben Grund seit jeher
/// unmittelbar.
fn list_rclone_dir_direct(
    path: &Path,
    context: &RcloneTransferContext,
) -> Result<Vec<ObjectStorageEntry>, String> {
    (|| -> Result<Vec<ObjectStorageEntry>, String> {
        let target = rclone_transfer_target(path, &context.mount_path, &context.spec)?;
        let real_path = canonicalize_with_missing_suffix(path);
        let output = direct_rclone_command(
            &context.spec,
            context.obscured_password.as_deref(),
            &[
                "lsjson".to_string(),
                "--no-mimetype".to_string(),
                "--contimeout".to_string(),
                "15s".to_string(),
                "--timeout".to_string(),
                "2m".to_string(),
                "--retries".to_string(),
                "2".to_string(),
                target,
            ],
        )?;
        if !output.status.success() {
            let password = remote_password(&context.spec)
                .ok()
                .flatten()
                .unwrap_or_default();
            return Err(sftp_client_error(&output.stderr, &password));
        }
        let entries = parse_rclone_list_entries(
            &output.stdout,
            "Die Antwort des Netzlaufwerks konnte nicht gelesen werden",
        )?;
        Ok(entries
            .into_iter()
            .map(|entry| ObjectStorageEntry {
                path: real_path.join(&entry.name),
                is_dir: entry.is_dir,
                size: rclone_list_size(entry.size),
                mtime: rclone_list_mtime(entry.modified.as_deref()),
                name: entry.name,
            })
            .collect())
    })()
}

pub fn list_rclone_dir(path: &Path) -> Option<Result<Vec<ObjectStorageEntry>, String>> {
    let context = rclone_transfer_context(path)?;
    let result = list_rclone_dir_direct(path, &context);
    // Scheitert der direkte Weg – etwa weil der Zugang gerade nicht antwortet
    // oder das Kennwort nicht abrufbar ist –, wird der Fehler NICHT
    // durchgereicht. Sonst bliebe die Anzeige leer und das Laufwerk wirkte
    // defekt, obwohl es eingehängt ist. Stattdessen wird auf das Lesen vom
    // eingehängten Pfad zurückgefallen: schlimmstenfalls ein etwas älterer
    // Stand, aber nie eine kaputte Ansicht.
    match result {
        Ok(entries) => Some(Ok(entries)),
        Err(reason) => {
            log_direct_listing_fallback("Verzeichnis", path, &reason);
            None
        }
    }
}

/// Rekursives Listing für die Vorschau des Abgleichs, ebenfalls unmittelbar
/// vom Server.
pub fn list_rclone_tree(path: &Path) -> Option<Result<Vec<ObjectStorageEntry>, String>> {
    let context = rclone_transfer_context(path)?;
    let result = (|| -> Result<Vec<ObjectStorageEntry>, String> {
        let target = rclone_transfer_target(path, &context.mount_path, &context.spec)?;
        let real_path = canonicalize_with_missing_suffix(path);
        let output = direct_rclone_command(
            &context.spec,
            context.obscured_password.as_deref(),
            &[
                "lsjson".to_string(),
                "--recursive".to_string(),
                "--no-mimetype".to_string(),
                "--contimeout".to_string(),
                "15s".to_string(),
                "--timeout".to_string(),
                "5m".to_string(),
                "--retries".to_string(),
                "2".to_string(),
                target,
            ],
        )?;
        if !output.status.success() {
            let password = remote_password(&context.spec)
                .ok()
                .flatten()
                .unwrap_or_default();
            return Err(sftp_client_error(&output.stderr, &password));
        }
        let entries = parse_rclone_list_entries(
            &output.stdout,
            "Die Antwort des Netzlaufwerks konnte nicht gelesen werden",
        )?;
        Ok(entries
            .into_iter()
            .map(|entry| {
                // Bei `--recursive` ist `Path` der Pfad unterhalb des Ordners.
                let relative = if entry.path.is_empty() {
                    entry.name.clone()
                } else {
                    entry.path.clone()
                };
                ObjectStorageEntry {
                    path: real_path.join(&relative),
                    is_dir: entry.is_dir,
                    size: rclone_list_size(entry.size),
                    mtime: rclone_list_mtime(entry.modified.as_deref()),
                    name: relative,
                }
            })
            .collect())
    })();
    // Anders als bei der Pane-Anzeige ist ein lokaler Fallback hier falsch:
    // Die Sync-Vorschau muss den aktuellen Serverstand zeigen oder mit dem
    // Serverfehler abbrechen. Ein NFS-Cache könnte gelöschte Dateien sonst
    // noch als vorhanden melden.
    Some(result)
}

/// Prüft einen Namen unmittelbar im Parent-Listing des rclone-Servers.
///
/// Der eingehängte NFS-Pfad darf hierfür nicht verwendet werden: Nach einem
/// Löschen oder einer Änderung am Server kann dessen Verzeichnis-Cache noch
/// den vorigen Stand liefern. `lsjson --stat` ist für nicht vorhandene
/// Verzeichnisse bei einigen Backends ebenfalls nicht eindeutig, deshalb wird
/// wie beim Objekt-Speicher das Parent-Listing verwendet.
pub fn rclone_path_exists(path: &Path) -> Option<Result<bool, String>> {
    let context = rclone_transfer_context(path)?;
    let real_path = canonicalize_with_missing_suffix(path);
    let real_mount = canonicalize_with_missing_suffix(&context.mount_path);
    if real_path == real_mount {
        return Some(Ok(true));
    }
    let Some(name) = real_path.file_name().map(|name| name.to_os_string())
    else {
        return Some(Ok(true));
    };
    let parent = real_path.parent().unwrap_or(&context.mount_path);
    Some(list_rclone_dir_direct(parent, &context).map(|entries| {
        entries
            .iter()
            .any(|entry| entry.name == name.to_string_lossy())
    }))
}

/// Extrahiert die Prozentzahl aus rclone `--stats-one-line`, zum Beispiel
/// `12.3 MiB / 45.6 MiB, 27%, 1.0 MiB/s`. Fehlermeldungen haben kein solches
/// Suffix und werden bewusst ignoriert.
fn rclone_progress_percent(line: &str) -> Option<u8> {
    let prefix = line.rsplit_once('%')?.0;
    let start = prefix
        .rfind(|ch: char| !ch.is_ascii_digit())
        .map(|index| index + 1)
        .unwrap_or(0);
    let value = prefix[start..].trim().parse::<u8>().ok()?;
    (value <= 100).then_some(value)
}

/// `rclone -v` meldet eine bestätigte Datei als
/// `INFO : relativer/pfad: Copied (new)`. Die genaue Klammerbemerkung ist
/// backendabhängig, der stabile Teil ist `: Copied (`.
fn rclone_copied_path(line: &str) -> Option<&str> {
    let prefix = line.trim().split_once(": Copied (")?.0;
    let (_, path) = prefix.rsplit_once(": ")?;
    (!path.is_empty()).then_some(path)
}

/// Verarbeitet eine rclone-Protokollzeile für den sichtbaren SFTP-Job.
/// Die Prozentanzeige bleibt bis zum erfolgreichen Prozessende bei höchstens
/// 99 %, während jede rclone-bestätigte Datei sofort gezählt wird.
fn report_sftp_copy_log_line(
    progress: &mut dyn FnMut(SftpCopyProgress),
    last_percent: &mut u8,
    line: &str,
) {
    if let Some(percent) = rclone_progress_percent(line) {
        if percent > *last_percent {
            *last_percent = percent;
            progress(SftpCopyProgress::Percent(percent.min(99)));
        }
    }
    if let Some(path) = rclone_copied_path(line) {
        progress(SftpCopyProgress::FileCopied(path.to_string()));
    }
}

#[cfg(test)]
fn sftp_transfer_target(
    path: &Path,
    mount_path: &Path,
    spec: &RemoteSpec,
) -> Result<String, String> {
    let real_path = canonicalize_with_missing_suffix(path);
    // Auf macOS können sowohl der technische Mount als auch ein Zielpfad über
    // einen Alias wie `/tmp` bzw. `/private/tmp` dargestellt sein. Beide
    // Seiten normalisieren, sonst würde `strip_prefix` einen gültigen Pfad
    // fälschlich als fremdes Ziel ablehnen.
    let real_mount = canonicalize_with_missing_suffix(mount_path);
    let relative = real_path
        .strip_prefix(&real_mount)
        .map_err(|_| "Ungültiger SFTP-Zielpfad".to_string())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| "Ungültiger SFTP-Zielpfad".to_string())?,
            ),
            std::path::Component::CurDir => {}
            _ => return Err("Ungültiger SFTP-Zielpfad".into()),
        }
    }
    if parts.is_empty() {
        return Err("Das Stammverzeichnis des SFTP-Laufwerks kann nicht kopiert werden".into());
    }
    Ok(format!(
        "{}/{}",
        remote_argument(spec).trim_end_matches('/'),
        parts.join("/")
    ))
}

#[cfg(test)]
#[allow(dead_code)]
fn sftp_copy_error(stderr: &[u8], password: &str) -> String {
    let detail = String::from_utf8_lossy(stderr)
        .replace(password, "[geschützt]")
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(600)
        .collect::<String>();
    if detail.is_empty() {
        "SFTP-Kopie fehlgeschlagen".into()
    } else {
        format!("SFTP-Kopie fehlgeschlagen: {detail}")
    }
}

/// Vergisst nach einem direkten SFTP-Upload ausschließlich die betroffenen
/// VFS-Verzeichnisse. Anders als `refresh_mount_after_direct_delete` wird
/// hier bewusst kein `read_dir` ausgeführt: Der NFS-Mount könnte dabei einen
/// langsamen Server-Refresh blockieren. Die anschließende Pane-Aktualisierung
/// liest die Liste dann unmittelbar und ohne veralteten Cache.
#[cfg(test)]
#[allow(dead_code)]
pub fn refresh_sftp_mount_after_direct_change(
    spec: &RemoteSpec,
    mount_path: &Path,
    paths: &[PathBuf],
) {
    if spec.protocol != RemoteProtocol::Sftp || paths.is_empty() {
        return;
    }
    let expected = spec.descriptor();
    let real_mount = canonicalize_with_missing_suffix(mount_path);
    let rc_socket = registry().lock().ok().and_then(|list| {
        list.iter().find_map(|entry| {
            let entry_mount = canonicalize_with_missing_suffix(&entry.path);
            (entry.descriptor == expected && entry_mount == real_mount)
                .then(|| entry.rc_socket.clone())
        })
    });
    let (Some(socket), Ok(rclone)) = (rc_socket, rclone_executable()) else {
        return;
    };
    // Ein Aufruf ohne `dir` leert den kompletten VFS-Cache dieses einen
    // SFTP-Mounts. Die vorherige Variante startete für jedes ausgewählte
    // Objekt einen eigenen rclone-RC-Prozess; bei vielen Dateien verlängerte
    // gerade dieser Nachlauf das Löschen spürbar. Der nachfolgende Pane-
    // Refresh liest ausschließlich die gerade sichtbare Ebene wieder ein.
    let mut command = Command::new(&rclone);
    command
        .args(["rc", "--unix-socket"])
        .arg(&socket)
        .arg("--no-output")
        .arg("vfs/forget")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = command.status();
}

/// Löscht ausgewählte Elemente eines SFTP-, FTP- oder FTPS-Mounts direkt mit
/// rclone. Der bisherige Weg über NFS musste jede Datei nacheinander durch den
/// macOS-Client entfernen. `rclone purge` kann dagegen die Löschvorgänge
/// parallelisieren und nutzt, falls ein Server es anbietet, dessen eigenen
/// schnellen Löschmechanismus.
pub fn purge_remote_storage(
    spec: &RemoteSpec,
    mount_path: &Path,
    paths: &[PathBuf],
    cancel: &std::sync::atomic::AtomicBool,
    mut progress: Option<&mut dyn FnMut(&str)>,
    mut on_removed: impl FnMut(&Path),
) -> Result<(), String> {
    if spec.protocol == RemoteProtocol::Sftp {
        return Err("SFTP-Löschvorgänge laufen über das eingehängte SSHFS-Dateisystem".into());
    }
    // Ein aktiver Mount ist der Beleg, dass etwa unsicheres FTP zuvor bewusst
    // bestätigt wurde. Ein Profil allein darf keinen direkten Netzaufruf
    // auslösen.
    validate(spec, true)?;
    if paths.is_empty() || cancel.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    let expected = spec.descriptor();
    let mounted = registry()
        .lock()
        .ok()
        .map(|list| {
            list.iter()
                .any(|entry| entry.path == mount_path && entry.descriptor == expected)
        })
        .unwrap_or(false);
    if !mounted {
        return Err("err.remote.notOurs".into());
    }
    let password = remote_password(spec)?.ok_or_else(|| "err.remote.emptyPassword".to_string())?;
    let rclone = rclone_executable()?;
    let known_hosts = if spec.protocol == RemoteProtocol::Sftp {
        let port = spec.port_or_default();
        if !is_trusted(&spec.host, port)? {
            return Err("err.remote.hostKeyUnknown".into());
        }
        Some(known_hosts_file()?)
    } else {
        None
    };
    let obscured = obscure(&rclone, &password)?;
    let base = remote_argument(spec);
    let mut targets: Vec<(PathBuf, String, bool)> = Vec::new();
    for path in paths {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        let relative = path
            .strip_prefix(mount_path)
            .map_err(|_| "Ungültiger Netzlaufwerkspfad".to_string())?;
        let mut parts = Vec::new();
        for component in relative.components() {
            match component {
                std::path::Component::Normal(part) => parts.push(
                    part.to_str()
                        .ok_or_else(|| "Ungültiger Netzlaufwerkspfad".to_string())?,
                ),
                std::path::Component::CurDir => {}
                _ => return Err("Ungültiger Netzlaufwerkspfad".into()),
            }
        }
        // Die eingehängte Wurzel entspricht dem konfigurierten Serverpfad und
        // darf nie als Ganzes entfernt werden.
        if parts.is_empty() {
            return Err("Das Stammverzeichnis des Netzlaufwerks kann nicht gelöscht werden".into());
        }
        let target = format!("{}/{}", base.trim_end_matches('/'), parts.join("/"));
        let is_dir = std::fs::symlink_metadata(path)
            .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
            .unwrap_or(false);
        if targets
            .iter()
            .any(|(selected, _, selected_is_dir)| *selected_is_dir && path.starts_with(selected))
        {
            continue;
        }
        if is_dir {
            targets.retain(|(selected, _, _)| !selected.starts_with(path));
        }
        targets.push((path.clone(), target, is_dir));
    }
    for (path, target, is_dir) in targets {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        // Nur der neue direkte SFTP-Pfad liest Verbose-Meldungen, um die
        // erfolgreich gelöschten Objekte live in der Statusleiste zu zählen.
        // FTP und FTPS behalten bytegenau ihren bisherigen Prozessaufruf.
        let report_progress = spec.protocol == RemoteProtocol::Sftp && progress.is_some();
        let environment = rclone_env(spec, &obscured, known_hosts.as_deref());
        let mut command = Command::new(&rclone);
        command
            .arg(if is_dir { "purge" } else { "deletefile" })
            .args([
                "--checkers",
                "32",
                "--contimeout",
                "15s",
                "--timeout",
                "2m",
                "--retries",
                "3",
                // Anbieter wie pCloud drosseln mehrere Zugriffe kurz
                // hintereinander. Ohne Pause zwischen den Versuchen laeuft die
                // Wiederholung in dieselbe Sperre und der Vorgang scheitert,
                // obwohl der Zugang in Ordnung ist.
                "--retries-sleep",
                "3s",
                "--low-level-retries",
                "10",
            ])
            .arg(&target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Die Fehlerausgabe wird bei jedem Protokoll mitgelesen. Wurde sie
            // verworfen, blieb im Fehlerfall nur eine pauschale Sammelmeldung
            // uebrig, die den eigentlichen Grund verschwieg.
            .stderr(Stdio::piped());
        if report_progress {
            command.arg("-v");
        }
        for (key, value) in &environment {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "err.remote.mountFailed".to_string())?;
        let (log_tx, log_rx) = mpsc::channel::<String>();
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "err.remote.mountFailed".to_string())?;
        let log_reader = Some(std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let _ = log_tx.send(line);
            }
        }));
        let mut captured: Vec<String> = Vec::new();
        loop {
            while let Ok(line) = log_rx.try_recv() {
                if report_progress {
                    if let Some(path) = rclone_deleted_path(&line) {
                        if let Some(callback) = progress.as_deref_mut() {
                            callback(path);
                        }
                    }
                }
                captured.push(line);
            }
            match child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    if let Some(reader) = log_reader {
                        let _ = reader.join();
                    }
                    // Nur der erfolgreiche Prozess bestätigt diesen Pfad;
                    // spätere Teilfehler oder Abbrüche ändern daran nichts.
                    on_removed(&path);
                    // Der Reader kann unmittelbar vor Prozessende noch
                    // Meldungen in die Queue geschrieben haben.
                    while let Ok(line) = log_rx.try_recv() {
                        if report_progress {
                            if let Some(path) = rclone_deleted_path(&line) {
                                if let Some(callback) = progress.as_deref_mut() {
                                    callback(path);
                                }
                            }
                        }
                        captured.push(line);
                    }
                    break;
                }
                Ok(Some(_)) => {
                    if let Some(reader) = log_reader {
                        let _ = reader.join();
                    }
                    while let Ok(line) = log_rx.try_recv() {
                        captured.push(line);
                    }
                    // rclone endet auch dann mit einem Fehlercode, wenn das
                    // Objekt bereits entfernt wurde und erst ein nachgelagerter
                    // Schritt scheitert. Als Erfolg gilt der Vorgang aber nur,
                    // wenn der Elternordner lesbar ist und den Namen nicht mehr
                    // enthaelt. Laesst er sich nicht lesen, ist der Ausgang
                    // unbekannt - und Unbekanntes ist hier ein Fehlschlag.
                    if remote_entry_absent(&rclone, &target, &environment) == Some(true) {
                        on_removed(&path);
                        break;
                    }
                    record_delete_failure(&target, &captured);
                    return Err(rclone_failure_message(&captured.join("\n")));
                }
                Ok(None) if cancel.load(std::sync::atomic::Ordering::SeqCst) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(reader) = log_reader {
                        let _ = reader.join();
                    }
                    return Ok(());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return Err("err.remote.mountFailed".into()),
            }
        }
    }
    Ok(())
}

/// Prueft ueber den Elternordner, ob ein Eintrag wirklich verschwunden ist.
///
/// Ein Fehlertext taugt nicht als Beleg: Meldet der Server "not found", kann
/// genauso gut der Elternpfad falsch sein - dann waere nie geloescht worden.
/// Deshalb zaehlt nur ein positiver Nachweis: Die Auflistung des Elternordners
/// muss gelingen und der Name darin fehlen. Gelingt die Auflistung nicht, ist
/// der Ausgang unbekannt (`None`) und gilt als Fehlschlag.
fn remote_entry_absent(
    rclone: &Path,
    target: &str,
    environment: &[(String, String)],
) -> Option<bool> {
    let (parent, name) = split_remote_target(target)?;
    let mut command = Command::new(rclone);
    command
        .args([
            "lsf",
            "--contimeout",
            "15s",
            "--timeout",
            "30s",
            "--retries",
            "1",
        ])
        .arg(parent)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in environment {
        command.env(key, value);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    Some(
        !listing
            .lines()
            .any(|line| line.trim_end().trim_end_matches('/') == name),
    )
}

/// Trennt ein rclone-Ziel in Elternpfad und Namen. Getrennt wird am letzten
/// Schraegstrich hinter dem Doppelpunkt; fehlt er, ist die Wurzel des Zugangs
/// der Elternpfad.
fn split_remote_target(target: &str) -> Option<(&str, &str)> {
    let colon = target.find(':')?;
    let rest = target.get(colon + 1..)?;
    let (parent, name) = match rest.rfind('/') {
        Some(offset) => {
            let cut = colon + 1 + offset;
            (target.get(..cut)?, target.get(cut + 1..)?)
        }
        None => (target.get(..=colon)?, rest),
    };
    if name.is_empty() {
        return None;
    }
    // `DUALBEAM:/datei` hinterlaesst als Elternpfad `DUALBEAM:/`. Der leere
    // Schrittname wuerde rclone am Adresspfad vorbeigreifen lassen.
    let parent = parent.trim_end_matches('/');
    (!parent.is_empty()).then_some((parent, name))
}

/// Schreibt den Grund eines gescheiterten Loeschversuchs mit. Ohne diese Spur
/// bleibt nur eine uebersetzte Sammelmeldung uebrig, aus der sich die Ursache
/// nicht mehr rekonstruieren laesst.
fn record_delete_failure(target: &str, captured: &[String]) {
    let Ok(root) = mount_root() else { return };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("remote-delete.log"))
    else {
        return;
    };
    use std::io::Write as _;
    let _ = writeln!(file, "--- {target}");
    for line in captured {
        let _ = writeln!(file, "{line}");
    }
}

/// Meldungen des rclone-Verbose-Logs haben die Form
/// `INFO : relativer/pfad: Deleted`. Verzeichnisentfernungen zählen hier
/// nicht: Die sichtbare Zahl soll die tatsächlich serverseitig bestätigten
/// Dateilöschungen ausdrücken.
fn rclone_deleted_path(line: &str) -> Option<&str> {
    let prefix = line.trim().strip_suffix(": Deleted")?;
    let (_, path) = prefix.rsplit_once(": ")?;
    (!path.is_empty()).then_some(path)
}

/// Der NFS-Mount verwaltet sein Verzeichnis-Listing für `dir-cache-time`.
/// Direkte rclone-Löschungen erfolgen jedoch außerhalb dieses Prozesses. Nach
/// einem kurzen Ablauf des bewusst kleinen Caches lesen wir die Elternordner
/// einmal ein, damit der anschließende Pane-Refresh nicht mehr die alte Liste
/// aus dem Mount-Cache erhält.
pub fn refresh_mount_after_direct_delete(mount_path: &Path, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let rc_socket = registry().lock().ok().and_then(|list| {
        list.iter()
            .find(|entry| entry.path == mount_path)
            .map(|entry| entry.rc_socket.clone())
    });
    let mut parents: Vec<PathBuf> = paths
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    parents.sort();
    parents.dedup();
    for parent in parents {
        if let (Some(socket), Ok(rclone)) = (&rc_socket, rclone_executable()) {
            let relative = parent.strip_prefix(mount_path).ok();
            let mut command = Command::new(rclone);
            command
                .args(["rc", "--unix-socket"])
                .arg(socket)
                .arg("--no-output")
                .arg("vfs/forget")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(relative) = relative.filter(|path| !path.as_os_str().is_empty()) {
                command.arg(format!("dir={}", relative.to_string_lossy()));
            }
            let _ = command.status();
        }
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries {
                let _ = entry;
            }
        }
    }
}

/// Übersetzt die Protokollausgabe von rclone in eine der bekannten Kennungen.
/// Der Rohtext ist englisch und technisch; er hilft dem Benutzer nicht weiter.
fn mount_failure_message(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    rclone_failure_message(&text)
}

/// SSHFS verwendet die bekannten OpenSSH-Fehlermeldungen; die Zuordnung hält
/// die Oberfläche dennoch bei denselben verständlichen Verbindungsfehlern wie
/// die übrigen Netzwerkprofile.
fn sshfs_failure_message(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lower = text.to_lowercase();
    if lower.contains("no ") && lower.contains("host key is known") {
        return "err.remote.hostKeyUnknown".into();
    }
    if lower.contains("remote host identification has changed") {
        return "err.remote.hostKeyChanged".into();
    }
    if lower.contains("host key verification failed") {
        return "err.remote.hostKeyUnknown".into();
    }
    if lower.contains("permission denied") || lower.contains("authentication failed") {
        return "err.remote.auth".into();
    }
    if lower.contains("no such file") || lower.contains("not a directory") {
        return "err.remote.path".into();
    }
    if lower.contains("could not resolve hostname")
        || lower.contains("connection refused")
        || lower.contains("operation timed out")
        || lower.contains("connection timed out")
    {
        return "err.remote.unreachable".into();
    }
    "err.remote.mountFailed".into()
}

/// Ordnet technische rclone-Fehler einer für die Oberfläche verwendbaren
/// Kennung zu. Die eigentlichen Zugangsdaten dürfen dabei nie im Fehlertext
/// nach außen gelangen.
fn rclone_failure_message(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("signaturedoesnotmatch") {
        return "err.objectStorage.signature".into();
    }
    if lower.contains("knownhosts") || lower.contains("key mismatch") {
        return "err.remote.hostKeyChanged".into();
    }
    if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("logon failure")
        || lower.contains("logon_failure")
        || lower.contains("invalid credentials")
        || (lower.contains("auth")
            && (lower.contains("fail") || lower.contains("reject") || lower.contains("denied")))
    {
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

/// Prüft die Anmeldung, bevor ein S3-Bucket oder Swift-Container als NFS-Laufwerk
/// sichtbar wird. `lsd` löst nur eine Metadatenabfrage aus und verändert weder
/// Dateien noch Container.
fn verify_object_storage_connection(
    rclone: &Path,
    argument: &str,
    env: &[(String, String)],
) -> Result<(), String> {
    let mut command = Command::new(rclone);
    command
        .args([
            "lsd",
            "--contimeout",
            "10s",
            "--timeout",
            "20s",
            "--retries",
            "1",
        ])
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|_| "err.remote.mountFailed".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let details = String::from_utf8_lossy(&output.stderr);
    Err(rclone_failure_message(&details))
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
    let real_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Die Registrierung ist die zuverlässigste Quelle für eigene Mounts. Der
    // generische Pfadvergleich darunter bleibt als Fallback für den Fall, dass
    // ein Eintrag unmittelbar nach dem Aushängen geprüft wird.
    if let Ok(list) = registry().lock() {
        if list.iter().any(|entry| {
            let mount_path =
                std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
            real_path.starts_with(mount_path)
        }) {
            return true;
        }
    }
    let Ok(root) = mount_root() else {
        return false;
    };
    let real_root = std::fs::canonicalize(&root).unwrap_or(root);
    real_path.starts_with(&real_root) && real_path != real_root
}

/// S3 und Swift sind Objekt-Speicher. Ihre „Ordner“ sind in der Regel nur
/// Präfixe und besitzen daher keinen echten Änderungszeitpunkt. Diese engere
/// Erkennung wird von der Dateiansicht genutzt, um rclones Platzhalterdatum
/// nicht als reales Ordnerdatum anzuzeigen.
pub fn is_object_storage_mount(path: &Path) -> bool {
    object_storage_mount_context(path).is_some()
}

struct ObjectStorageMountContext {
    descriptor: String,
    mount_path: PathBuf,
    real_path: PathBuf,
    profile: ObjectStorageProfile,
}

/// Beschreibt den direkten rclone-Transfer für genau eine Objekt-Speicher-
/// Seite. Die Zuordnung passiert absichtlich im Backend: Die Oberfläche kann
/// virtuelle Pfade nach einem Refresh oder einem Symlink anders darstellen,
/// während nur die aktive Mount-Registrierung sicher weiß, zu welchem Profil
/// ein Pfad gehört.
#[derive(Debug, Clone)]
pub struct ObjectStorageTransferContext {
    pub profile: ObjectStorageProfile,
    pub mount_path: PathBuf,
    pub source_is_object_storage: bool,
}

/// Erkennt eine Kopie zwischen einem lokalen bzw. normalen Netzpfad und einem
/// aktiven S3-/Swift-Mount. Kopien zwischen zwei unterschiedlichen
/// Objekt-Speichern werden hier bewusst nicht als Ein-Profil-Transfer
/// behandelt; dafür braucht rclone zwei getrennte, kurzlebige Profile.
pub fn object_storage_transfer_context(
    source: &Path,
    destination: &Path,
) -> Option<ObjectStorageTransferContext> {
    match (
        object_storage_mount_context(source),
        object_storage_mount_context(destination),
    ) {
        (Some(source), None) => Some(ObjectStorageTransferContext {
            profile: source.profile,
            mount_path: source.mount_path,
            source_is_object_storage: true,
        }),
        (None, Some(destination)) => Some(ObjectStorageTransferContext {
            profile: destination.profile,
            mount_path: destination.mount_path,
            source_is_object_storage: false,
        }),
        _ => None,
    }
}

/// Erkennt einen direkten Löschauftrag nur dann, wenn sämtliche ausgewählten
/// Pfade zu genau demselben aktiven S3-/Swift-Profil gehören. Die Oberfläche
/// darf diese Information zwar ebenfalls liefern, aber der Backend-Nachweis
/// ist maßgeblich: virtuelle Pfade können nach einem Refresh anders
/// normalisiert sein.
pub fn object_storage_delete_context(paths: &[PathBuf]) -> Option<ObjectStorageTransferContext> {
    let contexts: Option<Vec<_>> = paths
        .iter()
        .map(|path| object_storage_mount_context(path))
        .collect();
    let contexts = contexts?;
    let first = contexts.first()?;
    contexts
        .iter()
        .all(|context| {
            context.descriptor == first.descriptor && context.mount_path == first.mount_path
        })
        .then(|| ObjectStorageTransferContext {
            profile: first.profile.clone(),
            mount_path: first.mount_path.clone(),
            source_is_object_storage: false,
        })
}

/// Liefert für einen virtuellen S3-/Swift-Pfad die sichtbare
/// Laufwerkswurzel. Die UI darf oberhalb dieser Wurzel nicht über die
/// Systemordner navigieren, weil diese nur die interne Pfadkennung des
/// Objekt-Speichers enthalten.
pub fn object_storage_mount_root(path: &Path) -> Option<PathBuf> {
    let context = object_storage_mount_context(path)?;
    let Ok(list) = registry().lock() else {
        return Some(context.mount_path);
    };
    list.iter()
        .find(|entry| {
            let entry_mount =
                std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
            entry.object_profile.is_some()
                && entry_mount == context.mount_path
                && entry.object_home.as_ref().is_some_and(|home| {
                    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.clone());
                    context.real_path.starts_with(home)
                })
        })
        .and_then(|entry| entry.object_home.clone())
        .or(Some(context.mount_path))
}

#[derive(Clone)]
struct SftpMountContext {
    mount_path: PathBuf,
    spec: RemoteSpec,
}

/// Ordnet einen lokalen SSHFS-Pfad genau dem beim Einhängen geprüften
/// SFTP-Profil zu. Auch ein noch nicht angelegtes Ziel bleibt dadurch
/// eindeutig erkennbar.
fn sftp_mount_context(path: &Path) -> Option<SftpMountContext> {
    let Ok(list) = registry().lock() else {
        return None;
    };
    // Wie oben: ohne aktiven SFTP-Mount ist der teure Pfadabgleich zwecklos.
    if !list.iter().any(|entry| {
        entry
            .remote_spec
            .as_ref()
            .is_some_and(|spec| spec.protocol == RemoteProtocol::Sftp)
    }) {
        return None;
    }
    let real_path = canonicalize_with_missing_suffix(path);
    list.iter()
        .filter_map(|entry| {
            let spec = entry.remote_spec.as_ref()?;
            (spec.protocol == RemoteProtocol::Sftp).then_some((entry, spec))
        })
        .filter_map(|(entry, spec)| {
            let mount = canonicalize_with_missing_suffix(&entry.path);
            real_path.starts_with(&mount).then_some(SftpMountContext {
                mount_path: entry.path.clone(),
                spec: spec.clone(),
            })
        })
        .max_by_key(|context| context.mount_path.as_os_str().len())
}

/// Die sichtbare Wurzel eines aktiven SFTP-Mounts. Der SSHFS-Mount liegt in
/// einem App-Support-Verzeichnis; dessen Eltern dürfen in der Dateiansicht
/// niemals per „Nach oben“ erreichbar sein.
pub fn sftp_mount_root(path: &Path) -> Option<PathBuf> {
    sftp_mount_context(path).map(|context| context.mount_path)
}

/// Sichtbare Wurzel eines von DualBeam über SSHFS oder rclone eingehängten
/// Netzlaufwerks. Alle diese Mounts liegen technisch im App-Support-Ordner;
/// dessen Eltern sind keine sinnvolle Navigationsebene für den Benutzer.
pub fn remote_mount_root(path: &Path) -> Option<PathBuf> {
    let real_path = canonicalize_with_missing_suffix(path);
    let list = registry().lock().ok()?;
    list.iter()
        .filter(|entry| entry.remote_spec.is_some())
        .filter_map(|entry| {
            let mount = canonicalize_with_missing_suffix(&entry.path);
            real_path.starts_with(&mount).then(|| entry.path.clone())
        })
        .max_by_key(|mount| mount.as_os_str().len())
}

fn sftp_batch_arg(value: &str) -> Result<String, String> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("Ungültiger SFTP-Dateiname".into());
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn sftp_relative_path(path: &Path, mount_path: &Path) -> Result<String, String> {
    let real_path = canonicalize_with_missing_suffix(path);
    let real_mount = canonicalize_with_missing_suffix(mount_path);
    let relative = real_path
        .strip_prefix(&real_mount)
        .map_err(|_| "Dieser Ordner wurde nicht von DualBeam eingehängt.".to_string())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| "Ungültiger SFTP-Dateiname".to_string())?,
            ),
            std::path::Component::CurDir => {}
            _ => return Err("Ungültiger SFTP-Zielpfad".into()),
        }
    }
    if parts.is_empty() {
        return Err("Das Stammverzeichnis des SFTP-Laufwerks kann nicht kopiert werden".into());
    }
    Ok(parts.join("/"))
}

fn sftp_initial_directory(spec: &RemoteSpec) -> Option<String> {
    let path = spec.path.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

fn sftp_connect_address(spec: &RemoteSpec) -> String {
    let host = spec.host.trim_start_matches('[').trim_end_matches(']');
    if host.contains(':') {
        format!("{}@[{host}]", spec.username)
    } else {
        format!("{}@{host}", spec.username)
    }
}

fn append_sftp_mkdirs(script: &mut String, directory: &str) -> Result<(), String> {
    let mut cumulative = String::new();
    for part in directory.split('/').filter(|part| !part.is_empty()) {
        if !cumulative.is_empty() {
            cumulative.push('/');
        }
        cumulative.push_str(part);
        // Ein bereits vorhandener Ordner ist beim erneuten Kopieren kein
        // Fehler. Das führende '-' ist dafür der dokumentierte SFTP-
        // Batchmodus-Schalter.
        script.push_str("-mkdir ");
        script.push_str(&sftp_batch_arg(&cumulative)?);
        script.push('\n');
    }
    Ok(())
}

fn sftp_should_skip_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".DualBeamUndo" || crate::is_dualbeam_inprogress_name(name))
}

const SFTP_FILE_DONE_PREFIX: &str = "__DUALBEAM_FILE_DONE_";

struct NativeSftpUpload {
    path: String,
    size: u64,
}

fn append_sftp_file_done_marker(script: &mut String, index: usize) {
    // Der Befehl ist vollständig intern erzeugt und enthält keinerlei
    // Dateinamen. Er läuft erst nach einem erfolgreichen `put` und liefert
    // damit einen eindeutigen, sofort sichtbaren Dateizähler.
    script.push_str(&format!(
        "!printf '{SFTP_FILE_DONE_PREFIX}{index}__\\n' >&2\n"
    ));
}

fn append_sftp_put(
    script: &mut String,
    local: &str,
    remote: &str,
    index: usize,
    overwrite: bool,
) -> Result<(), String> {
    let upload_target = if overwrite {
        let temporary = format!(
            "{remote}.dualbeam-sftp-{}-{index}.inprogress",
            std::process::id()
        );
        // Altlast einer zuvor unterbrochenen Aktualisierung entfernen. Der
        // endgültige Name bleibt bis zum vollständig bestätigten Upload
        // unangetastet.
        script.push_str("-rm ");
        script.push_str(&sftp_batch_arg(&temporary)?);
        script.push('\n');
        temporary
    } else {
        remote.to_string()
    };

    script.push_str("put ");
    script.push_str(&sftp_batch_arg(local)?);
    script.push(' ');
    script.push_str(&sftp_batch_arg(&upload_target)?);
    script.push('\n');

    if overwrite {
        // Swiss-Backup-SFTP unterstützt das Schreiben neuer Dateien, lehnt
        // jedoch das direkte Überschreiben mit "Operation unsupported" ab.
        // Erst nach erfolgreichem Upload das alte Ziel ersetzen.
        script.push_str("-rm ");
        script.push_str(&sftp_batch_arg(remote)?);
        script.push('\n');
        script.push_str("rename ");
        script.push_str(&sftp_batch_arg(&upload_target)?);
        script.push(' ');
        script.push_str(&sftp_batch_arg(remote)?);
        script.push('\n');
    }
    append_sftp_file_done_marker(script, index);
    Ok(())
}

fn sftp_meter_percent(record: &[u8]) -> Option<u8> {
    let percent = record.iter().rposition(|byte| *byte == b'%')?;
    let mut start = percent;
    while start > 0 && record[start - 1].is_ascii_digit() {
        start -= 1;
    }
    (start < percent)
        .then(|| {
            std::str::from_utf8(&record[start..percent])
                .ok()?
                .parse::<u8>()
                .ok()
        })
        .flatten()
        .filter(|value| *value <= 100)
}

fn sftp_completed_file_index(record: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(record).ok()?;
    let start = text.find(SFTP_FILE_DONE_PREFIX)? + SFTP_FILE_DONE_PREFIX.len();
    let end = text[start..].find("__")? + start;
    text[start..end].parse().ok()
}

struct NativeSftpProgressState {
    pending: Vec<u8>,
    completed: usize,
    completed_bytes: u64,
    total_bytes: u64,
    last_percent: u8,
}

impl NativeSftpProgressState {
    fn new(uploads: &[NativeSftpUpload]) -> Self {
        Self {
            pending: Vec::new(),
            completed: 0,
            completed_bytes: 0,
            total_bytes: uploads.iter().map(|upload| upload.size).sum(),
            last_percent: 0,
        }
    }

    fn emit_percent(
        &mut self,
        file_percent: u8,
        uploads: &[NativeSftpUpload],
        progress: &mut dyn FnMut(SftpCopyProgress),
    ) {
        if uploads.is_empty() {
            return;
        }
        let overall = if self.total_bytes > 0 {
            let active_bytes = uploads
                .get(self.completed)
                .map(|upload| upload.size.saturating_mul(file_percent as u64) / 100)
                .unwrap_or(0);
            self.completed_bytes
                .saturating_add(active_bytes)
                .saturating_mul(100)
                .checked_div(self.total_bytes)
                .unwrap_or(0)
                .min(99) as u8
        } else {
            ((self.completed.saturating_mul(100) + file_percent as usize) / uploads.len()).min(99)
                as u8
        };
        if overall > self.last_percent {
            self.last_percent = overall;
            progress(SftpCopyProgress::Percent(overall));
        }
    }

    fn complete_through(
        &mut self,
        index: usize,
        uploads: &[NativeSftpUpload],
        progress: &mut dyn FnMut(SftpCopyProgress),
    ) {
        while self.completed <= index && self.completed < uploads.len() {
            let upload = &uploads[self.completed];
            self.completed_bytes = self.completed_bytes.saturating_add(upload.size);
            self.completed += 1;
            progress(SftpCopyProgress::FileCopied(upload.path.clone()));
        }
        self.emit_percent(0, uploads, progress);
    }

    fn process_record(
        &mut self,
        record: &[u8],
        uploads: &[NativeSftpUpload],
        progress: &mut dyn FnMut(SftpCopyProgress),
    ) {
        if let Some(index) = sftp_completed_file_index(record) {
            self.complete_through(index, uploads, progress);
        } else if let Some(percent) = sftp_meter_percent(record) {
            self.emit_percent(percent, uploads, progress);
        }
    }

    fn consume(
        &mut self,
        chunk: &[u8],
        uploads: &[NativeSftpUpload],
        progress: &mut dyn FnMut(SftpCopyProgress),
    ) {
        for byte in chunk {
            if *byte == b'\r' || *byte == b'\n' {
                if !self.pending.is_empty() {
                    let record = std::mem::take(&mut self.pending);
                    self.process_record(&record, uploads, progress);
                }
            } else {
                self.pending.push(*byte);
                // Diagnosezeilen können theoretisch ohne Zeilenende sehr lang
                // sein. Nur den relevanten jüngsten Ausschnitt aufbewahren.
                if self.pending.len() > 8_192 {
                    self.pending.drain(..4_096);
                }
            }
        }
    }

    fn finish(&mut self, uploads: &[NativeSftpUpload], progress: &mut dyn FnMut(SftpCopyProgress)) {
        if !self.pending.is_empty() {
            let record = std::mem::take(&mut self.pending);
            self.process_record(&record, uploads, progress);
        }
        if !uploads.is_empty() {
            self.complete_through(uploads.len() - 1, uploads, progress);
        }
        self.last_percent = 100;
        progress(SftpCopyProgress::Percent(100));
    }
}

fn append_sftp_uploads(
    script: &mut String,
    source: &Path,
    target: &str,
    overwrite: bool,
    uploads: &mut Vec<NativeSftpUpload>,
) -> Result<(), String> {
    let source_meta = std::fs::symlink_metadata(source)
        .map_err(|error| format!("{}: {error}", source.display()))?;
    if source_meta.is_dir() && !source_meta.file_type().is_symlink() {
        append_sftp_mkdirs(script, target)?;
        for entry in walkdir::WalkDir::new(source)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !sftp_should_skip_path(entry.path()))
        {
            let entry =
                entry.map_err(|error| format!("Verzeichnis lesen fehlgeschlagen: {error}"))?;
            let local = entry.path();
            if local == source || sftp_should_skip_path(local) {
                continue;
            }
            let relative = local
                .strip_prefix(source)
                .map_err(|_| "Ungültiger lokaler SFTP-Quellpfad".to_string())?;
            let remote = format!(
                "{}/{}",
                target.trim_end_matches('/'),
                relative.to_string_lossy()
            );
            if entry.file_type().is_dir() {
                append_sftp_mkdirs(script, &remote)?;
                continue;
            }
            if !entry.file_type().is_file() && !entry.file_type().is_symlink() {
                continue;
            }
            let index = uploads.len();
            append_sftp_put(script, &local.to_string_lossy(), &remote, index, overwrite)?;
            uploads.push(NativeSftpUpload {
                path: local.to_string_lossy().into_owned(),
                size: std::fs::metadata(local).map(|meta| meta.len()).unwrap_or(0),
            });
        }
    } else {
        if sftp_should_skip_path(source) {
            return Ok(());
        }
        if let Some(parent) = Path::new(target).parent() {
            let parent = parent.to_string_lossy();
            if !parent.is_empty() && parent != "." {
                append_sftp_mkdirs(script, &parent)?;
            }
        }
        let index = uploads.len();
        append_sftp_put(script, &source.to_string_lossy(), target, index, overwrite)?;
        uploads.push(NativeSftpUpload {
            path: source.to_string_lossy().into_owned(),
            size: std::fs::metadata(source)
                .map(|meta| meta.len())
                .unwrap_or(0),
        });
    }
    Ok(())
}

fn sftp_client_error(stderr: &[u8], password: &str) -> String {
    let redacted = String::from_utf8_lossy(stderr).replace(password, "[geschützt]");
    let normalized = redacted.replace('\r', "\n");
    let records = normalized
        .lines()
        .map(|line| {
            line.chars()
                .filter(|character| !character.is_control())
                .collect::<String>()
        })
        .map(|line| line.trim().trim_start_matches("^D").trim().to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let diagnostic_markers = [
        "dest open ",
        "write remote ",
        "Permission denied",
        "Host key verification failed",
        "Connection closed",
        "Connection refused",
        "Connection timed out",
        "Operation timed out",
        "No such file",
        "No space left",
        "Quota exceeded",
    ];
    let mut meaningful = Vec::new();
    for line in &records {
        if let Some(start) = diagnostic_markers
            .iter()
            .filter_map(|marker| line.find(marker))
            .min()
        {
            let message = line[start..].to_string();
            if !meaningful.contains(&message) {
                meaningful.push(message);
            }
        }
    }
    let detail = if meaningful.is_empty() {
        records
            .into_iter()
            .filter(|line| {
                !line.starts_with("sftp>")
                    && line != "Progress meter enabled"
                    && !(line.contains('%') && line.contains("KB/s"))
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        meaningful.join(" ")
    };
    let detail: String = detail.chars().take(1_200).collect();
    if detail.is_empty() {
        "Direkte SFTP-Kopie fehlgeschlagen".into()
    } else {
        format!("Direkte SFTP-Kopie fehlgeschlagen: {detail}")
    }
}

/// Überträgt Dateien direkt mit dem macOS-OpenSSH-SFTP-Client. SSHFS wird
/// bewusst nicht beschrieben: es dient bei SFTP nur der Ansicht und
/// Navigation. So kann ein fehlerhaftes FUSE-Dateihandle niemals eine
/// erfolgreiche Dateiübertragung vortäuschen oder blockieren.
pub fn upload_to_sftp_mount(
    source: &Path,
    destination: &Path,
    overwrite: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SftpCopyProgress),
) -> Result<Vec<String>, String> {
    let context = sftp_mount_context(destination)
        .ok_or_else(|| "Dieser Ordner wurde nicht von DualBeam eingehängt.".to_string())?;
    let target = sftp_relative_path(destination, &context.mount_path)?;
    let password = remote_password(&context.spec)?
        .ok_or_else(|| "Das SFTP-Kennwort fehlt im macOS-Schlüsselbund.".to_string())?;
    if !is_trusted(&context.spec.host, context.spec.port_or_default())? {
        return Err("Der Hostschlüssel des Servers ist noch nicht bestätigt.".into());
    }
    let known_hosts = known_hosts_file()?;
    let askpass = create_sshfs_askpass()?;
    let mut batch_file_to_remove = None;
    let result = (|| -> Result<Vec<String>, String> {
        // Im Batchmodus ist die OpenSSH-Fortschrittsanzeige zunächst aus.
        // Explizit einschalten; ihre CR-getrennten Messwerte werden unten
        // fortlaufend in den Gesamtfortschritt aller Dateien übersetzt.
        let mut script = String::from("progress\n");
        if let Some(root) = sftp_initial_directory(&context.spec) {
            script.push_str("cd ");
            script.push_str(&sftp_batch_arg(&root)?);
            script.push('\n');
        }
        let mut uploads = Vec::new();
        append_sftp_uploads(&mut script, source, &target, overwrite, &mut uploads)?;
        if uploads.is_empty() {
            return Ok(Vec::new());
        }
        script.push_str("bye\n");
        let batch_file = create_sftp_batch_file(&script)?;
        batch_file_to_remove = Some(batch_file.clone());

        // `sftp -b` erzwingt in OpenSSH immer `BatchMode=yes` und schaltet
        // damit Passwortauthentifizierung aus. Zuerst eine kurzlebige,
        // authentifizierte Master-Verbindung aufbauen; der eigentliche
        // Batchtransfer verwendet danach denselben SSH-Kanal und behält seine
        // zuverlässigen Abbruch- und Fehlercodes.
        let port = context.spec.port_or_default().to_string();
        let address = sftp_connect_address(&context.spec);
        let control_socket = rc_socket_path()?;
        let _ = std::fs::remove_file(&control_socket);
        let mut master_command = Command::new("/usr/bin/ssh");
        master_command
            .args(["-N", "-M"])
            .arg("-S")
            .arg(&control_socket)
            .args(["-p", &port])
            .args(["-o", "StrictHostKeyChecking=yes"])
            .args([
                "-o",
                &format!("UserKnownHostsFile={}", openssh_option_path(&known_hosts)),
            ])
            .args(["-o", "GlobalKnownHostsFile=/dev/null"])
            .arg(&address)
            .env("SSH_ASKPASS", &askpass)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "dualbeam-sftp-master")
            .env("DUALBEAM_SSHFS_PASSWORD", &password)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut master = master_command
            .spawn()
            .map_err(|error| format!("SFTP-Anmeldung konnte nicht gestartet werden: {error}"))?;
        let master_stderr = master
            .stderr
            .take()
            .ok_or_else(|| "SFTP-Anmeldung liefert keine Fehlerausgabe".to_string())?;
        let mut master_stderr_reader = Some(std::thread::spawn(move || {
            let mut reader = BufReader::new(master_stderr);
            let mut output = Vec::new();
            let _ = reader.read_to_end(&mut output);
            output
        }));
        let login_deadline = Instant::now() + Duration::from_secs(20);
        let login_result = loop {
            if control_socket.exists() {
                break Ok(());
            }
            match master.try_wait() {
                Ok(Some(_)) => {
                    let stderr = master_stderr_reader
                        .take()
                        .and_then(|reader| reader.join().ok())
                        .unwrap_or_default();
                    break Err(sftp_client_error(&stderr, &password));
                }
                Ok(None) if cancel.load(Ordering::SeqCst) => {
                    let _ = master.kill();
                    let _ = master.wait();
                    if let Some(reader) = master_stderr_reader.take() {
                        let _ = reader.join();
                    }
                    break Err("Kopieren nach SFTP wurde abgebrochen".into());
                }
                Ok(None) if Instant::now() >= login_deadline => {
                    let _ = master.kill();
                    let _ = master.wait();
                    if let Some(reader) = master_stderr_reader.take() {
                        let _ = reader.join();
                    }
                    break Err("Zeitüberschreitung bei der SFTP-Anmeldung".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    let _ = master.kill();
                    let _ = master.wait();
                    if let Some(reader) = master_stderr_reader.take() {
                        let _ = reader.join();
                    }
                    break Err(format!(
                        "SFTP-Anmeldung konnte nicht überwacht werden: {error}"
                    ));
                }
            }
        };
        if let Err(error) = login_result {
            let _ = std::fs::remove_file(&control_socket);
            return Err(error);
        }

        // OpenSSH zeichnet den Byte-Fortschritt nur an einem Terminal auf.
        // `script` stellt ausschließlich für diesen Kindprozess ein PTY bereit;
        // Authentifizierung und Batch-Fehlerverhalten bleiben unverändert.
        let mut command = Command::new("/usr/bin/script");
        command
            .args(["-q", "-F", "/dev/null", "/usr/bin/sftp", "-b"])
            .arg(&batch_file)
            .args(["-P", &port])
            .args([
                "-o",
                &format!("ControlPath={}", openssh_option_path(&control_socket)),
            ])
            .args(["-o", "StrictHostKeyChecking=yes"])
            .args([
                "-o",
                &format!("UserKnownHostsFile={}", openssh_option_path(&known_hosts)),
            ])
            .args(["-o", "GlobalKnownHostsFile=/dev/null"])
            .arg(&address)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let transfer_result = (|| -> Result<Vec<String>, String> {
            let mut child = command.spawn().map_err(|error| {
                format!("Direkter SFTP-Client konnte nicht gestartet werden: {error}")
            })?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "SFTP-Ausgabe konnte nicht gelesen werden".to_string())?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| "SFTP-Fehlerausgabe konnte nicht gelesen werden".to_string())?;
            let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>();
            let stdout_tx = output_tx.clone();
            let stdout_reader = std::thread::spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut collected = Vec::new();
                let mut buffer = [0_u8; 4_096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let chunk = buffer[..read].to_vec();
                            collected.extend_from_slice(&chunk);
                            let _ = stdout_tx.send(chunk);
                        }
                        Err(_) => break,
                    }
                }
                collected
            });
            let stderr_reader = std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut collected = Vec::new();
                let mut buffer = [0_u8; 4_096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            let chunk = buffer[..read].to_vec();
                            collected.extend_from_slice(&chunk);
                            let _ = output_tx.send(chunk);
                        }
                        Err(_) => break,
                    }
                }
                collected
            });
            let mut progress_state = NativeSftpProgressState::new(&uploads);
            progress(SftpCopyProgress::Percent(0));
            loop {
                while let Ok(chunk) = output_rx.try_recv() {
                    progress_state.consume(&chunk, &uploads, progress);
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let mut output = stdout_reader.join().unwrap_or_default();
                        output.extend(stderr_reader.join().unwrap_or_default());
                        while let Ok(chunk) = output_rx.try_recv() {
                            progress_state.consume(&chunk, &uploads, progress);
                        }
                        // macOS `script` reicht den Status seines Kindprozesses
                        // nicht zuverlässig durch. Der Marker hinter jedem
                        // erfolgreichen `put` ist daher die verbindliche
                        // Abschlussbestätigung.
                        if status.success() && progress_state.completed == uploads.len() {
                            progress_state.finish(&uploads, progress);
                            return Ok(uploads.iter().map(|upload| upload.path.clone()).collect());
                        }
                        return Err(sftp_client_error(&output, &password));
                    }
                    Ok(None) if cancel.load(Ordering::SeqCst) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err("Kopieren nach SFTP wurde abgebrochen".into());
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(format!(
                            "Direkter SFTP-Client konnte nicht überwacht werden: {error}"
                        ));
                    }
                }
            }
        })();
        let _ = master.kill();
        let _ = master.wait();
        if let Some(reader) = master_stderr_reader.take() {
            let _ = reader.join();
        }
        let _ = std::fs::remove_file(&control_socket);
        transfer_result
    })();
    let _ = std::fs::remove_file(askpass);
    if let Some(batch_file) = batch_file_to_remove {
        let _ = std::fs::remove_file(batch_file);
    }
    result
}

fn object_storage_mount_context(path: &Path) -> Option<ObjectStorageMountContext> {
    let Ok(list) = registry().lock() else {
        return None;
    };
    // Erst prüfen, ob überhaupt ein Objekt-Speicher eingehängt ist. `canonicalize`
    // löst ein `lstat` aus; zeigt der Pfad auf eine lastende Netz-Einhängung,
    // wartet das bis zum Mount-Timeout. Ohne S3/Swift gäbe es hier ohnehin
    // nichts zu finden — dieses Warten wäre also vollständig umsonst.
    if !list.iter().any(|entry| entry.object_profile.is_some()) {
        return None;
    }
    let real_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    list.iter().find_map(|entry| {
        let profile = entry.object_profile.as_ref()?;
        let mount_path = std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
        real_path
            .starts_with(&mount_path)
            .then(|| ObjectStorageMountContext {
                descriptor: entry.descriptor.clone(),
                mount_path,
                real_path: real_path.clone(),
                profile: profile.clone(),
            })
    })
}

/// Stellt ausschließlich für S3 die flüchtige Registrierung eines noch
/// sichtbaren DualBeam-Mountpfads wieder her. Objekt-Speicher verwendet keinen
/// Kernel-Mount; nach einem App-Neustart ist ein leerer lokaler Kennungspfad
/// deshalb kein Beweis für eine aktive Verbindung. Der vom Aufrufer gelieferte
/// Pfad wird nur innerhalb der eigenen Mountwurzel und nur relativ zu dessen
/// erklärter S3-Wurzel akzeptiert.
///
/// Swift ist bewusst ausgenommen: Dort bleibt der bereits funktionierende
/// Ablauf vollständig unverändert.
fn reconnect_s3_copy_context(
    profile: &ObjectStorageProfile,
    declared_mount: &Path,
    object_path: &Path,
) -> Result<Option<ObjectStorageMountContext>, String> {
    if profile.protocol != ObjectStorageProtocol::S3 {
        return Ok(None);
    }

    let root = mount_root()?;
    let real_root = std::fs::canonicalize(&root).unwrap_or(root);
    let real_declared_mount =
        std::fs::canonicalize(declared_mount).unwrap_or_else(|_| declared_mount.to_path_buf());
    if !real_declared_mount.starts_with(&real_root) || real_declared_mount == real_root {
        return Ok(None);
    }
    // Das neue S3-Ziel existiert vor dem ersten Upload absichtlich noch nicht.
    // `canonicalize` würde dann den Pfad unverändert unter `/Users/...`
    // lassen, während die vorhandene Mountwurzel als
    // `/System/Volumes/Data/Users/...` zurückkommt. Die gemeinsame,
    // existierende Vorfahrwurzel wird deshalb aufgelöst und der fehlende
    // Suffix wieder angehängt.
    let real_object_path = canonicalize_with_missing_suffix(object_path);
    let Ok(relative) = real_object_path.strip_prefix(&real_declared_mount) else {
        return Ok(None);
    };
    // Selbst wenn eine manipulierte IPC-Nachricht einen Sonderpfad enthielte,
    // darf daraus nie ein Ziel außerhalb der neu aufgebauten S3-Wurzel werden.
    if relative.components().any(|component| {
        !matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    }) {
        return Ok(None);
    }

    let descriptor = format!("s3://{}", profile.id);
    let active_root = registry().lock().ok().and_then(|list| {
        list.iter().find_map(|entry| {
            (entry.descriptor == descriptor
                && entry
                    .object_profile
                    .as_ref()
                    .is_some_and(|active| active.protocol == ObjectStorageProtocol::S3))
            .then(|| std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone()))
        })
    });
    let active_root = match active_root {
        Some(path) => {
            object_storage_copy_log("S3 copy path remapped to the active mount registration");
            path
        }
        None => {
            object_storage_copy_log("S3 mount registration missing; reconnecting S3 profile");
            let secret = crate::object_storage::object_storage_secret(&profile.id)?;
            PathBuf::from(mount_object_storage(profile, &secret)?)
        }
    };
    let remapped_path = active_root.join(relative);
    let context = object_storage_mount_context(&remapped_path);
    if context.is_some() {
        object_storage_copy_log("S3 mount registration restored for direct copy");
    }
    Ok(context)
}

/// Wie `canonicalize`, aber auch für einen noch nicht angelegten Zielpfad.
/// Das wird für direkte S3- und SFTP-Uploads benötigt: Das Ziel kann beim
/// Start der Übertragung naturgemäß noch nicht im lokalen Dateisystem liegen.
fn canonicalize_with_missing_suffix(path: &Path) -> PathBuf {
    let mut current = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(current) {
            let mut resolved = real;
            for component in missing.iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }
        let Some(name) = current.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            return path.to_path_buf();
        };
        current = parent;
    }
}

#[derive(Debug, Deserialize)]
struct RcloneListEntry {
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Size", default)]
    size: i64,
    #[serde(rename = "ModTime", default)]
    modified: Option<String>,
    #[serde(rename = "IsDir", default)]
    is_dir: bool,
}

/// S3-Bucket-Wurzeln besitzen oft keinen Zeitstempel. rclone liefert dafür
/// je nach Anbieter `null` statt eines fehlenden Feldes; beides ist für die
/// Dateiansicht ein unbekanntes Änderungsdatum.
fn rclone_list_mtime(value: Option<&str>) -> i64 {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|time| time.timestamp())
        .unwrap_or(0)
}

/// rclone kennzeichnet die Größe einiger virtueller S3-Ordner mit `-1`.
/// Solche Einträge besitzen keine übertragbare Dateigröße.
fn rclone_list_size(value: i64) -> u64 {
    value.max(0) as u64
}

/// Manche S3-kompatiblen Anbieter antworten bei einer leeren Bucket-Wurzel
/// mit JSON `null` statt `[]`. Beide Antworten stehen für eine leere Liste.
fn parse_rclone_list_entries(
    bytes: &[u8],
    error_message: &'static str,
) -> Result<Vec<RcloneListEntry>, String> {
    serde_json::from_slice::<Option<Vec<RcloneListEntry>>>(bytes)
        .map(|entries| entries.unwrap_or_default())
        // Der Parsergrund enthält ausschließlich JSON-Strukturinformationen
        // (z. B. `null` statt Zeichenkette), nie Antwortinhalte oder Secrets.
        .map_err(|error| format!("{error_message}: {error}"))
}

/// Ein Eintrag aus einem direkten S3-/Swift-Listing. Die Struktur ist bewusst
/// unabhängig vom Dateisystem, damit die Pane-Anzeige ohne NFS auskommt.
#[derive(Debug, Clone)]
pub struct ObjectStorageEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

fn object_storage_target(context: &ObjectStorageMountContext) -> Result<String, String> {
    let relative =
        object_relative_path(&context.real_path, &context.mount_path).unwrap_or_default();
    if relative.is_empty() {
        Ok(object_storage_argument(&context.profile))
    } else {
        Ok(format!(
            "{}/{}",
            object_storage_argument(&context.profile).trim_end_matches('/'),
            relative
        ))
    }
}

fn direct_object_storage_command(
    profile: &ObjectStorageProfile,
    secret: &str,
    args: &[String],
) -> Result<std::process::Output, String> {
    let rclone = rclone_executable()?;
    let mut command = Command::new(rclone);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in object_storage_env(profile, secret) {
        command.env(key, value);
    }
    command
        .output()
        .map_err(|_| "Objekt-Speicher-Befehl konnte nicht gestartet werden".to_string())
}

fn direct_object_storage_error(stderr: &[u8], secret: &str) -> String {
    let detail = String::from_utf8_lossy(stderr)
        .replace(secret, "[geschützt]")
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(600)
        .collect::<String>();
    if detail.is_empty() {
        "Objekt-Speicher-Anfrage fehlgeschlagen".into()
    } else {
        format!("Objekt-Speicher-Anfrage fehlgeschlagen: {detail}")
    }
}

/// Liest einen Objekt-Speicherordner unmittelbar über dessen API. `None`
/// bedeutet, dass der Pfad kein aktiver Objekt-Speicher-Dateiraum ist.
pub fn list_object_storage_dir(path: &Path) -> Option<Result<Vec<ObjectStorageEntry>, String>> {
    let context = object_storage_mount_context(path)?;
    Some((|| {
        let secret = crate::object_storage::object_storage_secret(&context.profile.id)?;
        let target = object_storage_target(&context)?;
        let output = direct_object_storage_command(
            &context.profile,
            &secret,
            &[
                "lsjson".to_string(),
                "--no-mimetype".to_string(),
                "--contimeout".to_string(),
                "15s".to_string(),
                "--timeout".to_string(),
                "2m".to_string(),
                "--retries".to_string(),
                "2".to_string(),
                target,
            ],
        )?;
        if !output.status.success() {
            return Err(direct_object_storage_error(&output.stderr, &secret));
        }
        let entries = parse_rclone_list_entries(
            &output.stdout,
            "Objekt-Speicher-Antwort konnte nicht gelesen werden",
        )?;
        Ok(entries
            .into_iter()
            .map(|entry| ObjectStorageEntry {
                path: context.real_path.join(&entry.name),
                is_dir: entry.is_dir,
                size: rclone_list_size(entry.size),
                mtime: rclone_list_mtime(entry.modified.as_deref()),
                name: entry.name,
            })
            .collect())
    })())
}

/// Rekursives Listing für die Sync-Vorschau. Auch dies liest ausschließlich
/// die Objekt-Metadaten; es lädt keine Dateiinhalte herunter.
pub fn list_object_storage_tree(path: &Path) -> Option<Result<Vec<ObjectStorageEntry>, String>> {
    let context = object_storage_mount_context(path)?;
    Some((|| {
        let secret = crate::object_storage::object_storage_secret(&context.profile.id)?;
        let target = object_storage_target(&context)?;
        let output = direct_object_storage_command(
            &context.profile,
            &secret,
            &[
                "lsjson".to_string(),
                "--recursive".to_string(),
                "--no-mimetype".to_string(),
                "--contimeout".to_string(),
                "15s".to_string(),
                "--timeout".to_string(),
                "2m".to_string(),
                "--retries".to_string(),
                "2".to_string(),
                target,
            ],
        )?;
        if !output.status.success() {
            return Err(direct_object_storage_error(&output.stderr, &secret));
        }
        let entries = parse_rclone_list_entries(
            &output.stdout,
            "Objekt-Speicher-Antwort konnte nicht gelesen werden",
        )?;
        Ok(entries
            .into_iter()
            .map(|entry| ObjectStorageEntry {
                path: context.mount_path.join(&entry.path),
                is_dir: entry.is_dir,
                size: rclone_list_size(entry.size),
                mtime: rclone_list_mtime(entry.modified.as_deref()),
                name: entry.path,
            })
            .collect())
    })())
}

fn run_object_storage_path_command(
    context: &ObjectStorageMountContext,
    command: &str,
    extra_args: &[&str],
) -> Result<(), String> {
    let secret = crate::object_storage::object_storage_secret(&context.profile.id)?;
    let mut args: Vec<String> = vec![command.to_string()];
    args.extend(extra_args.iter().map(|value| (*value).to_string()));
    args.push(object_storage_target(context)?);
    let output = direct_object_storage_command(&context.profile, &secret, &args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(direct_object_storage_error(&output.stderr, &secret))
    }
}

/// Erzeugt einen Objekt-Speicherordner ohne den Umweg über ein lokales
/// Dateisystem. `None` kennzeichnet einen gewöhnlichen lokalen Pfad.
pub fn create_object_storage_dir(path: &Path) -> Option<Result<(), String>> {
    let context = object_storage_mount_context(path)?;
    Some(run_object_storage_path_command(&context, "mkdir", &[]))
}

/// Legt eine leere Datei direkt im Objekt-Speicher an.
pub fn create_object_storage_file(path: &Path) -> Option<Result<(), String>> {
    let context = object_storage_mount_context(path)?;
    Some(run_object_storage_path_command(&context, "touch", &[]))
}

/// Prüft einen Namen im direkten Parent-Listing. `lsjson --stat` kann für
/// nicht vorhandene S3-/Swift-Präfixe ein leeres Verzeichnis vortäuschen und
/// eignet sich deshalb nicht für die Konfliktprüfung.
pub fn object_storage_path_exists(path: &Path) -> Option<Result<bool, String>> {
    let context = object_storage_mount_context(path)?;
    if context.real_path == context.mount_path {
        return Some(Ok(true));
    }
    let Some(name) = context
        .real_path
        .file_name()
        .map(|name| name.to_os_string())
    else {
        return Some(Ok(true));
    };
    let parent = context.real_path.parent().unwrap_or(&context.mount_path);
    Some(
        list_object_storage_dir(parent)
            .unwrap_or_else(|| Ok(Vec::new()))
            .map(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.name == name.to_string_lossy())
            }),
    )
}

/// Liefert die Metadaten eines einzelnen Eintrags aus dessen Parent-Listing.
/// Für S3-/Swift-Präfixe ist dies zuverlässiger als `lsjson --stat`.
pub fn object_storage_entry(path: &Path) -> Option<Result<Option<ObjectStorageEntry>, String>> {
    let context = object_storage_mount_context(path)?;
    let Some(name) = context
        .real_path
        .file_name()
        .map(|name| name.to_os_string())
    else {
        return Some(Ok(Some(ObjectStorageEntry {
            name: String::new(),
            path: context.mount_path,
            is_dir: true,
            size: 0,
            mtime: 0,
        })));
    };
    let parent = context.real_path.parent().unwrap_or(&context.mount_path);
    Some(
        list_object_storage_dir(parent)
            .unwrap_or_else(|| Ok(Vec::new()))
            .map(|entries| {
                entries
                    .into_iter()
                    .find(|entry| entry.name == name.to_string_lossy())
            }),
    )
}

pub fn object_storage_path_is_dir(path: &Path) -> Option<Result<bool, String>> {
    object_storage_entry(path)
        .map(|result| result.map(|entry| entry.is_some_and(|entry| entry.is_dir)))
}

/// Benennt ein Objekt bzw. einen Objekt-Präfix direkt auf dem Server um.
pub fn rename_object_storage_path(old: &Path, new: &Path) -> Option<Result<(), String>> {
    let source = object_storage_mount_context(old)?;
    let target = object_storage_mount_context(new)?;
    Some((|| {
        if source.descriptor != target.descriptor || source.mount_path != target.mount_path {
            return Err(
                "Objekt-Speicher kann nur innerhalb desselben Laufwerks umbenannt werden".into(),
            );
        }
        let secret = crate::object_storage::object_storage_secret(&source.profile.id)?;
        let from = object_storage_target(&source)?;
        let to = object_storage_target(&target)?;
        let output = direct_object_storage_command(
            &source.profile,
            &secret,
            &[
                "moveto".to_string(),
                "--contimeout".to_string(),
                "15s".to_string(),
                "--timeout".to_string(),
                "2m".to_string(),
                "--retries".to_string(),
                "3".to_string(),
                from,
                to,
            ],
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(direct_object_storage_error(&output.stderr, &secret))
        }
    })())
}

/// Lädt eine einzelne Objekt-Speicherdatei in einen privaten, temporären
/// DualBeam-Ordner. Dadurch funktionieren „Öffnen mit“ und Quick Look ohne
/// dass ein S3-/Swift-Dateisystem im macOS-Kernel eingehängt werden muss.
pub fn download_object_storage_file(path: &Path) -> Option<Result<PathBuf, String>> {
    let context = object_storage_mount_context(path)?;
    Some((|| {
        let secret = crate::object_storage::object_storage_secret(&context.profile.id)?;
        let source = object_storage_target(&context)?;
        let name = context
            .real_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "Ungültiger Objekt-Speicher-Dateiname".to_string())?;
        let cache = app_dir()?.join("object-open-cache");
        std::fs::create_dir_all(&cache)
            .map_err(|_| "Objekt-Speicher-Cache konnte nicht erstellt werden".to_string())?;
        let sequence = RC_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let local = cache.join(format!("{}-{sequence}-{name}", std::process::id()));
        let output = direct_object_storage_command(
            &context.profile,
            &secret,
            &[
                "copyto".to_string(),
                "--transfers".to_string(),
                "1".to_string(),
                "--checkers".to_string(),
                "1".to_string(),
                "--contimeout".to_string(),
                "15s".to_string(),
                "--timeout".to_string(),
                "2m".to_string(),
                "--retries".to_string(),
                "3".to_string(),
                source,
                local.to_string_lossy().into_owned(),
            ],
        )?;
        if output.status.success() {
            Ok(local)
        } else {
            let _ = std::fs::remove_file(&local);
            Err(direct_object_storage_error(&output.stderr, &secret))
        }
    })())
}

/// Stellt einen Objekt-Speicherpfad kurzzeitig als lokale Datei oder lokalen
/// Ordner bereit. Dieser Zwischenschritt ist ausschließlich für Ziele gedacht,
/// die selbst einen eigenen Übertragungsweg haben – insbesondere WebDAV. So
/// schreibt rclone niemals in einen macOS-WebDAV-Mount; der anschließende
/// Upload erfolgt dort über den bestätigten WebDAV-PUT im Hauptmodul.
pub fn materialize_object_storage_path(path: &Path) -> Option<Result<PathBuf, String>> {
    let context = object_storage_mount_context(path)?;
    Some((|| {
        let is_dir = object_storage_path_is_dir(&context.real_path)
            .ok_or_else(|| "Objekt-Speicherpfad ist nicht aktiv".to_string())??;
        let secret = crate::object_storage::object_storage_secret(&context.profile.id)?;
        let source = object_storage_target(&context)?;
        let cache = app_dir()?.join("object-transfer-cache");
        std::fs::create_dir_all(&cache).map_err(|_| {
            "Objekt-Speicher-Zwischenspeicher konnte nicht erstellt werden".to_string()
        })?;
        let sequence = RC_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let local = cache.join(format!("{}-{sequence}", std::process::id()));
        let args = vec![
            if is_dir { "copy" } else { "copyto" }.to_string(),
            "--transfers".to_string(),
            context.profile.parallel_transfers.to_string(),
            "--checkers".to_string(),
            "1".to_string(),
            "--contimeout".to_string(),
            "15s".to_string(),
            "--timeout".to_string(),
            "2m".to_string(),
            "--retries".to_string(),
            "3".to_string(),
            source,
            local.to_string_lossy().into_owned(),
        ];
        // Bei Verzeichnissen kopiert rclone die Inhalte in den frisch
        // reservierten Zielordner. Ein vorhandener Pfad darf dafür nie als
        // semantisches Ziel wiederverwendet werden.
        let output = direct_object_storage_command(&context.profile, &secret, &args)?;
        if !output.status.success() {
            cleanup_object_storage_materialization(&local);
            return Err(direct_object_storage_error(&output.stderr, &secret));
        }
        let valid = std::fs::metadata(&local)
            .map(|metadata| metadata.is_dir() == is_dir)
            .unwrap_or(false);
        if valid {
            Ok(local)
        } else {
            cleanup_object_storage_materialization(&local);
            Err("Objekt-Speicher-Zwischenkopie konnte nicht bestätigt werden".into())
        }
    })())
}

/// Entfernt ausschließlich einen von `materialize_object_storage_path`
/// angelegten, privaten Transferpuffer.
pub fn cleanup_object_storage_materialization(path: &Path) {
    let Ok(cache) = app_dir().map(|dir| dir.join("object-transfer-cache")) else {
        return;
    };
    if !path.starts_with(&cache) {
        return;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(path);
        }
        Err(_) => {}
    }
}

#[derive(Default, Deserialize, Serialize)]
struct ObjectDirectoryTimes {
    /// Schlüssel: Objekt-Speicher-Profilkennung; Wert: relativer Pfad →
    /// Erstellzeitpunkt. Es werden bewusst keine Zugangsdaten gespeichert.
    entries: HashMap<String, HashMap<String, i64>>,
}

fn object_directory_times_path() -> Option<PathBuf> {
    app_dir()
        .ok()
        .map(|dir| dir.join("object-directory-times.json"))
}

fn load_object_directory_times() -> ObjectDirectoryTimes {
    object_directory_times_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_object_directory_times(times: &ObjectDirectoryTimes) {
    let Some(path) = object_directory_times_path() else {
        return;
    };
    let Ok(raw) = serde_json::to_vec(times) else {
        return;
    };
    // Die Zeitstempel sind reine Anzeige-Metadaten. Ein voller Datenträger darf
    // das erfolgreich angelegte Objekt-Speicher-Verzeichnis nicht wieder zu
    // einem Benutzerfehler machen.
    let _ = std::fs::write(path, raw);
}

fn object_relative_path(path: &Path, mount_path: &Path) -> Option<String> {
    let rel = path.strip_prefix(mount_path).ok()?;
    (!rel.as_os_str().is_empty()).then(|| rel.to_string_lossy().replace('\\', "/"))
}

/// Merkt sich den tatsächlichen Anlegezeitpunkt eines in DualBeam erzeugten
/// S3-/Swift-Ordners. Diese Protokolle kennen keine nativen Ordner-Metadaten;
/// rclone kann deshalb nur einen Platzhalterzeitwert liefern.
pub fn remember_object_directory(path: &Path) {
    let Some(context) = object_storage_mount_context(path) else {
        return;
    };
    let Some(relative) = object_relative_path(&context.real_path, &context.mount_path) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let mut times = load_object_directory_times();
    times
        .entries
        .entry(context.descriptor)
        .or_default()
        .entry(relative)
        .or_insert(now);
    save_object_directory_times(&times);
}

/// Liefert die von DualBeam bekannten Zeitstempel der unmittelbaren Unterordner
/// eines Objekt-Speicherpfads – einmal pro Pane-Liste statt pro Eintrag.
pub fn object_directory_times_in(path: &Path) -> HashMap<String, i64> {
    let Some(context) = object_storage_mount_context(path) else {
        return HashMap::new();
    };
    let parent = object_relative_path(&context.real_path, &context.mount_path).unwrap_or_default();
    let prefix = if parent.is_empty() {
        String::new()
    } else {
        format!("{parent}/")
    };
    load_object_directory_times()
        .entries
        .remove(&context.descriptor)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(relative, timestamp)| {
            let child = relative.strip_prefix(&prefix)?;
            (!child.is_empty() && !child.contains('/')).then(|| (child.to_string(), timestamp))
        })
        .collect()
}

/// Hängt ein selbst eingehängtes Netzlaufwerk aus und beendet dessen Client.
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

/// Beendet den Client eines bereits ausgehängten Laufwerks und räumt Ordner
/// sowie Protokoll weg.
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
    if let Some(child) = entry.child.as_mut() {
        // rclone beendet sich nach dem Aushängen von selbst. Kurz warten ist
        // freundlicher, als es sofort abzuschießen.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
    let _ = std::fs::remove_file(&entry.log);
    let _ = std::fs::remove_file(&entry.rc_socket);
    if let Some(askpass) = entry.sshfs_askpass.take() {
        let _ = std::fs::remove_file(askpass);
    }
    let _ = std::fs::remove_dir(&entry.path);
}

/// Nimmt ein selbst eingehängtes Laufwerk ohne Hilfsprozess in die Liste auf.
///
/// NFS spricht der Kernel unmittelbar; es gibt weder rclone noch SSHFS und
/// damit keinen Kindprozess. Über diesen Eintrag greifen Anzeige, Aushängen
/// und das Aufräumen beim Beenden trotzdem unverändert.
pub(crate) fn register_plain_mount(path: PathBuf, label: String, descriptor: String) {
    if let Ok(mut list) = registry().lock() {
        list.push(ActiveMount {
            path,
            object_home: None,
            label,
            descriptor,
            rc_socket: PathBuf::new(),
            child: None,
            log: PathBuf::new(),
            remote_spec: None,
            object_profile: None,
            obscured_password: None,
            sshfs_askpass: None,
        });
    }
}

/// Alle derzeit von DualBeam eingehängten Netzlaufwerke.
pub fn active_mounts() -> Vec<RemoteMountInfo> {
    let Ok(list) = registry().lock() else {
        return Vec::new();
    };
    list.iter()
        .filter(|entry| entry.object_profile.is_some() || is_mount_point(&entry.path))
        .map(|entry| RemoteMountInfo {
            path: entry.path.to_string_lossy().into_owned(),
            home_path: entry
                .object_home
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
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

/// Liest die Einhängetabelle des Kerns. `getfsstat` mit `MNT_NOWAIT` liefert
/// ausschliesslich den bereits zwischengespeicherten Zustand und fragt die
/// Dateisysteme selbst nicht an. Der Aufruf kann deshalb auch dann nicht
/// blockieren, wenn eine Einhaengung ihren Server verloren hat.
#[cfg(target_os = "macos")]
fn kernel_mount_points() -> std::collections::HashSet<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut points = std::collections::HashSet::new();
    unsafe {
        let count = libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT);
        if count <= 0 {
            return points;
        }
        let mut buffer: Vec<libc::statfs> = Vec::with_capacity(count as usize);
        let size = std::mem::size_of::<libc::statfs>() * count as usize;
        let written = libc::getfsstat(buffer.as_mut_ptr(), size as libc::c_int, libc::MNT_NOWAIT);
        if written <= 0 {
            return points;
        }
        buffer.set_len(written as usize);
        for entry in &buffer {
            let raw = std::ffi::CStr::from_ptr(entry.f_mntonname.as_ptr());
            points.insert(PathBuf::from(std::ffi::OsStr::from_bytes(raw.to_bytes())));
        }
    }
    points
}

#[cfg(not(target_os = "macos"))]
fn kernel_mount_points() -> std::collections::HashSet<PathBuf> {
    std::collections::HashSet::new()
}

/// Räumt Reste eines abgestürzten früheren Laufs weg: Ordner, die niemand mehr
/// eingehängt hat, und liegengebliebene Protokolle.
pub fn cleanup_stale() {
    let Ok(root) = mount_root() else {
        return;
    };
    // Beim Start darf kein Zugriff auf einen unbekannten Ordner erfolgen. Blieb
    // aus einem abgestuerzten Lauf eine verwaiste Einhaengung zurueck, blockiert
    // schon ein einzelnes `stat()` endlos; die Anwendung liefe dann zwar als
    // Prozess weiter, zeigte aber nie ein Fenster. Die Einhaengetabelle des
    // Kerns beantwortet dieselbe Frage, ohne die Dateisysteme anzufassen.
    let mounted = kernel_mount_points();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            // `file_type()` stammt aus dem Verzeichniseintrag selbst und loest
            // deshalb - anders als `is_dir()` - keinen Zugriff auf das Ziel aus.
            if !entry
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let path = entry.path();
            // Ältere DualBeam-Versionen haben S3/Swift noch per NFS
            // eingehängt. Beim ersten Start der direkten Variante gehören
            // diese Mounts sicher zu unserem eigenen Ordner und werden
            // kontrolliert gelöst, bevor der leere Kennungsordner entfernt
            // wird.
            if mounted.contains(&path) {
                let _ = unmount_path(&path);
            }
            // Schlägt das Lösen fehl, bleibt der Ordner ein Einhängepunkt und
            // `remove_dir` scheitert folgenlos.
            let _ = std::fs::remove_dir(&path);
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

    /// Die Einhaengetabelle des Kerns muss ohne Zugriff auf die Dateisysteme
    /// lesbar sein. Enthaelt sie die Wurzel, wurde sie tatsaechlich gelesen.
    #[test]
    fn kernel_mount_table_is_readable() {
        let points = kernel_mount_points();
        assert!(
            points.contains(&PathBuf::from("/")),
            "Einhaengetabelle ohne Wurzel: {points:?}"
        );
    }

    /// Ein Verbindungsfehler darf niemals als vollzogene Loeschung gelten.
    #[test]
    fn targets_split_into_parent_and_name() {
        assert_eq!(
            split_remote_target("DUALBEAM:ordner/datei.zip"),
            Some(("DUALBEAM:ordner", "datei.zip"))
        );
        // Ein fuehrender Schraegstrich gehoert nicht in den Elternpfad: Bei
        // WebDAV griffe rclone damit am Adresspfad vorbei.
        assert_eq!(
            split_remote_target("DUALBEAM:/datei.zip"),
            Some(("DUALBEAM:", "datei.zip"))
        );
        assert_eq!(
            split_remote_target("DUALBEAM:datei.zip"),
            Some(("DUALBEAM:", "datei.zip"))
        );
        assert_eq!(
            split_remote_target("DUALBEAM:/freigabe/tief/datei.zip"),
            Some(("DUALBEAM:/freigabe/tief", "datei.zip"))
        );
        // Ohne Namen gibt es nichts zu pruefen.
        assert_eq!(split_remote_target("DUALBEAM:"), None);
        assert_eq!(split_remote_target("DUALBEAM:ordner/"), None);
        assert_eq!(split_remote_target("ohne-doppelpunkt"), None);
    }

    #[test]
    fn hosts_are_checked() {
        assert!(valid_host("example.com"));
        assert!(valid_host("sftp.example.com"));
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
    fn sftp_batch_arguments_preserve_spaces_and_quotes() {
        assert_eq!(
            sftp_batch_arg("Dokumente/Mein Bericht.pdf").unwrap(),
            "\"Dokumente/Mein Bericht.pdf\""
        );
        assert_eq!(
            sftp_batch_arg("Bericht \"final\".pdf").unwrap(),
            "\"Bericht \\\"final\\\".pdf\""
        );
        assert!(sftp_batch_arg("Zeile\nneu").is_err());
    }

    #[test]
    fn sftp_hidden_names_are_transfer_content() {
        assert!(!sftp_should_skip_path(Path::new(".env")));
        assert!(!sftp_should_skip_path(Path::new(".DS_Store")));
        assert!(!sftp_should_skip_path(Path::new("._Urlaub.pdf")));
        assert!(sftp_should_skip_path(Path::new(".DualBeamUndo")));
        assert!(sftp_should_skip_path(Path::new(
            ".upload.zip.dualbeam-40316-0.inprogress"
        )));
    }

    #[test]
    fn sftp_overwrite_uploads_new_file_before_replacing_old_one() {
        let mut script = String::new();
        append_sftp_put(
            &mut script,
            "/tmp/DualBeam.png",
            "Screenshots/DualBeam.png",
            7,
            true,
        )
        .unwrap();
        let temporary = format!(
            "Screenshots/DualBeam.png.dualbeam-sftp-{}-7.inprogress",
            std::process::id()
        );
        assert_eq!(
            script,
            format!(
                "-rm \"{temporary}\"\nput \"/tmp/DualBeam.png\" \"{temporary}\"\n-rm \"Screenshots/DualBeam.png\"\nrename \"{temporary}\" \"Screenshots/DualBeam.png\"\n!printf '__DUALBEAM_FILE_DONE_7__\\n' >&2\n"
            )
        );
    }

    #[test]
    fn sftp_error_hides_terminal_transcript_and_keeps_server_diagnostic() {
        let output = b"^D\x08\x08sftp> progress\r\nProgress meter enabled\r\nsftp> put \"a\" \"b\"\r\nb 100% 165KB 500KB/s 00:00\r\nwrite remote \"b\": Operation unsupported\r\n";
        assert_eq!(
            sftp_client_error(output, "secret"),
            "Direkte SFTP-Kopie fehlgeschlagen: write remote \"b\": Operation unsupported"
        );
    }

    #[test]
    fn native_sftp_progress_combines_bytes_and_completed_files() {
        let uploads = vec![
            NativeSftpUpload {
                path: "/tmp/a.bin".into(),
                size: 100,
            },
            NativeSftpUpload {
                path: "/tmp/b.bin".into(),
                size: 300,
            },
        ];
        let mut events = Vec::new();
        let mut state = NativeSftpProgressState::new(&uploads);
        state.consume(b"a.bin  50% 50B\r", &uploads, &mut |event| {
            events.push(event)
        });
        state.consume(b"__DUALBEAM_FILE_", &uploads, &mut |event| {
            events.push(event)
        });
        state.consume(b"DONE_0__\n", &uploads, &mut |event| events.push(event));
        state.consume(b"b.bin  50% 150B\r", &uploads, &mut |event| {
            events.push(event)
        });
        state.finish(&uploads, &mut |event| events.push(event));

        assert_eq!(
            events,
            vec![
                SftpCopyProgress::Percent(12),
                SftpCopyProgress::FileCopied("/tmp/a.bin".into()),
                SftpCopyProgress::Percent(25),
                SftpCopyProgress::Percent(62),
                SftpCopyProgress::FileCopied("/tmp/b.bin".into()),
                SftpCopyProgress::Percent(99),
                SftpCopyProgress::Percent(100),
            ]
        );
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
        assert_eq!(sanitize_label("Archiv"), Some("Archiv".into()));
        assert_eq!(sanitize_label("a/b"), Some("a-b".into()));
        assert_eq!(sanitize_label(".."), None);
        assert_eq!(sanitize_label("   "), None);
        assert_eq!(
            sanitize_label("mit Leerzeichen"),
            Some("mit Leerzeichen".into())
        );
    }

    #[test]
    fn host_key_requires_matching_public_key_not_just_a_saved_entry() {
        let saved = "# Host example.com found: line 1\nexample.com ssh-ed25519 AAAAold\n";
        let same = vec!["example.com ssh-ed25519 AAAAold".to_string()];
        let changed = vec!["example.com ssh-ed25519 AAAAnew".to_string()];
        assert!(known_host_matches_scan(saved, &same));
        assert!(!known_host_matches_scan(saved, &changed));
    }

    fn spec(protocol: RemoteProtocol) -> RemoteSpec {
        RemoteSpec {
            protocol,
            host: "example.com".into(),
            port: None,
            username: "user".into(),
            path: "/data".into(),
            label: String::new(),
            domain: String::new(),
            base_path: String::new(),
            vendor: String::new(),
        }
    }

    /// Baut ein Objekt-Speicher-Profil aus den Pflichtfeldern; alles Weitere
    /// füllt serde selbst.
    fn objekt_profil(protokoll: &str) -> crate::object_storage::ObjectStorageProfile {
        serde_json::from_value(serde_json::json!({
            "id": "1",
            "name": "Testablage",
            "protocol": protokoll,
            "endpoint": "https://beispiel.example.com",
        }))
        .expect("Profil muss sich aus den Pflichtfeldern bauen lassen")
    }

    /// Sicherung gegen Rückschritte: Der direkte rclone-Weg wurde eingeführt,
    /// weil WebDAV, SMB und FTP über den zwischengespeicherten Mount liefen.
    /// Objekt-Speicher und SFTP hatten diesen Fehler nie und haben eigene,
    /// erprobte Wege – sie dürfen von der neuen Weiche niemals erfasst werden.
    #[test]
    fn direkter_rclone_weg_laesst_objektspeicher_und_sftp_unberuehrt() {
        for protokoll in ["s3", "swift"] {
            let profil = objekt_profil(protokoll);
            assert!(
                !is_rclone_transfer_candidate(Some(&profil), None),
                "Objekt-Speicher {protokoll} wurde erfasst"
            );
            // Auch mit gesetztem RemoteSpec bleibt das Profil ausschlaggebend.
            assert!(
                !is_rclone_transfer_candidate(Some(&profil), Some(&spec(RemoteProtocol::Webdav))),
                "Objekt-Speicher {protokoll} mit Spec wurde erfasst"
            );
        }

        assert!(
            !is_rclone_transfer_candidate(None, Some(&spec(RemoteProtocol::Sftp))),
            "SFTP wurde erfasst"
        );

        // Ohne Angaben gibt es nichts anzusprechen.
        assert!(!is_rclone_transfer_candidate(None, None));

        // Und das, wofür der Weg gebaut wurde:
        for protokoll in [
            RemoteProtocol::Webdav,
            RemoteProtocol::Smb,
            RemoteProtocol::Ftp,
        ] {
            assert!(
                is_rclone_transfer_candidate(None, Some(&spec(protokoll))),
                "{protokoll:?} wurde nicht erfasst"
            );
        }
    }

    #[test]
    fn webdav_adresspfad_darf_die_adresse_nicht_umlenken() {
        let mut s = spec(RemoteProtocol::Webdav);
        s.path = String::new();
        for boese in [
            "/dav?x=1",
            "/dav#weg",
            "//fremder.example.net/dav",
            "/../oben",
            "/dav\\zurueck",
        ] {
            s.base_path = boese.into();
            assert_eq!(
                validate(&s, false).unwrap_err(),
                "err.remote.basePath",
                "durchgelassen: {boese}"
            );
        }
        s.base_path = "/remote.php/dav/files/norbert".into();
        assert!(validate(&s, false).is_ok());
        s.base_path = String::new();
        assert!(validate(&s, false).is_ok(), "leer ist zulässig");
    }

    #[test]
    fn webdav_adresse_laesst_den_standardport_weg() {
        // pCloud antwortet auf `https://host:443/` mit einer Umleitung, die
        // rclone als Fehler wertet. Ohne Portangabe verbindet es sauber.
        let mut s = spec(RemoteProtocol::Webdav);
        s.host = "ewebdav.pcloud.com".into();
        assert_eq!(webdav_url(&s), "https://ewebdav.pcloud.com");
        s.port = Some(443);
        assert_eq!(webdav_url(&s), "https://ewebdav.pcloud.com");
        s.port = Some(8443);
        assert_eq!(webdav_url(&s), "https://ewebdav.pcloud.com:8443");
    }

    #[test]
    fn webdav_adresse_haengt_den_adresspfad_an() {
        // Nextcloud stellt die Dateien unter einem festen Unterpfad bereit.
        // Führende und abschließende Schrägstriche darf der Benutzer setzen
        // oder weglassen, ohne dass eine doppelte Trennung entsteht.
        let mut s = spec(RemoteProtocol::Webdav);
        s.host = "wolke.example.net".into();
        for eingabe in ["/remote.php/dav/files/nojan", "remote.php/dav/files/nojan/"] {
            s.base_path = eingabe.into();
            assert_eq!(
                webdav_url(&s),
                "https://wolke.example.net/remote.php/dav/files/nojan",
                "Eingabe: {eingabe}"
            );
        }
    }

    #[test]
    fn webdav_umgebung_nennt_adresse_statt_rechnername() {
        // rclone wertet bei WebDAV weder `host` noch `port` aus. Stuenden sie
        // dennoch in der Umgebung, ginge die Verbindung auf eine leere Adresse.
        let mut s = spec(RemoteProtocol::Webdav);
        s.host = "ewebdav.pcloud.com".into();
        let env = rclone_env(&s, "geheim", None);
        let key = |name: &str| {
            env.iter()
                .find(|(k, _)| *k == env_key(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(key("type"), Some("webdav"));
        assert_eq!(key("url"), Some("https://ewebdav.pcloud.com"));
        assert_eq!(key("vendor"), Some("other"), "leer bedeutet other");
        assert_eq!(key("host"), None, "host darf nicht gesetzt sein");
        assert_eq!(key("port"), None, "port darf nicht gesetzt sein");

        s.vendor = "nextcloud".into();
        let env = rclone_env(&s, "geheim", None);
        assert!(env.contains(&(env_key("vendor"), "nextcloud".to_string())));
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
        assert_eq!(RemoteProtocol::Smb.default_port(), 445);
    }

    /// Bei SMB ist der erste Pfadteil die Freigabe, kein Ordner. Ein führender
    /// Schrägstrich würde auf die Wurzel des Zugangs zeigen — dort liegt bei
    /// SMB nur die Liste der Freigaben, kein Inhalt.
    #[test]
    fn smb_addresses_the_share_without_a_leading_slash() {
        let mut s = spec(RemoteProtocol::Smb);
        s.path = "/Öffentliche Daten/unter".into();
        assert_eq!(remote_argument(&s), "DUALBEAM:Öffentliche Daten/unter");
        s.path = "Daten".into();
        assert_eq!(remote_argument(&s), "DUALBEAM:Daten");
    }

    /// Ohne Freigabe zeigt SMB nur auf die Liste der Freigaben. Genau daran
    /// scheiterte der frühere Weg über den Finder.
    #[test]
    fn smb_demands_a_share() {
        let mut s = spec(RemoteProtocol::Smb);
        for leer in ["", "/", "   ", "//"] {
            s.path = leer.into();
            assert_eq!(
                validate(&s, false).unwrap_err(),
                "err.remote.shareMissing",
                "leer erkannt: {leer:?}"
            );
        }
        s.path = "Daten".into();
        assert!(validate(&s, false).is_ok());
    }

    /// Das Kennwort geht bei NTLMv2 nie über die Leitung, deshalb gilt SMB als
    /// geschützt: keine Zustimmungspflicht, keine Beschränkung auf lokale
    /// Adressen. NAS-Geräte werden fast immer über einen Namen angesprochen.
    #[test]
    fn smb_needs_no_confirmation_and_reaches_named_hosts() {
        let mut s = spec(RemoteProtocol::Smb);
        s.path = "Daten".into();
        s.host = "nas.fritz.box".into();
        assert!(validate(&s, false).is_ok());
    }

    /// Die Domäne ist bei Heimnetzen leer und darf dann nicht als leerer Wert
    /// gesetzt werden — rclone würde sonst eine leere Domäne aushandeln.
    #[test]
    fn smb_passes_the_domain_only_when_filled() {
        let mut s = spec(RemoteProtocol::Smb);
        s.path = "Daten".into();
        let ohne = rclone_env(&s, "geheim", None);
        assert!(!ohne.iter().any(|(key, _)| key.ends_with("_DOMAIN")));
        assert!(ohne.contains(&("RCLONE_CONFIG_DUALBEAM_TYPE".into(), "smb".into())));
        assert!(ohne.contains(&("RCLONE_CONFIG_DUALBEAM_PASS".into(), "geheim".into())));

        s.domain = "  FIRMA  ".into();
        let mit = rclone_env(&s, "geheim", None);
        assert!(mit.contains(&("RCLONE_CONFIG_DUALBEAM_DOMAIN".into(), "FIRMA".into())));
    }

    #[test]
    fn smb_login_errors_are_shown_as_authentication_failures() {
        for message in [
            "SMB: STATUS_LOGON_FAILURE",
            "access denied",
            "invalid credentials",
        ] {
            assert_eq!(rclone_failure_message(message), "err.remote.auth", "{message}");
        }
    }

    #[test]
    fn smb_allows_only_unspecific_preflight_failures_to_reach_the_mount() {
        assert!(may_continue_after_verification_error(
            RemoteProtocol::Smb,
            "err.remote.mountFailed"
        ));
        assert!(!may_continue_after_verification_error(
            RemoteProtocol::Smb,
            "err.remote.auth"
        ));
        assert!(!may_continue_after_verification_error(
            RemoteProtocol::Smb,
            "err.remote.unreachable"
        ));
        assert!(!may_continue_after_verification_error(
            RemoteProtocol::Webdav,
            "err.remote.mountFailed"
        ));
    }

    #[test]
    fn s3_listing_accepts_null_timestamp_and_empty_null_response() {
        let entries = parse_rclone_list_entries(
            br#"[{"Path":"bucket","Name":"bucket","Size":-1,"ModTime":null,"IsDir":true}]"#,
            "unlesbar",
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(rclone_list_size(entries[0].size), 0);
        assert_eq!(rclone_list_mtime(entries[0].modified.as_deref()), 0);
        assert!(parse_rclone_list_entries(b"null", "unlesbar")
            .unwrap()
            .is_empty());
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

    /// Hängt eine echte SMB-Freigabe ein, liest, schreibt und hängt wieder aus.
    ///
    /// Läuft nur auf Anforderung, weil dafür ein SMB-Server bereitstehen muss:
    /// `cargo test --lib remote::tests::echter_smb_mount -- --ignored --nocapture`
    ///
    /// Gegenstelle beim Entwickeln: Windows 11 in Parallels.
    /// ```powershell
    /// New-Item -ItemType Directory C:\DualBeamTest -Force
    /// New-SmbShare -Name "Daten" -Path C:\DualBeamTest -EncryptData $true
    /// net user smbtest DualBeam-Test-2026 /add
    /// Grant-SmbShareAccess -Name Daten -AccountName "$env:COMPUTERNAME\smbtest" `
    ///   -AccessRight Full -Force
    /// icacls C:\DualBeamTest /grant "smbtest:(OI)(CI)F"
    /// Enable-NetFirewallRule -DisplayGroup "Datei- und Druckerfreigabe"
    /// ```
    /// Das Kennwort steht hier bewusst offen: Es gehört zu einem Wegwerf-Konto
    /// auf einer Testfreigabe und darf nirgends sonst gelten.
    #[test]
    #[ignore = "benötigt eine erreichbare SMB-Freigabe"]
    fn echter_smb_mount() {
        let mut s = spec(RemoteProtocol::Smb);
        s.host = std::env::var("DUALBEAM_SMB_HOST").unwrap_or("10.211.55.3".into());
        s.username = std::env::var("DUALBEAM_SMB_USER").unwrap_or("smbtest".into());
        s.path = std::env::var("DUALBEAM_SMB_SHARE").unwrap_or("Daten".into());
        s.label = "SMB-Praxistest".into();
        let kennwort = std::env::var("DUALBEAM_SMB_PASS").unwrap_or("DualBeam-Test-2026".into());

        let pfad = mount_blocking(s, kennwort, false).expect("Einhängen fehlgeschlagen");
        let dir = Path::new(&pfad);
        assert!(is_mount_point(dir), "kein Mountpunkt: {pfad}");

        // Lesen: die Testdatei ist genau 512 KiB groß.
        let gelesen = std::fs::read(dir.join("probe.bin")).expect("Lesen fehlgeschlagen");
        assert_eq!(gelesen.len(), 524_288, "unerwartete Größe");

        // Schreiben, wiederlesen, entfernen.
        let ziel = dir.join("rust-probe.tmp");
        std::fs::write(&ziel, b"DualBeam").expect("Schreiben fehlgeschlagen");
        assert_eq!(std::fs::read(&ziel).unwrap(), b"DualBeam");
        std::fs::remove_file(&ziel).expect("Löschen fehlgeschlagen");

        // Unterordner müssen als solche erkannt werden, sonst greift die
        // Ordner-Navigation nicht.
        assert!(dir.join("unter").is_dir(), "Unterordner nicht erkannt");

        // Das Laufwerk muss in der gemeinsamen Liste stehen; nur darüber
        // findet die Oberfläche das Aushängen und den Löschschutz.
        assert!(
            active_mounts().iter().any(|m| m.path == pfad),
            "nicht in der Liste der Netzlaufwerke"
        );

        unmount_owned(dir).expect("Aushängen fehlgeschlagen");
        assert!(!is_mount_point(dir), "noch eingehängt");
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
    fn sftp_relative_path_starts_below_the_sftp_home() {
        let mut s = spec(RemoteProtocol::Sftp);
        s.path = "default".into();
        assert_eq!(remote_argument(&s), "DUALBEAM:default");

        // Ein führender Slash bleibt die bewusste Anforderung eines
        // absoluten Serverpfads.
        s.path = "/default".into();
        assert_eq!(remote_argument(&s), "DUALBEAM:/default");
    }

    #[test]
    fn sshfs_preserves_the_configured_sftp_root() {
        let mut s = spec(RemoteProtocol::Sftp);
        s.path = "default".into();
        assert_eq!(sshfs_source(&s), "user@example.com:default");

        s.path = "/default".into();
        assert_eq!(sshfs_source(&s), "user@example.com:/default");
    }

    #[test]
    fn ssh_option_path_escapes_spaces_for_openssh() {
        assert_eq!(
            ssh_option_path(Path::new(
                "/Users/example/Library/Application Support/DualBeam/known_hosts"
            )),
            "/Users/example/Library/Application\\\\ Support/DualBeam/known_hosts"
        );
    }

    #[test]
    fn openssh_option_path_quotes_spaces() {
        assert_eq!(
            openssh_option_path(Path::new(
                "/Users/example/Library/Application Support/DualBeam/known_hosts"
            )),
            "\"/Users/example/Library/Application Support/DualBeam/known_hosts\""
        );
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
    fn sftp_transfer_preserves_the_configured_server_root() {
        let target = sftp_transfer_target(
            Path::new("/tmp/dualbeam-sftp-test/reports/report.pdf"),
            Path::new("/tmp/dualbeam-sftp-test"),
            &spec(RemoteProtocol::Sftp),
        )
        .unwrap();
        assert_eq!(target, "DUALBEAM:/data/reports/report.pdf");
    }

    #[test]
    fn rclone_progress_percentage_is_parsed() {
        assert_eq!(
            rclone_progress_percent("NOTICE: 12.3 MiB / 45.6 MiB, 27%, 1.0 MiB/s"),
            Some(27)
        );
        assert_eq!(rclone_progress_percent("NOTICE: connection refused"), None);
        assert_eq!(rclone_progress_percent("NOTICE: 100%"), Some(100));
    }

    #[test]
    fn rclone_deleted_path_is_parsed() {
        assert_eq!(
            rclone_deleted_path("2026/08/21 19:37:20 INFO  : nested/a.txt: Deleted"),
            Some("nested/a.txt")
        );
        assert_eq!(
            rclone_deleted_path("2026/08/21 19:37:20 INFO  : nested: Removing directory"),
            None
        );
    }

    #[test]
    fn rclone_copied_path_is_parsed() {
        assert_eq!(
            rclone_copied_path("2026/08/21 19:52:19 INFO  : nested/a.txt: Copied (new)"),
            Some("nested/a.txt")
        );
        assert_eq!(rclone_copied_path("NOTICE: 1 MiB / 1 MiB, 100%"), None);
    }

    #[test]
    fn known_hosts_pattern_includes_unusual_ports() {
        assert_eq!(host_pattern("example.com", 22), "example.com");
        assert_eq!(host_pattern("example.com", 2222), "[example.com]:2222");
    }
}
