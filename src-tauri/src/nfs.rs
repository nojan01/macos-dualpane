//! NFS-Netzlaufwerke.
//!
//! Anders als SFTP oder FTP läuft NFS nicht über einen Hilfsprozess, sondern
//! unmittelbar im Kernel. Eingehängt wird deshalb mit `/sbin/mount -t nfs`.
//!
//! Der Weg über den Finder (`mount volume "nfs://…"`) nimmt keinerlei Optionen
//! entgegen. Für die Wahl der Protokollfassung, des Sicherheitsverfahrens oder
//! der Übertragungsart ist der unmittelbare Aufruf daher zwingend.
//!
//! # Der privilegierte Port
//!
//! NFS-Server dürfen verlangen, dass die Gegenstelle von einem Port unterhalb
//! 1024 aus spricht. Solche Ports darf nur der Systemverwalter belegen. DualBeam
//! hängt bewusst ohne Administratorrechte ein und verwendet deshalb `noresvport`.
//!
//! Das ist keine Einschränkung gegenüber dem bisherigen Verhalten: Der Finder
//! verfährt als gewöhnlicher Benutzer genauso. Server, die auf dem privilegierten
//! Port bestehen — bei Linux ist das die Voreinstellung `secure` — bleiben damit
//! unerreichbar. Diesen Fall übersetzt `explain_failure` in einen verständlichen
//! Hinweis samt Lösungsweg, statt den nichtssagenden Systemfehler zu zeigen.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::remote;

/// Zeitlimit für den Einhängevorgang. Ein stiller Server ließe `mount_nfs`
/// sonst sehr lange warten.
const MOUNT_TIMEOUT: Duration = Duration::from_secs(30);

/// Fassung des NFS-Protokolls.
///
/// `Auto` überlässt die Wahl der Aushandlung zwischen Client und Server; das
/// ist der Normalfall. Die übrigen Werte erzwingen eine bestimmte Fassung, was
/// bei älteren Geräten nötig sein kann.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NfsVersion {
    #[default]
    Auto,
    /// Nur für sehr alte Geräte. Kennt keine Dateien über 2 GB.
    V2,
    V3,
    V4,
    /// Höchste von macOS beherrschte Fassung.
    V41,
}

impl NfsVersion {
    fn option(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::V2 => Some("vers=2"),
            Self::V3 => Some("vers=3"),
            Self::V4 => Some("vers=4"),
            Self::V41 => Some("vers=4.1"),
        }
    }
}

/// Sicherheitsverfahren der NFS-Verbindung.
///
/// macOS beherrscht ausschließlich diese vier. `AUTH_NONE` lässt sich nicht
/// erzwingen, wird aber ausgehandelt, wenn ein Server nichts anderes anbietet.
/// Weitergehende Verfahren wie NFS über TLS weist der Client zurück.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NfsSecurity {
    /// Wird zwischen Client und Server ausgehandelt.
    #[default]
    Auto,
    /// Rechte über Benutzer- und Gruppennummer. Ohne jede Kryptografie.
    Sys,
    /// Kerberos-Anmeldung. Daten unverschlüsselt.
    Krb5,
    /// Kerberos-Anmeldung mit Schutz vor Verfälschung. Daten unverschlüsselt.
    Krb5i,
    /// Kerberos-Anmeldung mit vollständiger Verschlüsselung.
    Krb5p,
}

impl NfsSecurity {
    fn option(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Sys => Some("sec=sys"),
            Self::Krb5 => Some("sec=krb5"),
            Self::Krb5i => Some("sec=krb5i"),
            Self::Krb5p => Some("sec=krb5p"),
        }
    }

    /// Nur `krb5p` verschlüsselt die übertragenen Dateiinhalte.
    fn encrypts(self) -> bool {
        matches!(self, Self::Krb5p)
    }

    /// Kerberos weist beide Seiten kryptografisch aus. Anmeldedaten wandern
    /// dabei nie über die Leitung, weshalb für diese Verfahren die sonst für
    /// unverschlüsselte Protokolle geltende Beschränkung auf das eigene Netz
    /// entfällt.
    fn uses_kerberos(self) -> bool {
        matches!(self, Self::Krb5 | Self::Krb5i | Self::Krb5p)
    }
}

/// Übertragungsart. Ältere Unix-Server und einige NAS-Geräte bieten für die
/// Fassungen 2 und 3 ausschließlich UDP an.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NfsTransport {
    #[default]
    Auto,
    Tcp,
    Udp,
}

impl NfsTransport {
    fn option(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Tcp => Some("tcp"),
            Self::Udp => Some("udp"),
        }
    }
}

/// Beschreibung eines einzuhängenden NFS-Laufwerks.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NfsSpec {
    pub host: String,
    /// Freigegebener Pfad auf dem Server, etwa `/export/daten`.
    pub path: String,
    #[serde(default)]
    pub version: NfsVersion,
    #[serde(default)]
    pub security: NfsSecurity,
    /// Kerberos-Bereich. Nur nötig, wenn mehrere Zugänge vorliegen.
    #[serde(default)]
    pub realm: String,
    #[serde(default)]
    pub transport: NfsTransport,
    /// Schaltet Dateisperren ab. Nötig bei Servern ohne `rpc.statd`, bei denen
    /// Zugriffe sonst hängen bleiben.
    #[serde(default)]
    pub no_locks: bool,
    #[serde(default)]
    pub label: String,
    /// Bestätigung des Benutzers, dass die Inhalte unverschlüsselt übertragen
    /// werden dürfen.
    #[serde(default)]
    pub allow_insecure: bool,
}

impl NfsSpec {
    /// Kennung für die Anzeige und zum Wiedererkennen des Laufwerks.
    fn descriptor(&self) -> String {
        format!("nfs://{}{}", self.host, self.path)
    }

    fn display_label(&self) -> String {
        remote::sanitize_label(&self.label)
            .or_else(|| {
                self.path
                    .rsplit('/')
                    .find(|part| !part.is_empty())
                    .and_then(remote::sanitize_label)
            })
            .or_else(|| remote::sanitize_label(&self.host))
            .unwrap_or_else(|| "NFS".to_string())
    }

    /// Prüft die Angaben, bevor irgendetwas ausgeführt wird.
    fn validate(&self) -> Result<(), String> {
        if !remote::valid_host(&self.host) {
            return Err("err.nfs.host".to_string());
        }
        if !self.path.starts_with('/') || !remote::valid_remote_path(&self.path) {
            return Err("err.nfs.path".to_string());
        }
        if !self.realm.is_empty() {
            if !self.security.uses_kerberos() {
                return Err("err.nfs.realmWithoutKerberos".to_string());
            }
            let ok = self.realm.len() <= 255
                && self
                    .realm
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'));
            if !ok {
                return Err("err.nfs.realm".to_string());
            }
        }

        // Fassung 2 kann nur UDP; Fassung 4 kann nur TCP.
        if self.version == NfsVersion::V2 && self.transport == NfsTransport::Tcp {
            return Err("err.nfs.v2NeedsUdp".to_string());
        }
        if matches!(self.version, NfsVersion::V4 | NfsVersion::V41)
            && self.transport == NfsTransport::Udp
        {
            return Err("err.nfs.v4NeedsTcp".to_string());
        }

        // Verschlüsselt der Mount die Inhalte, ist kein weiterer Nachweis nötig.
        if self.security.encrypts() {
            return Ok(());
        }
        if !self.allow_insecure {
            return Err("err.nfs.insecureNotConfirmed".to_string());
        }
        // Kerberos schützt die Anmeldung auch ohne Verschlüsselung der Inhalte
        // kryptografisch. Nur bei `sys` und bei ausgehandelter Sicherheit bleibt
        // es bei der Beschränkung auf das eigene Netz.
        if self.security.uses_kerberos() {
            return Ok(());
        }
        match self
            .host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<IpAddr>()
        {
            Ok(ip) if crate::is_local_network_address(ip) => Ok(()),
            Ok(_) => Err("err.nfs.remoteNeedsKerberos".to_string()),
            Err(_) => Err("err.nfs.hostnameNeedsKerberos".to_string()),
        }
    }

    /// Baut die Optionsliste für `mount -t nfs -o …`.
    fn mount_options(&self) -> String {
        // `noresvport` steht immer voran: Ohne Administratorrechte lässt sich
        // kein privilegierter Port belegen.
        let mut parts = vec!["noresvport".to_string()];
        for opt in [
            self.version.option(),
            self.security.option(),
            self.transport.option(),
        ]
        .into_iter()
        .flatten()
        {
            parts.push(opt.to_string());
        }
        if self.no_locks {
            parts.push("nolocks".to_string());
        }
        if !self.realm.is_empty() {
            parts.push(format!("realm={}", self.realm));
        }
        parts.join(",")
    }

    /// Ziel in der Schreibweise von `mount_nfs`. IPv6-Adressen gehören in
    /// eckige Klammern.
    fn target(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        format!("{host}:{}", self.path)
    }
}

/// Übersetzt die Meldung von `mount_nfs` in einen Sprachschlüssel.
///
/// Die Systemmeldungen sind für sich genommen nichtssagend. „Operation not
/// permitted“ etwa bedeutet hier stets, dass der Server einen privilegierten
/// Port verlangt — ein Umstand, der sich nur auf dem Server beheben lässt.
fn explain_failure(output: &str) -> String {
    let text = output.to_ascii_lowercase();
    if text.contains("operation not permitted") {
        "err.nfs.privilegedPort"
    } else if text.contains("rpc prog. not avail") || text.contains("rpc prog not avail") {
        "err.nfs.versionUnavailable"
    } else if text.contains("permission denied") || text.contains("access denied") {
        "err.nfs.notExported"
    } else if text.contains("authentication error") {
        "err.nfs.authentication"
    } else if text.contains("connection reset") || text.contains("rpc timed out") {
        "err.nfs.transport"
    } else if text.contains("no such file or directory") {
        "err.nfs.noSuchExport"
    } else if text.contains("can't resolve") || text.contains("unknown host") {
        "err.nfs.unknownHost"
    } else if text.contains("timed out") || text.contains("connection refused") {
        "err.nfs.unreachable"
    } else {
        return format!("err.nfs.generic\u{1f}{}", output.trim());
    }
    .to_string()
}

/// Hängt ein NFS-Laufwerk ein und trägt es in die Liste der Netzlaufwerke ein.
pub fn mount(spec: NfsSpec) -> Result<remote::RemoteMountInfo, String> {
    spec.validate()?;

    let label = spec.display_label();
    let (dir, unique) = remote::unique_mount_dir(&label)?;

    let options = spec.mount_options();
    let target = spec.target();

    let mut child = Command::new("/sbin/mount")
        .args(["-t", "nfs", "-o", &options, &target])
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("err.nfs.spawn\u{1f}{err}"))?;

    // `mount_nfs` kann bei einem stillen Server sehr lange warten. Nach Ablauf
    // der Frist wird der Vorgang abgebrochen.
    let deadline = Instant::now() + MOUNT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                let _ = std::fs::remove_dir(&dir);
                return Err(format!("err.nfs.spawn\u{1f}{err}"));
            }
        }
    };

    let Some(status) = status else {
        let _ = std::fs::remove_dir(&dir);
        return Err("err.nfs.timeout".to_string());
    };

    let mut message = String::new();
    if let Some(mut err) = child.stderr.take() {
        use std::io::Read;
        let _ = err.read_to_string(&mut message);
    }

    if !status.success() || !remote::is_mount_point(&dir) {
        let _ = std::fs::remove_dir(&dir);
        return Err(explain_failure(&message));
    }

    // Ein NFS-Mount gelingt auch dann, wenn der Server anschließend jeden
    // Zugriff verweigert. Der Ordner erschien in der Oberfläche dann als leeres
    // Laufwerk, ohne jede Begründung. Deshalb wird sofort einmal gelesen.
    if let Err(err) = std::fs::read_dir(&dir) {
        let reason = access_denial_reason(&dir, &err);
        let _ = remote::unmount_owned(&dir);
        let _ = std::fs::remove_dir(&dir);
        return Err(reason);
    }

    let path = dir.to_string_lossy().to_string();
    let descriptor = spec.descriptor();
    remote::register_plain_mount(dir, unique.clone(), descriptor.clone());
    Ok(remote::RemoteMountInfo {
        path,
        home_path: None,
        label: unique,
        descriptor,
    })
}

/// Begründet, warum eine eingehängte Freigabe nicht lesbar ist.
///
/// Der häufigste Fall ist kein Netz- oder Anmeldefehler, sondern eine schlichte
/// Rechtefrage: Bei AUTH_SYS meldet der Mac seine eigene Benutzernummer. Gehört
/// der freigegebene Ordner einem anderen Benutzer und erlaubt er Fremden nichts,
/// bleibt er verschlossen. Die Nummern stehen deshalb in der Meldung — ohne sie
/// ist der Fall vom Anwender nicht zu klären.
fn access_denial_reason(dir: &Path, err: &std::io::Error) -> String {
    if err.kind() != std::io::ErrorKind::PermissionDenied {
        return format!("err.nfs.unreadable\u{1f}{err}");
    }
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(dir) else {
        return "err.nfs.noAccess".to_string();
    };
    // Nur die neun Rechtebits, oktal — so, wie sie auch `chmod` erwartet.
    let mode = meta.mode() & 0o777;
    format!(
        "err.nfs.noAccessDetails\u{1f}{}\u{1f}{}\u{1f}{:o}\u{1f}{}",
        meta.uid(),
        meta.gid(),
        mode,
        unsafe { libc::getuid() }
    )
}

/// Tauri-Befehl: hängt ein NFS-Laufwerk ein und liefert seinen Pfad.
#[tauri::command]
pub async fn mount_nfs(spec: NfsSpec) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || mount(spec).map(|info| info.path))
        .await
        .map_err(|_| "err.nfs.timeout".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> NfsSpec {
        NfsSpec {
            host: "127.0.0.1".to_string(),
            path: "/export/daten".to_string(),
            version: NfsVersion::Auto,
            security: NfsSecurity::Auto,
            realm: String::new(),
            transport: NfsTransport::Auto,
            no_locks: false,
            label: String::new(),
            allow_insecure: true,
        }
    }

    #[test]
    fn noresvport_steht_immer_in_den_optionen() {
        // Ohne Administratorrechte ist kein privilegierter Port belegbar.
        assert_eq!(spec().mount_options(), "noresvport");
    }

    #[test]
    fn optionen_werden_zusammengesetzt() {
        let mut s = spec();
        s.version = NfsVersion::V41;
        s.security = NfsSecurity::Krb5p;
        s.transport = NfsTransport::Tcp;
        s.no_locks = true;
        s.realm = "BEISPIEL.DE".to_string();
        assert_eq!(
            s.mount_options(),
            "noresvport,vers=4.1,sec=krb5p,tcp,nolocks,realm=BEISPIEL.DE"
        );
    }

    #[test]
    fn ipv6_bekommt_klammern() {
        let mut s = spec();
        s.host = "fe80::1".to_string();
        assert_eq!(s.target(), "[fe80::1]:/export/daten");
        s.host = "[fe80::1]".to_string();
        assert_eq!(s.target(), "[fe80::1]:/export/daten");
    }

    #[test]
    fn unbestaetigt_unverschluesselt_wird_abgelehnt() {
        let mut s = spec();
        s.allow_insecure = false;
        assert_eq!(s.validate(), Err("err.nfs.insecureNotConfirmed".into()));
    }

    #[test]
    fn krb5p_braucht_keine_bestaetigung() {
        let mut s = spec();
        s.allow_insecure = false;
        s.security = NfsSecurity::Krb5p;
        s.host = "server.beispiel.de".to_string();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn kerberos_erlaubt_fremde_server() {
        let mut s = spec();
        s.security = NfsSecurity::Krb5i;
        s.host = "server.beispiel.de".to_string();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn ohne_kerberos_nur_im_eigenen_netz() {
        let mut s = spec();
        s.host = "server.beispiel.de".to_string();
        assert_eq!(s.validate(), Err("err.nfs.hostnameNeedsKerberos".into()));
        s.host = "93.184.216.34".to_string();
        assert_eq!(s.validate(), Err("err.nfs.remoteNeedsKerberos".into()));
        s.host = "192.168.1.10".to_string();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn bereich_nur_mit_kerberos() {
        let mut s = spec();
        s.realm = "BEISPIEL.DE".to_string();
        assert_eq!(s.validate(), Err("err.nfs.realmWithoutKerberos".into()));
    }

    #[test]
    fn unvereinbare_fassung_und_uebertragung() {
        let mut s = spec();
        s.version = NfsVersion::V2;
        s.transport = NfsTransport::Tcp;
        assert_eq!(s.validate(), Err("err.nfs.v2NeedsUdp".into()));
        let mut s = spec();
        s.version = NfsVersion::V4;
        s.transport = NfsTransport::Udp;
        assert_eq!(s.validate(), Err("err.nfs.v4NeedsTcp".into()));
    }

    #[test]
    fn pfad_muss_absolut_sein() {
        let mut s = spec();
        s.path = "export/daten".to_string();
        assert_eq!(s.validate(), Err("err.nfs.path".into()));
    }

    #[test]
    fn systemmeldungen_werden_uebersetzt() {
        // Genau diese Meldungen wurden gegen einen echten Server gemessen.
        assert_eq!(
            explain_failure("mount_nfs: … : Operation not permitted"),
            "err.nfs.privilegedPort"
        );
        assert_eq!(
            explain_failure("mount_nfs: … : RPC prog. not avail"),
            "err.nfs.versionUnavailable"
        );
        assert_eq!(
            explain_failure("mount_nfs: … : Permission denied"),
            "err.nfs.notExported"
        );
        assert_eq!(
            explain_failure("mount_nfs: … : Authentication error"),
            "err.nfs.authentication"
        );
        assert_eq!(
            explain_failure("mount_nfs: … : Connection reset by peer"),
            "err.nfs.transport"
        );
    }

    /// Hängt eine echte Freigabe ein, liest, schreibt und hängt wieder aus.
    ///
    /// Läuft nur auf Anforderung, weil dafür ein NFS-Server bereitstehen muss:
    /// `cargo test --lib nfs::tests::echter_mount -- --ignored --nocapture`
    /// Vorbereitung siehe Kopf dieser Datei; die Freigabe muss `insecure`
    /// erlauben, da ohne Administratorrechte eingehängt wird.
    #[test]
    #[ignore = "benötigt einen laufenden NFS-Server"]
    fn echter_mount() {
        let mut s = spec();
        s.path = "/Users/Shared/dualbeam-nfs-test".to_string();
        s.version = NfsVersion::V3;

        let info = mount(s).expect("Einhängen fehlgeschlagen");
        let dir = std::path::Path::new(&info.path);
        assert!(remote::is_mount_point(dir), "kein Mountpunkt");

        // Lesen
        let gelesen = std::fs::read(dir.join("probe.bin")).expect("Lesen fehlgeschlagen");
        assert_eq!(gelesen.len(), 524_288, "unerwartete Größe");

        // Schreiben und wieder entfernen
        let ziel = dir.join("rust-probe.tmp");
        std::fs::write(&ziel, b"DualBeam").expect("Schreiben fehlgeschlagen");
        assert_eq!(std::fs::read(&ziel).unwrap(), b"DualBeam");
        std::fs::remove_file(&ziel).expect("Löschen fehlgeschlagen");

        // Das Laufwerk muss in der gemeinsamen Liste stehen.
        assert!(
            remote::active_mounts().iter().any(|m| m.path == info.path),
            "nicht in der Liste der Netzlaufwerke"
        );

        remote::unmount_owned(dir).expect("Aushängen fehlgeschlagen");
        assert!(!remote::is_mount_point(dir), "noch eingehängt");
    }

    #[test]
    fn name_faellt_auf_den_freigabeordner_zurueck() {
        assert_eq!(spec().display_label(), "daten");
    }
}
