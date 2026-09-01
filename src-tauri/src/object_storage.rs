//! S3- und OpenStack-Swift-Profile für DualBeam.
//!
//! Die bereits in RemoteDeskRDP bewährte Profilform wird übernommen. Im
//! Unterschied zum früheren Objekt-Browser hängt DualBeam die Ziele jedoch
//! über das mitgelieferte rclone ein. Damit stehen sie dem Dateimanager und
//! der vorhandenen Dateisystem-Synchronisation genau wie WebDAV zur Verfügung.

use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "com.nojan.dualbeam.object-storage";
/// Nur für die einmalige, verlustfreie Übernahme vorhandener RemoteDeskRDP-
/// Profile. Neue Geheimnisse liegen ausschließlich im DualBeam-Dienst.
const LEGACY_KEYCHAIN_SERVICE: &str = "com.nojan.remotedesk.object-storage";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObjectStorageProtocol {
    S3,
    Swift,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SwiftAuthVersion {
    V2,
    V3,
}

fn default_swift_v3() -> SwiftAuthVersion {
    SwiftAuthVersion::V3
}

fn default_parallel_transfers() -> u8 {
    1
}

/// Gespeicherte Verbindungsdaten ohne Secret Access Key bzw. Swift-Kennwort.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectStorageProfile {
    pub id: String,
    pub name: String,
    pub protocol: ObjectStorageProtocol,
    pub endpoint: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub container: String,
    #[serde(default = "default_path_style")]
    pub path_style: bool,
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub swift_project: String,
    #[serde(default)]
    pub swift_user_domain: String,
    #[serde(default)]
    pub swift_project_domain: String,
    #[serde(default = "default_identity_path")]
    pub swift_identity_path: String,
    #[serde(default = "default_swift_v3")]
    pub swift_auth_version: SwiftAuthVersion,
    /// Parallelität bleibt absichtlich auf die zwei UI-Optionen begrenzt. Zu
    /// viele gleichzeitige rclone-Transfers können WebDAV-Ziele überfordern.
    #[serde(default = "default_parallel_transfers")]
    pub parallel_transfers: u8,
}

fn default_path_style() -> bool {
    true
}
fn default_identity_path() -> String {
    "/identity/v3".to_string()
}

fn valid_profile_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub fn validate(profile: &ObjectStorageProfile) -> Result<(), String> {
    if !valid_profile_id(&profile.id) || profile.name.trim().is_empty() {
        return Err("err.objectStorage.profile".into());
    }
    let endpoint = url::Url::parse(profile.endpoint.trim())
        .map_err(|_| "err.objectStorage.endpoint".to_string())?;
    if !matches!(endpoint.scheme(), "https" | "http") || endpoint.host_str().is_none() {
        return Err("err.objectStorage.endpoint".into());
    }
    if profile.container.contains(['\\', '\n', '\r'])
        || profile.container.split('/').any(|part| part == "..")
    {
        return Err("err.objectStorage.container".into());
    }
    if !matches!(profile.parallel_transfers, 1 | 4) {
        return Err("err.objectStorage.profile".into());
    }
    match profile.protocol {
        ObjectStorageProtocol::S3
            if profile.access_key.trim().is_empty() || profile.region.trim().is_empty() =>
        {
            Err("err.objectStorage.s3Credentials".into())
        }
        ObjectStorageProtocol::Swift
            if profile.username.trim().is_empty()
                || profile.swift_project.trim().is_empty()
                || !profile.swift_identity_path.starts_with('/') =>
        {
            Err("err.objectStorage.swiftCredentials".into())
        }
        _ => Ok(()),
    }
}

#[tauri::command]
pub fn save_object_storage_secret(profile_id: String, secret: String) -> Result<(), String> {
    if !valid_profile_id(&profile_id) || secret.is_empty() || secret.contains(['\n', '\r']) {
        return Err("err.objectStorage.secret".into());
    }
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            &profile_id,
            secret.as_bytes(),
        )
        .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (profile_id, secret);
        Err("err.remote.keychainUnavailable".into())
    }
}

fn read_secret(service: &str, profile_id: &str) -> Result<Option<String>, String> {
    #[cfg(target_os = "macos")]
    {
        match security_framework::passwords::get_generic_password(service, profile_id) {
            Ok(value) => String::from_utf8(value)
                .map(Some)
                .map_err(|_| "err.objectStorage.secret".to_string()),
            Err(_) => Ok(None),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (service, profile_id);
        Ok(None)
    }
}

/// Liest zunächst den DualBeam-Eintrag. Nur bei der Migration wird der alte
/// RemoteDeskRDP-Dienst probiert und sein Wert danach in DualBeam kopiert.
pub fn object_storage_secret(profile_id: &str) -> Result<String, String> {
    if !valid_profile_id(profile_id) {
        return Err("err.objectStorage.profile".into());
    }
    if let Some(secret) = read_secret(KEYCHAIN_SERVICE, profile_id)? {
        return Ok(secret);
    }
    if let Some(secret) = read_secret(LEGACY_KEYCHAIN_SERVICE, profile_id)? {
        #[cfg(target_os = "macos")]
        let _ = security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            profile_id,
            secret.as_bytes(),
        );
        return Ok(secret);
    }
    Err("err.objectStorage.secretMissing".into())
}

/// Prüft nur das Vorhandensein des Secrets. Das Secret selbst darf nie zurück
/// an die WebView gelangen – die Oberfläche benötigt lediglich den Status, um
/// beim erneuten Bearbeiten den richtigen Hinweis anzeigen zu können.
#[tauri::command]
pub fn has_object_storage_secret(profile_id: String) -> Result<bool, String> {
    if !valid_profile_id(&profile_id) {
        return Err("err.objectStorage.profile".into());
    }
    match object_storage_secret(&profile_id) {
        Ok(_) => Ok(true),
        Err(error) if error == "err.objectStorage.secretMissing" => Ok(false),
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub fn forget_object_storage_secret(profile_id: String) -> Result<(), String> {
    if !valid_profile_id(&profile_id) {
        return Err("err.objectStorage.profile".into());
    }
    #[cfg(target_os = "macos")]
    {
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, &profile_id)
        {
            Ok(()) => {}
            Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = profile_id;
        Ok(())
    }
}

/// Nur die zu S3/Swift gehörenden Teile aus RemoteDeskRDP übernehmen.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDeskProfile {
    id: String,
    name: String,
    protocol: String,
    #[serde(default)]
    object_endpoint: String,
    #[serde(default)]
    object_region: String,
    #[serde(default)]
    object_container: String,
    #[serde(default = "default_path_style")]
    object_path_style: bool,
    #[serde(default)]
    object_access_key: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    swift_project: String,
    #[serde(default)]
    swift_user_domain: String,
    #[serde(default)]
    swift_project_domain: String,
    #[serde(default = "default_identity_path")]
    swift_identity_path: String,
    #[serde(default = "default_swift_v3")]
    swift_auth_version: SwiftAuthVersion,
}

#[tauri::command]
pub fn import_remotedesk_object_storage_profiles() -> Result<Vec<ObjectStorageProfile>, String> {
    let path = dirs::data_dir()
        .ok_or_else(|| "err.objectStorage.noAppDir".to_string())?
        .join("RemoteDesk")
        .join("profiles.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let profiles: Vec<RemoteDeskProfile> =
        serde_json::from_str(&raw).map_err(|_| "err.objectStorage.import".to_string())?;
    Ok(profiles
        .into_iter()
        .filter_map(|profile| {
            let protocol = match profile.protocol.as_str() {
                "s3" => ObjectStorageProtocol::S3,
                "swift" => ObjectStorageProtocol::Swift,
                _ => return None,
            };
            Some(ObjectStorageProfile {
                id: profile.id,
                name: profile.name,
                protocol,
                endpoint: profile.object_endpoint,
                region: profile.object_region,
                container: profile.object_container,
                path_style: profile.object_path_style,
                access_key: profile.object_access_key,
                username: profile.username,
                swift_project: profile.swift_project,
                swift_user_domain: profile.swift_user_domain,
                swift_project_domain: profile.swift_project_domain,
                swift_identity_path: profile.swift_identity_path,
                swift_auth_version: profile.swift_auth_version,
                parallel_transfers: default_parallel_transfers(),
            })
        })
        .filter(|profile| validate(profile).is_ok())
        .collect())
}

#[tauri::command]
pub async fn mount_object_storage(profile: ObjectStorageProfile) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        validate(&profile)?;
        let secret = object_storage_secret(&profile.id)?;
        crate::remote::mount_object_storage(&profile, &secret)
    })
    .await
    .map_err(|_| "err.remote.mountFailed".to_string())?
}
