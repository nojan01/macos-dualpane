import { invoke } from "@tauri-apps/api/core";
import type { Entry } from "./types";

export async function listDir(path: string, showHidden: boolean): Promise<Entry[]> {
  return invoke<Entry[]>("list_dir", { path, showHidden });
}

export async function openDefault(path: string): Promise<void> {
  return invoke<void>("open_default", { path });
}

export async function openUrl(url: string): Promise<void> {
  return invoke<void>("open_url", { url });
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

export async function renamePath(oldPath: string, newPath: string): Promise<void> {
  return invoke<void>("rename_path", { oldPath, newPath });
}

export async function createSymlink(target: string, linkPath: string): Promise<void> {
  return invoke<void>("create_symlink", { target, linkPath });
}

export async function createFinderAlias(target: string, linkPath: string): Promise<void> {
  return invoke<void>("create_finder_alias", { target, linkPath });
}

export async function moveToTrash(paths: string[]): Promise<void> {
  return invoke<void>("move_to_trash", { paths });
}

export async function forceDeleteAdmin(paths: string[]): Promise<void> {
  return invoke<void>("force_delete_admin", { paths });
}

export async function pathExists(path: string): Promise<boolean> {
  return invoke<boolean>("path_exists", { path });
}

export type Volume = { name: string; path: string; kind: "local" | "network" };

export async function listVolumes(): Promise<Volume[]> {
  return invoke<Volume[]>("list_volumes");
}

export async function ejectVolume(path: string): Promise<void> {
  return invoke<void>("eject_volume", { path });
}

export type NetworkBookmark = { name: string; url: string; mountPath: string; connected: boolean };

export async function listNetworkBookmarks(): Promise<NetworkBookmark[]> {
  return invoke<NetworkBookmark[]>("list_network_bookmarks");
}

export async function mountNetworkUrl(url: string): Promise<string> {
  return invoke<string>("mount_network_url", { url });
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

export async function runJob(jobId: string, kind: JobKind, items: JobItem[]): Promise<void> {
  return invoke<void>("run_job", { jobId, kind, items });
}

export async function cancelJob(jobId: string): Promise<void> {
  return invoke<void>("cancel_job", { jobId });
}

export type SyncAction = "copy" | "update" | "delete";
export type SyncEntry = { rel: string; action: SyncAction; isDir: boolean; size: number };

export async function syncPreview(src: string, dst: string, deleteExtra: boolean): Promise<SyncEntry[]> {
  return invoke<SyncEntry[]>("sync_preview", { src, dst, deleteExtra });
}

export type JobProgress = {
  jobId: string;
  done: number;
  total: number;
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
  return invoke<Entry[]>("search_in_dir", { root, query, showHidden, maxResults });
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

export async function readTextPreview(path: string, maxBytes = TEXT_PREVIEW_MAX_BYTES): Promise<string> {
  return invoke<string>("read_text_preview", { path, maxBytes });
}

export async function readImageThumb(path: string, size = IMAGE_THUMB_SIZE): Promise<string> {
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
  size: number;
  fileCount: number;
  dirCount: number;
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

export async function setPermissions(path: string, mode: number): Promise<void> {
  return invoke<void>("set_permissions", { path, mode });
}

export type TmDestination = {
  name: string;
  id: string;
  mountPoint: string;
  kind: string;
};

export async function tmListDestinations(): Promise<TmDestination[]> {
  return invoke<TmDestination[]>("tm_list_destinations");
}

export async function tmListBackups(mountPoint?: string | null): Promise<string[]> {
  return invoke<string[]>("tm_list_backups", { mountPoint: mountPoint ?? null });
}

export async function tmDeleteBackup(backupPath: string, password: string): Promise<string> {
  return invoke<string>("tm_delete_backup", { backupPath, password });
}

export async function tmWipeVolume(mountPoint: string): Promise<string> {
  return invoke<string>("tm_wipe_volume", { mountPoint });
}

export type TmVolume = {
  name: string;
  path: string;
  kind: "active" | "former";
  hasBackupdb: boolean;
  roleBackup: boolean;
  registered: boolean;
};

export async function tmListWipeableVolumes(): Promise<TmVolume[]> {
  return invoke<TmVolume[]>("tm_list_wipeable_volumes");
}

export async function tmListLocalSnapshots(): Promise<string[]> {
  return invoke<string[]>("tm_list_local_snapshots");
}

export async function tmDeleteLocalSnapshot(date: string): Promise<void> {
  return invoke<void>("tm_delete_local_snapshot", { date });
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

export async function resolvePromiseDrop(id: number, action: "overwrite" | "cancel" | "keep_both"): Promise<void> {
  return invoke<void>("resolve_promise_drop", { id, action });
}

export type PaneChanged = { paneId: string; path: string };
