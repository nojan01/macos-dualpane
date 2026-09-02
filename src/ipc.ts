import { invoke } from "@tauri-apps/api/core";
import type { Entry } from "./types";
import type { ObjectStorageProfile } from "./objectStorageProfiles";

export async function listDir(
  path: string,
  showHidden: boolean,
): Promise<Entry[]> {
  return invoke<Entry[]>("list_dir", { path, showHidden });
}

export async function openDefault(path: string): Promise<void> {
  return invoke<void>("open_default", { path });
}

export async function openPrivacySettings(): Promise<void> {
  return invoke<void>("open_privacy_settings");
}

export async function homeDir(): Promise<string> {
  return invoke<string>("home_dir");
}

export async function createDir(path: string): Promise<void> {
  return invoke<void>("create_dir", { path });
}

export async function createFile(path: string): Promise<void> {
  return invoke<void>("create_file", { path });
}

export async function renamePath(
  oldPath: string,
  newPath: string,
): Promise<void> {
  return invoke<void>("rename_path", { oldPath, newPath });
}

export async function createSymlink(
  target: string,
  linkPath: string,
): Promise<void> {
  return invoke<void>("create_symlink", { target, linkPath });
}

export async function createFinderAlias(
  target: string,
  linkPath: string,
): Promise<void> {
  return invoke<void>("create_finder_alias", { target, linkPath });
}

export async function moveToTrash(paths: string[]): Promise<void> {
  return invoke<void>("move_to_trash", { paths });
}

export type ObjectStorageDeleteTarget = {
  profile: ObjectStorageProfile;
  mountPath: string;
  /** Pfade, die in der Dateiansicht als Verzeichnisse erkannt wurden. */
  directoryPaths?: string[];
};

export type ObjectStorageTransferTarget = ObjectStorageDeleteTarget & {
  sourceIsObjectStorage: boolean;
};

export type RemoteStorageDeleteTarget = {
  spec: RemoteSpec;
  mountPath: string;
};

/** Direkter SFTP-Transfer. Ausschließlich SFTP verwendet diesen Weg; FTP und
 * FTPS behalten ihren bisherigen Dateisystem-Transfer. */
export type RemoteStorageTransferTarget = RemoteStorageDeleteTarget & {
  sourceIsRemote: boolean;
};

export async function runNetworkDelete(
  jobId: string,
  paths: string[],
  objectStorage?: ObjectStorageDeleteTarget,
  remoteStorage?: RemoteStorageDeleteTarget,
): Promise<void> {
  return invoke<void>("run_network_delete", {
    jobId, paths, objectStorage, remoteStorage,
  });
}

export type UndoDeleteItem = { original: string; staged: string };
export type UndoDeleteBatch = { token: string; items: UndoDeleteItem[] };

export async function stageDeleteForUndo(
  paths: string[],
): Promise<UndoDeleteBatch> {
  return invoke<UndoDeleteBatch>("stage_delete_for_undo", { paths });
}

export async function undoStagedDelete(items: UndoDeleteItem[]): Promise<void> {
  return invoke<void>("undo_staged_delete", { items });
}

export async function finalizeStagedDelete(
  items: UndoDeleteItem[],
): Promise<void> {
  return invoke<void>("finalize_staged_delete", { items });
}

export async function cleanupExpiredUndo(): Promise<void> {
  return invoke<void>("cleanup_expired_undo");
}

export async function forceDeleteAdmin(paths: string[]): Promise<void> {
  return invoke<void>("force_delete_admin", { paths });
}

export async function pathExists(path: string): Promise<boolean> {
  return invoke<boolean>("path_exists", { path });
}

/** Sichtbare Obergrenze eines WebDAV-, S3/Swift- bzw. SFTP-Laufwerks. */
export async function navigationRoot(path: string): Promise<string | null> {
  return invoke<string | null>("navigation_root", { path });
}

export async function pathIsNetwork(path: string): Promise<boolean> {
  return invoke<boolean>("path_is_network", { path });
}

export type Volume = {
  name: string;
  path: string;
  /** „remote“ steht für die von DualBeam selbst eingehängten Ziele (SFTP, FTPS). */
  kind: "local" | "network" | "remote";
};

export async function listVolumes(): Promise<Volume[]> {
  return invoke<Volume[]>("list_volumes");
}

export async function ejectVolume(path: string): Promise<void> {
  return invoke<void>("eject_volume", { path });
}

export type NetworkBookmark = {
  name: string;
  url: string;
  mountPath: string;
  connected: boolean;
};

export async function listNetworkBookmarks(): Promise<NetworkBookmark[]> {
  return invoke<NetworkBookmark[]>("list_network_bookmarks");
}

export async function removeNetworkBookmark(url: string): Promise<void> {
  return invoke<void>("remove_network_bookmark", { url });
}

export async function rememberNetworkVolume(path: string): Promise<void> {
  return invoke<void>("remember_network_volume", { path });
}

export async function mountNetworkUrl(
  url: string,
  allowInsecureLocal = false,
): Promise<string> {
  return invoke<string>("mount_network_url", { url, allowInsecureLocal });
}

export async function saveObjectStorageSecret(profileId: string, secret: string): Promise<void> {
  return invoke<void>("save_object_storage_secret", { profileId, secret });
}

export async function hasObjectStorageSecret(profileId: string): Promise<boolean> {
  return invoke<boolean>("has_object_storage_secret", { profileId });
}

export async function forgetObjectStorageSecret(profileId: string): Promise<void> {
  return invoke<void>("forget_object_storage_secret", { profileId });
}

/** Hängt ein S3-Bucket bzw. einen Swift-Container über das mitgelieferte
 * rclone ein. Danach ist er ein normaler Dateisystempfad für Sync-Profile. */
export async function mountObjectStorage(profile: ObjectStorageProfile): Promise<string> {
  return invoke<string>("mount_object_storage", { profile });
}

export async function importRemoteDeskObjectStorageProfiles(): Promise<ObjectStorageProfile[]> {
  return invoke<ObjectStorageProfile[]>("import_remotedesk_object_storage_profiles");
}

/** Protokolle, die DualBeam über das mitgelieferte rclone einhängt. */
export type RemoteProtocol =
  | "sftp"
  | "ftp"
  | "ftpsExplicit"
  | "ftpsImplicit"
  | "smb";

export type RemoteSpec = {
  protocol: RemoteProtocol;
  host: string;
  /** Leer lassen für den Standardport des Protokolls. */
  port?: number | null;
  username: string;
  /** Pfad auf dem Server. Leer bedeutet die Wurzel des Zugangs. */
  path: string;
  /** Anzeigename. Leer bedeutet: aus dem Rechnernamen ableiten. */
  label: string;
  /** Windows-Domäne oder Arbeitsgruppe. Nur bei SMB gefüllt. */
  domain?: string;
};

export type HostKeyReport = {
  host: string;
  port: number;
  /** Fingerabdrücke in der Form „ED25519 SHA256:…“. */
  fingerprints: string[];
  trusted: boolean;
};

export type RemoteMount = {
  path: string;
  homePath?: string | null;
  label: string;
  descriptor: string;
};

export async function remoteHostKeys(
  host: string,
  port?: number | null,
): Promise<HostKeyReport> {
  return invoke<HostKeyReport>("remote_host_keys", { host, port: port ?? null });
}

export async function remoteTrustHost(
  host: string,
  port?: number | null,
): Promise<void> {
  return invoke<void>("remote_trust_host", { host, port: port ?? null });
}

export async function saveRemotePassword(
  spec: RemoteSpec,
  password: string,
): Promise<void> {
  return invoke<void>("save_remote_password", { spec, password });
}

export async function loadRemotePassword(
  spec: RemoteSpec,
): Promise<string | null> {
  return invoke<string | null>("load_remote_password", { spec });
}

/** Hängt das Ziel ein und liefert den lokalen Pfad des neuen Ordners. */
export async function mountRemote(
  spec: RemoteSpec,
  password: string,
  allowInsecure = false,
): Promise<string> {
  return invoke<string>("mount_remote", { spec, password, allowInsecure });
}

/** Fassung des NFS-Protokolls. macOS beherrscht höchstens 4.1. */
export type NfsVersion = "auto" | "v2" | "v3" | "v4" | "v41";

/** Sicherheitsverfahren. macOS kennt ausschließlich diese vier. */
export type NfsSecurity = "auto" | "sys" | "krb5" | "krb5i" | "krb5p";

export type NfsTransport = "auto" | "tcp" | "udp";

export type NfsSpec = {
  host: string;
  path: string;
  version: NfsVersion;
  security: NfsSecurity;
  /** Kerberos-Bereich; nur bei mehreren Zugängen nötig. */
  realm: string;
  transport: NfsTransport;
  /** Für Server ohne `rpc.statd`, bei denen Zugriffe sonst hängen bleiben. */
  noLocks: boolean;
  label: string;
  allowInsecure: boolean;
};

/** Hängt eine NFS-Freigabe ein und liefert den lokalen Pfad. */
export async function mountNfs(spec: NfsSpec): Promise<string> {
  return invoke<string>("mount_nfs", { spec });
}

export async function unmountRemote(path: string): Promise<void> {
  return invoke<void>("unmount_remote", { path });
}

export async function remoteMounts(): Promise<RemoteMount[]> {
  return invoke<RemoteMount[]>("remote_mounts");
}

export async function appVersion(): Promise<string> {
  return invoke<string>("app_version");
}

/** Startet die Anwendung neu, etwa nach einem eingespielten Update. */
export async function restartApplication(): Promise<void> {
  return invoke<void>("restart_application");
}

export async function setMenuLanguage(lang: string): Promise<void> {
  return invoke<void>("set_menu_language", { lang });
}

export async function mountDmg(path: string): Promise<string> {
  return invoke<string>("mount_dmg", { path });
}

export async function findDmgMount(path: string): Promise<string | null> {
  return invoke<string | null>("find_dmg_mount", { path });
}

export async function detachDmg(path: string): Promise<void> {
  return invoke<void>("detach_dmg", { path });
}

export async function quickLook(path: string): Promise<void> {
  return invoke<void>("quick_look", { path });
}

export type JobItem = { src: string; dst: string; overwrite: boolean };
export type JobKind = "copy" | "move";

export async function checkConflicts(items: JobItem[]): Promise<string[]> {
  return invoke<string[]>("check_conflicts", { items });
}

export async function runJob(
  jobId: string,
  kind: JobKind,
  items: JobItem[],
  objectStorage?: ObjectStorageTransferTarget,
  remoteStorage?: RemoteStorageTransferTarget,
): Promise<void> {
  return invoke<void>("run_job", {
    jobId, kind, items, objectStorage, remoteStorage,
  });
}

export async function cancelJob(jobId: string): Promise<void> {
  return invoke<void>("cancel_job", { jobId });
}

export type SyncAction =
  "copy" | "update" | "delete" | "left_to_right" | "right_to_left" | "conflict";
export type SyncEntry = {
  rel: string;
  action: SyncAction;
  isDir: boolean;
  size: number;
};

export async function syncPreview(
  previewId: string,
  src: string,
  dst: string,
  deleteExtra: boolean,
  ignorePatterns: string[] = [],
  verifyChecksums = false,
  maxFileSize = 0,
): Promise<SyncEntry[]> {
  return invoke<SyncEntry[]>("sync_preview", {
    previewId,
    src,
    dst,
    deleteExtra,
    ignorePatterns,
    verifyChecksums,
    maxFileSize,
  });
}

export async function syncTwoWayPreview(
  previewId: string,
  left: string,
  right: string,
  ignorePatterns: string[] = [],
  verifyChecksums = false,
  maxFileSize = 0,
): Promise<SyncEntry[]> {
  return invoke<SyncEntry[]>("sync_two_way_preview", {
    previewId,
    left,
    right,
    ignorePatterns,
    verifyChecksums,
    maxFileSize,
  });
}

export type JobProgress = {
  jobId: string;
  done: number;
  total: number;
  filesDone: number;
  /** Reeller Byte-Fortschritt eines direkten SFTP-Uploads (0…100). */
  transferPercent?: number;
  /** Serveroperation läuft, ihre Einzelobjekte sind jedoch nicht zählbar. */
  indeterminate: boolean;
  current: string;
  finished: boolean;
  cancelled: boolean;
  error: string | null;
};

export async function watchPath(paneId: string, path: string): Promise<void> {
  return invoke<void>("watch_path", { paneId, path });
}

export async function unwatchPane(paneId: string): Promise<void> {
  return invoke<void>("unwatch_pane", { paneId });
}

export async function searchInDir(
  root: string,
  query: string,
  showHidden: boolean,
  maxResults = 1000,
): Promise<Entry[]> {
  return invoke<Entry[]>("search_in_dir", {
    root,
    query,
    showHidden,
    maxResults,
  });
}

export async function zipCreate(srcs: string[], dst: string): Promise<void> {
  return invoke<void>("zip_create", { srcs, dst });
}

export async function zipExtract(src: string, dstDir: string): Promise<void> {
  return invoke<void>("zip_extract", { src, dstDir });
}

export type Favorite = { name: string; icon: string; path: string };

export async function loadFavorites(): Promise<Favorite[]> {
  return invoke<Favorite[]>("load_favorites");
}

export async function saveFavorites(favs: Favorite[]): Promise<void> {
  return invoke<void>("save_favorites", { favs });
}

export type PreviewInfo = {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  mtime: number;
  ext: string;
  kind: "text" | "image" | "dir" | "binary" | "other";
};

export async function previewInfo(path: string): Promise<PreviewInfo> {
  return invoke<PreviewInfo>("preview_info", { path });
}

/** Maximale Anzahl Bytes, die für die Textvorschau gelesen werden. */
export const TEXT_PREVIEW_MAX_BYTES = 65536;
/** Kantenlänge (px) für Bild-Thumbnails in der Vorschau. */
export const IMAGE_THUMB_SIZE = 256;

export async function readTextPreview(
  path: string,
  maxBytes = TEXT_PREVIEW_MAX_BYTES,
): Promise<string> {
  return invoke<string>("read_text_preview", { path, maxBytes });
}

export async function readImageThumb(
  path: string,
  size = IMAGE_THUMB_SIZE,
): Promise<string> {
  return invoke<string>("read_image_thumb", { path, size });
}

export async function readFileIcon(path: string, size = 32): Promise<string> {
  return invoke<string>("read_file_icon", { path, size });
}

export async function openTerminal(path: string): Promise<void> {
  return invoke<void>("open_terminal", { path });
}

export async function openInEditor(path: string): Promise<void> {
  return invoke<void>("open_in_editor", { path });
}

/** Ein Programm, das macOS für eine bestimmte Datei anbietet. */
export type OpenWithApp = {
  name: string;
  path: string;
  isDefault: boolean;
};

export async function listOpenWithApps(path: string): Promise<OpenWithApp[]> {
  return invoke<OpenWithApp[]>("list_open_with_apps", { path });
}

export async function openWithApp(
  paths: string[],
  appPath: string,
): Promise<void> {
  return invoke<void>("open_with_app", { paths, appPath });
}

/** Systemdialog zur Programmauswahl. `null`, wenn der Nutzer abbricht. */
export async function chooseApplication(): Promise<string | null> {
  return invoke<string | null>("choose_application_dialog");
}

/** Macht das Programm zum systemweiten Standard für Dateien dieses Typs. */
export async function setDefaultApplicationFor(
  appPath: string,
  filePath: string,
): Promise<void> {
  return invoke<void>("set_default_application_for", { appPath, filePath });
}

export async function setDockBadge(label: string | null): Promise<void> {
  return invoke<void>("set_dock_badge", { label });
}

export type Properties = {
  path: string;
  name: string;
  kind: string;
  isDir: boolean;
  isSymlink: boolean;
  symlinkTarget: string | null;
  size: number | null;
  fileCount: number | null;
  dirCount: number | null;
  mtime: number;
  btime: number;
  atime: number;
  owner: string;
  group: string;
  uid: number;
  gid: number;
  mode: number;
  modeStr: string;
};

export async function getProperties(path: string): Promise<Properties> {
  return invoke<Properties>("get_properties", { path });
}

export async function setPermissions(
  path: string,
  mode: number,
): Promise<void> {
  return invoke<void>("set_permissions", { path, mode });
}

export async function clipboardWriteFiles(paths: string[]): Promise<void> {
  return invoke<void>("clipboard_write_files", { paths });
}

export async function clipboardReadFiles(): Promise<string[]> {
  return invoke<string[]>("clipboard_read_files");
}

export async function dragIconPath(): Promise<string> {
  return invoke<string>("drag_icon_path");
}

export async function startPromiseDrag(paths: string[]): Promise<void> {
  return invoke<void>("start_promise_drag", { paths });
}

export async function resolvePromiseDrop(
  id: number,
  action: "overwrite" | "cancel" | "keep_both",
): Promise<void> {
  return invoke<void>("resolve_promise_drop", { id, action });
}

export type PaneChanged = { paneId: string; path: string };

/** Eine in RemoteDeskRDP eingerichtete RDP-Verbindung. */
export type RdpProfile = { id: string; name: string; host: string };

export async function rdpProfiles(): Promise<RdpProfile[]> {
  return invoke<RdpProfile[]>("rdp_profiles");
}

/** Reicht die Verbindung an RemoteDeskRDP weiter; die Sitzung baut jene App auf. */
export async function rdpConnect(id: string): Promise<void> {
  return invoke<void>("rdp_connect", { id });
}
