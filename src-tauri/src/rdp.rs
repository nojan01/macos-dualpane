//! Brücke zu RemoteDeskRDP: gespeicherte RDP-Verbindungen anzeigen und öffnen.
//!
//! DualBeam baut selbst keine RDP-Sitzung auf. Das Zusammensetzen der
//! FreeRDP-Argumente – Gateway, Ordnerfreigaben, Auflösung, Zertifikate – steckt
//! vollständig in RemoteDeskRDP und wird hier bewusst nicht nachgebaut. Ein
//! Klick reicht die gewünschte Verbindung nur weiter:
//!
//! ```text
//! remotedesk://connect?id=<uuid>
//! ```
//!
//! macOS startet die App dabei selbst, falls sie noch nicht läuft. Läuft sie
//! schon, bekommt sie die Anforderung als Ereignis – in beiden Fällen entsteht
//! genau ein Fenster.
//!
//! Kennwörter kommen hier nie vor. Sie stehen im Schlüsselbund und werden
//! ausschließlich von RemoteDeskRDP selbst gelesen.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Bundle-Kennung von RemoteDeskRDP.
const BUNDLE_ID: &str = "com.nojan.remotedesk";

/// Ordner, in dem RemoteDeskRDP seine Profile ablegt (unter
/// `~/Library/Application Support`).
const PROFILE_DIRECTORY: &str = "RemoteDesk";
const PROFILE_FILE: &str = "profiles.json";

/// Eine Verbindung, wie sie die Seitenleiste braucht. Bewusst nur diese drei
/// Felder: alles Weitere wertet RemoteDeskRDP aus, DualBeam geht es nichts an.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RdpProfile {
    pub id: String,
    pub name: String,
    pub host: String,
}

fn profile_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join(PROFILE_DIRECTORY).join(PROFILE_FILE))
}

/// Sucht das App-Bundle.
///
/// Erst die üblichen Orte, denn das kostet nur einen Blick auf die Platte.
/// Nur wenn dort nichts liegt, fragt Spotlight – das findet die App auch an
/// einem ungewöhnlichen Ort, startet aber einen Prozess.
fn locate_app() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/Applications/RemoteDeskRDP.app")];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications/RemoteDeskRDP.app"));
    }
    if let Some(found) = candidates.into_iter().find(|path| path.is_dir()) {
        return Some(found);
    }
    let query = format!("kMDItemCFBundleIdentifier == '{BUNDLE_ID}'");
    let output = Command::new("mdfind").arg(&query).output().ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .find(|path| path.is_dir())
}

/// Wie `locate_app`, aber mit kurzem Gedächtnis.
///
/// Die Seitenleiste fragt bei jedem Fensterwechsel nach. Eine Installation
/// ändert sich selten, ein Spotlight-Aufruf kostet aber jedes Mal einen
/// Prozessstart. Nach einer halben Minute wird erneut nachgesehen, damit eine
/// frisch installierte App nicht bis zum Programmneustart unsichtbar bleibt.
fn find_app() -> Option<PathBuf> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Option<PathBuf>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let Ok(mut slot) = cache.lock() else {
        return locate_app();
    };
    if let Some((measured, ref result)) = *slot {
        if measured.elapsed() < Duration::from_secs(30) {
            return result.clone();
        }
    }
    let result = locate_app();
    *slot = Some((Instant::now(), result.clone()));
    result
}

/// Liest die Profile aus der Datei von RemoteDeskRDP.
///
/// Fehlertolerant mit Absicht: Eine fehlende, leere oder beschädigte Datei
/// ergibt eine leere Liste. Ein fremdes Programm darf DualBeams Seitenleiste
/// nicht durch eine kaputte Datei lahmlegen. Einträge ohne Kennung oder Namen
/// werden übergangen, weil sie nicht anklickbar wären.
pub fn read_profiles() -> Vec<RdpProfile> {
    let Some(path) = profile_path() else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&data) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            // Objekt-Speicherprofile gehören zu DualBeam/Netzwerk, nicht in
            // die Liste der startbaren RemoteDesk-Sitzungen. Die anderen
            // Protokolle bleiben absichtlich erhalten: RemoteDeskRDP kann
            // neben RDP auch VNC, SSH, SFTP und Mosh über denselben Deep Link
            // starten. Alte Profile ohne `protocol` gelten als RDP.
            if matches!(
                item.get("protocol").and_then(|value| value.as_str()),
                Some("s3" | "swift")
            ) {
                return None;
            }
            let id = item.get("id")?.as_str()?.trim().to_string();
            let name = item.get("name")?.as_str()?.trim().to_string();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            let host = item
                .get("host")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            Some(RdpProfile { id, name, host })
        })
        .collect()
}

/// Kodiert die Kennung für die Abfragezeichenkette.
///
/// Eine Kennung ist normalerweise eine UUID, aber die Datei stammt aus einem
/// fremden Programm. Ein `&` oder `#` darin würde die URL sonst zerlegen und
/// eine andere Verbindung öffnen als angeklickt.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Baut die Adresse für eine bekannte Kennung.
///
/// Gibt `None` zurück, wenn die Kennung nicht in der übergebenen Liste steht.
/// Damit lässt sich über diesen Weg nichts anderes starten als das, was auch
/// in der Seitenleiste steht.
fn link_for(id: &str, known: &[RdpProfile]) -> Option<String> {
    if !known.iter().any(|profile| profile.id == id) {
        return None;
    }
    Some(format!("remotedesk://connect?id={}", encode(id)))
}

/// Ist RemoteDeskRDP nutzbar? Nur wenn die App vorhanden ist *und* mindestens
/// eine Verbindung eingerichtet wurde – ein leerer Abschnitt in der
/// Seitenleiste wäre nur im Weg.
#[tauri::command]
pub fn rdp_available() -> bool {
    find_app().is_some() && !read_profiles().is_empty()
}

#[tauri::command]
pub fn rdp_profiles() -> Vec<RdpProfile> {
    if find_app().is_none() {
        return Vec::new();
    }
    read_profiles()
}

#[tauri::command]
pub fn rdp_connect(id: String) -> Result<(), String> {
    let known = read_profiles();
    let link = link_for(&id, &known).ok_or("err.rdp.unknown")?;
    Command::new("open")
        .arg(&link)
        .status()
        .map_err(|e| format!("err.rdp.open\u{1f}{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str) -> RdpProfile {
        RdpProfile { id: id.into(), name: "X".into(), host: "h".into() }
}
    /// Die Profildatei stammt aus einem fremden Programm. Was hier nicht
    /// abgefangen wird, legt die Seitenleiste lahm.
    #[test]
    fn broken_profile_files_yield_no_entries() {
        let parse = |raw: &str| {
            serde_json::from_str::<Vec<serde_json::Value>>(raw)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            let id = item.get("id")?.as_str()?.trim().to_string();
                            let name = item.get("name")?.as_str()?.trim().to_string();
                            if id.is_empty() || name.is_empty() {
                                return None;
                            }
                            Some(id + "|" + &name)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        assert!(parse("das ist kein JSON").is_empty());
        assert!(parse("{}").is_empty());
        assert!(parse("[]").is_empty());
        // Eintrag ohne Namen faellt raus, der gueltige bleibt.
        assert_eq!(
            parse(r#"[{"id":"a"},{"id":"b","name":" B "},{"name":"ohne id"}]"#),
            vec!["b|B"]
        );
        // Leere Werte zaehlen wie fehlende.
        assert!(parse(r#"[{"id":"  ","name":"X"},{"id":"y","name":"  "}]"#).is_empty());
    }

    /// Sonderzeichen in der Kennung duerfen die Adresse nicht zerlegen.
    #[test]
    fn ids_are_percent_encoded() {
        assert_eq!(encode("2ae2238f-1c4d"), "2ae2238f-1c4d");
        assert_eq!(encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("x#y?z"), "x%23y%3Fz");
        // Mehrbyte-Zeichen werden byteweise kodiert.
        assert_eq!(encode("ä"), "%C3%A4");
    }

    /// Es darf nur geoeffnet werden, was auch in der Liste steht.
    #[test]
    fn only_known_ids_produce_a_link() {
        let known = vec![profile("abc"), profile("d&e")];
        assert_eq!(
            link_for("abc", &known).as_deref(),
            Some("remotedesk://connect?id=abc")
        );
        assert_eq!(
            link_for("d&e", &known).as_deref(),
            Some("remotedesk://connect?id=d%26e")
        );
        assert_eq!(link_for("fremd", &known), None);
        assert_eq!(link_for("", &known), None);
        assert_eq!(link_for("abc", &[]), None);
    }
}
