// Verzeichnis-Synchronisation: aktiver Pane → anderer Pane (z. B. HiDrive).
import { createSignal } from "solid-js";
import { state, setState, refreshPane } from "./state";
import {
  syncPreview,
  syncTwoWayPreview,
  runJob,
  runRsync,
  cancelJob,
  loadRsyncPassword,
  saveRsyncPassword,
  moveToTrash,
  pathIsNetwork,
  remoteMounts,
  loadRemotePassword,
  mountRemote,
  mountObjectStorage,
  listNetworkBookmarks,
  mountNetworkUrl,
  runNetworkDelete,
  type SyncEntry,
} from "./ipc";
import type { PaneId } from "./types";
import { t, errMsg } from "./i18n";
import { folderCopyDestination, joinPath } from "./paths";
import { askConfirm, askPrompt, notifyError } from "./components/Dialogs";
import { reportKnownDeleteError } from "./deleteErrors";
import {
  newSyncProfileId,
  removeSyncProfile,
  saveSyncProfile,
  syncProfiles,
  type SyncProfile,
} from "./syncProfiles";
import {
  remoteDescriptor,
  remoteProfiles,
  remoteStorageDeleteTarget,
  remoteStorageTransferTarget,
} from "./remoteProfiles";
import {
  objectStorageDeleteTarget,
  objectStorageProfiles,
  objectStorageTransferTarget,
} from "./objectStorageProfiles";

export type SyncDialogState = {
  src: string;
  dst: string;
  srcName: string;
  target: PaneId;
};

const [syncDialog, setSyncDialog] = createSignal<SyncDialogState | null>(null);
const [syncEntries, setSyncEntries] = createSignal<SyncEntry[]>([]);
const [syncDeleteExtra, setSyncDeleteExtra] = createSignal(false);
const [syncLoading, setSyncLoading] = createSignal(false);
// Eine Dateisystem-Synchronisation darf nur mit einer Vorschau starten.
// Einstellungen selbst lösen bewusst keinen Netzlaufwerk-Scan aus.
const [syncPreviewReady, setSyncPreviewReady] = createSignal(false);
const [syncIgnorePatterns, setSyncIgnorePatterns] = createSignal("");
const [syncMode, setSyncMode] = createSignal<"oneWay" | "twoWay">("oneWay");
const [syncVerifyChecksums, setSyncVerifyChecksums] = createSignal(false);
// Obergrenze je Datei in MB; 0 bedeutet „keine Grenze". Sehr große Dateien
// blockieren auf langsamen Zielen sonst den gesamten Abgleich.
const [syncMaxFileSizeMb, setSyncMaxFileSizeMb] = createSignal(0);
const [syncTransport, setSyncTransport] = createSignal<
  "filesystem" | "rsync"
>("filesystem");
const [syncRsyncHost, setSyncRsyncHost] = createSignal("rsync.hidrive.ionos.com");
const [syncRsyncUsername, setSyncRsyncUsername] = createSignal("");
const [syncRsyncRemotePath, setSyncRsyncRemotePath] = createSignal("/");
const [syncRsyncPassword, setSyncRsyncPassword] = createSignal("");
const [syncRsyncSavePassword, setSyncRsyncSavePassword] = createSignal(true);
const [syncConflictChoices, setSyncConflictChoices] = createSignal<
  Record<string, "left" | "right" | "skip">
>({});
const [activeSyncProfileId, setActiveSyncProfileId] = createSignal<
  string | null
>(null);
// Eine abgebrochene oder durch eine neue Vorschau ersetzte IPC-Antwort darf
// den Dialog nicht wieder öffnen oder dessen Ergebnisse überschreiben.
let previewGeneration = 0;
let activePreviewId: string | null = null;

export {
  syncDialog,
  syncEntries,
  syncDeleteExtra,
  syncLoading,
  syncPreviewReady,
  syncIgnorePatterns,
  syncMode,
  syncVerifyChecksums,
  syncMaxFileSizeMb,
  syncTransport,
  syncRsyncHost,
  syncRsyncUsername,
  syncRsyncRemotePath,
  syncRsyncPassword,
  syncRsyncSavePassword,
  syncConflictChoices,
  activeSyncProfileId,
};

const newJobId = () => `job-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

/** Rechnet die Eingabe in MB auf Bytes um. Ungültige oder nicht positive
 * Werte ergeben 0, was im Backend „keine Grenze" bedeutet. */
function maxFileSizeBytes(): number {
  const mb = syncMaxFileSizeMb();
  if (!Number.isFinite(mb) || mb <= 0) return 0;
  return Math.floor(mb) * 1024 * 1024;
}

function ignorePatternList(): string[] {
  return syncIgnorePatterns()
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function basename(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || path;
}

const HIDRIVE_WEBDAV_MOUNT = "/Volumes/webdav.hidrive.ionos.com";

type RemotePathRef = NonNullable<NonNullable<SyncProfile["remotePaths"]>["src"]>;
type NetworkPathRef = NonNullable<NonNullable<SyncProfile["networkPaths"]>["src"]>;

function localRemotePath(path: string) {
  // Die Schreibweise des Ordners ist nicht garantiert: Auf case-insensitiven
  // Dateisystemen kann derselbe Pfad als „DualBeam“ oder „dualbeam“ auftreten.
  const match =
    /^(.*\/Library\/Application Support\/DualBeam\/Remote\/Volumes\/)([^/]+)(\/.*)?$/i.exec(path);
  if (!match) return null;
  return { label: match[2], relativePath: match[3] || "" };
}

function remotePathRef(
  path: string,
  mounts: Awaited<ReturnType<typeof remoteMounts>>,
): RemotePathRef | undefined {
  const mount = mounts.find(
    (item) => path === item.path || path.startsWith(`${item.path}/`),
  );
  if (!mount) return undefined;
  return { descriptor: mount.descriptor, relativePath: path.slice(mount.path.length) };
}

function resolvedRemotePath(
  ref: RemotePathRef | undefined,
  mounts: Awaited<ReturnType<typeof remoteMounts>>,
) {
  if (!ref) return undefined;
  const mount = mounts.find((item) => item.descriptor === ref.descriptor);
  if (!mount) return null;
  return `${mount.path}${ref.relativePath}`;
}

function networkPathRef(
  path: string,
  bookmarks: Awaited<ReturnType<typeof listNetworkBookmarks>>,
): NetworkPathRef | undefined {
  const bookmark = bookmarks.find(
    (item) => path === item.mountPath || path.startsWith(`${item.mountPath}/`),
  );
  if (!bookmark) return undefined;
  return { url: bookmark.url, relativePath: path.slice(bookmark.mountPath.length) };
}

function resolvedNetworkPath(
  ref: NetworkPathRef | undefined,
  bookmarks: Awaited<ReturnType<typeof listNetworkBookmarks>>,
) {
  if (!ref) return undefined;
  const bookmark = bookmarks.find((item) => item.url === ref.url && item.connected);
  if (!bookmark) return null;
  return `${bookmark.mountPath}${ref.relativePath}`;
}

async function mountRemoteForSync(ref: RemotePathRef): Promise<string | null> {
  const profile = remoteProfiles().find(
    (item) => remoteDescriptor(item) === ref.descriptor,
  );
  if (profile) {
    const password = await loadRemotePassword(profile);
    if (!password) return "Das Passwort für das Netzwerk-Laufwerk fehlt im Schlüsselbund.";
    await mountRemote(profile, password, profile.protocol === "ftp");
    return null;
  }
  const object = objectStorageProfiles().find(
    (item) => `${item.protocol}://${item.id}` === ref.descriptor,
  );
  if (object) {
    await mountObjectStorage(object);
    return null;
  }
  return "Das gespeicherte Netzwerk-Laufwerk wurde nicht gefunden.";
}

async function mountNetworkForSync(ref: NetworkPathRef): Promise<string | null> {
  const bookmark = (await listNetworkBookmarks()).find((item) => item.url === ref.url);
  if (!bookmark) return "Das gespeicherte Netzwerk-Laufwerk wurde nicht gefunden.";
  await mountNetworkUrl(bookmark.url);
  return null;
}

/** Alte Profile enthielten nur den flüchtigen Mount-Namen. Eine automatische
 * Umstellung ist ausschließlich dann sicher, wenn genau ein gespeichertes
 * Remote-Profil genau einem aktuell eingehängten Remote-Laufwerk entspricht. */
function migrateLegacyRemotePath(
  path: string,
  mounts: Awaited<ReturnType<typeof remoteMounts>>,
) {
  const legacy = localRemotePath(path);
  if (!legacy) return undefined;
  const exact = mounts.find((item) => item.label === legacy.label);
  if (exact) {
    const nextPath = `${exact.path}${legacy.relativePath}`;
    return { path: nextPath, ref: remotePathRef(nextPath, mounts)! };
  }
  const known = mounts.filter(
    (mount) =>
      remoteProfiles().some(
        (profile) => remoteDescriptor(profile) === mount.descriptor,
      ) ||
      // S3 und Swift werden über eigene Profile verwaltet. Ohne sie bliebe ein
      // Altprofil auf einem Objekt-Speicher dauerhaft unreparierbar.
      objectStorageProfiles().some(
        (profile) => `${profile.protocol}://${profile.id}` === mount.descriptor,
      ),
  );
  if (known.length !== 1) return undefined;
  const nextPath = `${known[0].path}${legacy.relativePath}`;
  return { path: nextPath, ref: remotePathRef(nextPath, mounts)! };
}

async function resolveProfileRemotePaths(
  profile: SyncProfile,
): Promise<SyncProfile | null> {
  let mounts = await remoteMounts();
  const refs = [profile.remotePaths?.src, profile.remotePaths?.dst].filter(
    (ref): ref is NonNullable<RemotePathRef> => !!ref,
  );
  for (const ref of refs) {
    if (resolvedRemotePath(ref, mounts) !== null) continue;
    try {
      const error = await mountRemoteForSync(ref);
      if (error) {
        await notifyError(`Netzwerk-Laufwerk ist offline: ${error}`);
        return null;
      }
      mounts = await remoteMounts();
      if (resolvedRemotePath(ref, mounts) === null) {
        await notifyError("Netzwerk-Laufwerk ist offline und konnte nicht verbunden werden.");
        return null;
      }
    } catch (error) {
      await notifyError(`Netzwerk-Laufwerk ist offline: ${errMsg(error)}`);
      return null;
    }
  }

  let bookmarks = await listNetworkBookmarks();
  // Auch ältere Profile, die nur den früheren /Volumes-Pfad kennen, können
  // über das gespeicherte Lesezeichen eindeutig migriert und verbunden werden.
  const savedSourceNetworkRef = profile.networkPaths?.src
    ?? networkPathRef(profile.src, bookmarks);
  const savedTargetNetworkRef = profile.networkPaths?.dst
    ?? networkPathRef(profile.dst, bookmarks);
  const networkRefs = [savedSourceNetworkRef, savedTargetNetworkRef].filter(
    (ref): ref is NonNullable<NetworkPathRef> => !!ref,
  );
  for (const ref of networkRefs) {
    if (resolvedNetworkPath(ref, bookmarks) !== null) continue;
    try {
      const error = await mountNetworkForSync(ref);
      if (error) {
        await notifyError(`Netzwerk-Laufwerk ist offline: ${error}`);
        return null;
      }
      bookmarks = await listNetworkBookmarks();
      if (resolvedNetworkPath(ref, bookmarks) === null) {
        await notifyError("Netzwerk-Laufwerk ist offline und konnte nicht verbunden werden.");
        return null;
      }
    } catch (error) {
      await notifyError(`Netzwerk-Laufwerk ist offline: ${errMsg(error)}`);
      return null;
    }
  }

  const sourceResolved = resolvedRemotePath(profile.remotePaths?.src, mounts)
    ?? resolvedNetworkPath(savedSourceNetworkRef, bookmarks);
  const targetResolved = resolvedRemotePath(profile.remotePaths?.dst, mounts)
    ?? resolvedNetworkPath(savedTargetNetworkRef, bookmarks);

  let src = sourceResolved ?? profile.src;
  let dst = targetResolved ?? profile.dst;
  let srcRef = profile.remotePaths?.src;
  let dstRef = profile.remotePaths?.dst;
  let srcNetworkRef = savedSourceNetworkRef;
  let dstNetworkRef = savedTargetNetworkRef;

  // Einmalige Migration für Profile, die vor der stabilen Zuordnung gespeichert
  // wurden. Scheint die Zuordnung nicht eindeutig, wird nichts geraten.
  if (!srcRef) {
    const migrated = migrateLegacyRemotePath(src, mounts);
    if (migrated) {
      src = migrated.path;
      srcRef = migrated.ref;
    }
  }
  if (!dstRef) {
    const migrated = migrateLegacyRemotePath(dst, mounts);
    if (migrated) {
      dst = migrated.path;
      dstRef = migrated.ref;
    }
  }

  const stale = !srcRef
    ? localRemotePath(src)
    : !dstRef
      ? localRemotePath(dst)
      : null;
  if (stale) {
    await notifyError(`Das Laufwerk „${stale.label}“ ist nicht verbunden. Bitte verbinden Sie es in der Seitenleiste erneut.`);
    return null;
  }
  const remotePaths = srcRef || dstRef ? { src: srcRef, dst: dstRef } : undefined;
  const networkPaths = srcNetworkRef || dstNetworkRef
    ? { src: srcNetworkRef, dst: dstNetworkRef }
    : undefined;
  const resolved = { ...profile, src, dst, remotePaths, networkPaths };
  if (
    resolved.src !== profile.src ||
    resolved.dst !== profile.dst ||
    JSON.stringify(resolved.remotePaths) !== JSON.stringify(profile.remotePaths) ||
    JSON.stringify(resolved.networkPaths) !== JSON.stringify(profile.networkPaths)
  ) {
    saveSyncProfile(resolved);
  }
  return resolved;
}

const isHiDriveWebDavPath = (path: string) =>
  path === HIDRIVE_WEBDAV_MOUNT || path.startsWith(`${HIDRIVE_WEBDAV_MOUNT}/`);

/** rsync (über SSH zu HiDrive) ist nur sinnvoll, wenn das Sync-Ziel auf dem
 * HiDrive-WebDAV-Mount liegt. Bei lokalen Zielen wird der Transport-Selektor
 * ausgeblendet und immer das Dateisystem verwendet. */
export function syncRsyncAvailable(): boolean {
  const s = syncDialog();
  return !!s && isHiDriveWebDavPath(s.dst);
}

function rsyncDefaultsFromWebDavPath(dst: string) {
  // Der sichtbare WebDAV-Pfad dient nur zur Orientierung. rsync benötigt
  // denselben HiDrive-Pfad ohne den lokalen /Volumes-Mountpoint.
  const mount = HIDRIVE_WEBDAV_MOUNT;
  const remotePath = dst === mount || dst.startsWith(`${mount}/`)
    ? dst.slice(mount.length) || "/"
    : "/";
  const username = remotePath.match(/^\/users\/([^/]+)/)?.[1] ?? "";
  return { host: "rsync.hidrive.ionos.com", remotePath, username };
}

function setRsyncDefaults(dst: string) {
  const defaults = rsyncDefaultsFromWebDavPath(dst);
  setSyncRsyncHost(defaults.host);
  setSyncRsyncRemotePath(defaults.remotePath);
  setSyncRsyncUsername(defaults.username);
}

async function reloadPreview() {
  const s = syncDialog();
  if (!s) return;
  const generation = ++previewGeneration;
  const previewId = `preview-${newJobId()}`;
  activePreviewId = previewId;
  setSyncPreviewReady(false);
  // Bei rsync ist ein WebDAV-Vergleich nicht verlässlich und für den Ablauf
  // auch nicht nötig: rsync ermittelt seine Differenzen direkt am Server.
  if (syncTransport() === "rsync") {
    setSyncEntries([]);
    setSyncConflictChoices({});
    setSyncLoading(false);
    setSyncPreviewReady(true);
    return;
  }
  setSyncLoading(true);
  try {
    // Immer mit delete_extra=true vorschauen, damit überzählige Ziel-Dateien
    // (in der Quelle gelöscht/nicht vorhanden) stets erkannt und dem Nutzer
    // angezeigt werden. Ob sie tatsächlich gelöscht werden, entscheidet erst
    // die Checkbox (syncDeleteExtra) in confirmSync.
    const preview =
      syncMode() === "twoWay"
        ? await syncTwoWayPreview(
            previewId,
            s.src,
            s.dst,
            ignorePatternList(),
            syncVerifyChecksums(),
            maxFileSizeBytes(),
          )
        : await syncPreview(
            previewId,
            s.src,
            s.dst,
            true,
            ignorePatternList(),
            syncVerifyChecksums(),
            maxFileSizeBytes(),
          );
    if (generation !== previewGeneration) return;
    // IPC-Daten defensiv prüfen: Ein unvollständiger Eintrag darf den Dialog
    // nicht über eine Property-Zugriffsverletzung zum Absturz bringen.
    const entries = preview.filter(
      (entry): entry is SyncEntry =>
        !!entry &&
        typeof entry.rel === "string" &&
        typeof entry.action === "string" &&
        typeof entry.isDir === "boolean" &&
        typeof entry.size === "number",
    );
    setSyncEntries(entries);
    setSyncConflictChoices(
      Object.fromEntries(
        entries
          .filter((entry) => entry.action === "conflict")
          .map((entry) => [entry.rel, "skip"]),
      ),
    );
    setSyncPreviewReady(true);
  } catch (e) {
    if (generation !== previewGeneration) return;
    await notifyError(t("common.error", { msg: errMsg(e) }));
    cancelSync();
  } finally {
    if (activePreviewId === previewId) activePreviewId = null;
    if (generation === previewGeneration) setSyncLoading(false);
  }
}

export async function openSyncDialog(
  src: string,
  dst: string,
  srcName: string,
  target: PaneId,
) {
  if (state.job || syncLoading()) return;
  setSyncDeleteExtra(false);
  setSyncIgnorePatterns("");
  setSyncMode("oneWay");
  setSyncVerifyChecksums(false);
  setSyncMaxFileSizeMb(0);
  setSyncTransport("filesystem");
  setRsyncDefaults(dst);
  setSyncRsyncPassword("");
  setSyncRsyncSavePassword(true);
  setSyncConflictChoices({});
  setActiveSyncProfileId(null);
  setSyncEntries([]);
  setSyncPreviewReady(false);
  setSyncDialog({ src, dst, srcName, target });
}

export function setSyncDelete(v: boolean) {
  // Nur den Schalter umlegen – die Extras sind bereits in der Vorschau enthalten,
  // ein erneuter (bei Netzlaufwerken langsamer) Preview-Roundtrip entfällt.
  setSyncDeleteExtra(v);
  // Ein über die Sidebar gestartetes Profil soll dieselbe Löschentscheidung
  // verwenden. Bisher war dafür zusätzlich „Profil speichern“ nötig, wodurch
  // der sichtbar gesetzte Schalter beim nächsten Sidebar-Start wieder verloren
  // ging. Die Änderung ist klein und wird sofort im aktiven Profil gesichert.
  const id = activeSyncProfileId();
  const profile = id
    ? syncProfiles().find((item) => item.id === id)
    : undefined;
  if (profile) saveSyncProfile({ ...profile, deleteExtra: v });
}

export function setSyncIgnoreText(value: string) {
  setSyncIgnorePatterns(value);
  setSyncPreviewReady(false);
}

export function setSyncModeAndRefresh(mode: "oneWay" | "twoWay") {
  setSyncMode(mode);
  setSyncPreviewReady(false);
}

export function setSyncVerifyChecksumsAndRefresh(value: boolean) {
  setSyncVerifyChecksums(value);
  setSyncPreviewReady(false);
}

/** Setzt die Größengrenze in MB. Ungültige Eingaben werden auf 0 („keine
 * Grenze") abgebildet, damit ein leeres Feld nicht alles ausschließt. Die
 * Vorschau muss danach neu erstellt werden, weil sich die Auswahl ändert. */
export function setSyncMaxFileSizeAndRefresh(value: number) {
  setSyncMaxFileSizeMb(
    Number.isFinite(value) && value > 0 ? Math.floor(value) : 0,
  );
  setSyncPreviewReady(false);
}

export function setSyncTransportAndRefresh(
  transport: "filesystem" | "rsync",
) {
  setSyncTransport(transport);
  // rsync arbeitet einweg (lokal → HiDrive); Zwei-Wege-Konflikte gehören
  // weiterhin zum Dateisystem-Transport über das eingebundene Laufwerk.
  if (transport === "rsync") setSyncMode("oneWay");
  setSyncEntries([]);
  setSyncConflictChoices({});
  setSyncPreviewReady(false);
}

export function setSyncRsyncHostValue(value: string) {
  setSyncRsyncHost(value);
}

export function setSyncRsyncUsernameValue(value: string) {
  setSyncRsyncUsername(value);
}

export function setSyncRsyncRemotePathValue(value: string) {
  setSyncRsyncRemotePath(value);
}

export function setSyncRsyncPasswordValue(value: string) {
  setSyncRsyncPassword(value);
}

export function setSyncRsyncSavePasswordValue(value: boolean) {
  setSyncRsyncSavePassword(value);
}

/** Lädt ein gespeichertes Kennwort. Fehlende Einträge bleiben still leer,
 * damit ein gespeichertes Profil aus der Sidebar nicht blockiert wird. */
export async function loadSyncRsyncPasswordFromKeychain(): Promise<boolean> {
  const host = syncRsyncHost().trim();
  const username = syncRsyncUsername().trim();
  if (!host || !username) return false;
  const password = await loadRsyncPassword(host, username);
  if (!password) return false;
  setSyncRsyncPassword(password);
  return true;
}

export function setSyncConflictChoice(
  rel: string,
  choice: "left" | "right" | "skip",
) {
  setSyncConflictChoices((choices) => ({ ...choices, [rel]: choice }));
}

export async function refreshSyncPreview() {
  await reloadPreview();
}

export async function applySyncProfile(id: string, preview = false) {
  const savedProfile = syncProfiles().find((item) => item.id === id);
  if (!savedProfile || state.job) return false;
  // Ein Profil bleibt auch dann auswählbar, wenn sein Netzlaufwerk nicht
  // mehr existiert. Nur Vorschau und Ausführung brauchen eine Verbindung;
  // insbesondere muss der Löschen-Button für einen verwaisten Eintrag sofort
  // verfügbar sein.
  setActiveSyncProfileId(savedProfile.id);
  const profile = await resolveProfileRemotePaths(savedProfile);
  if (!profile) return false;
  setSyncDeleteExtra(profile.deleteExtra);
  setSyncIgnorePatterns(profile.ignorePatterns);
  setSyncMode(profile.mode);
  setSyncVerifyChecksums(profile.verifyChecksums);
  setSyncMaxFileSizeMb(profile.maxFileSizeMb);
  // Sicherheitsnetz: rsync gilt nur für HiDrive-Ziele. Ein (altes) Profil mit
  // lokalem Ziel fällt auf den Dateisystem-Transport zurück.
  const transport =
    profile.transport === "rsync" && !isHiDriveWebDavPath(profile.dst)
      ? "filesystem"
      : profile.transport;
  const destination =
    transport === "filesystem"
      ? folderCopyDestination(basename(profile.src), profile.dst)
      : profile.dst;
  setSyncTransport(transport);
  if (transport === "rsync") {
    const defaults = rsyncDefaultsFromWebDavPath(profile.dst);
    setSyncRsyncHost(profile.rsync?.host || defaults.host);
    setSyncRsyncUsername(profile.rsync?.username || defaults.username);
    setSyncRsyncRemotePath(profile.rsync?.remotePath || defaults.remotePath);
    setSyncRsyncPassword("");
    setSyncRsyncSavePassword(true);
    // Der Schlüsselbund ist die einzige persistente Passwortquelle. Das
    // ermöglicht den Start eines rsync-Profils direkt aus der Sidebar.
    try {
      await loadSyncRsyncPasswordFromKeychain();
    } catch {
      // Wenn der Schlüsselbund nicht verfügbar ist, zeigt confirmSync eine
      // klare Meldung statt das Profil unbrauchbar zu machen.
    }
  } else {
    setRsyncDefaults(destination);
    setSyncRsyncPassword("");
  }
  setSyncDialog({
    src: profile.src,
    dst: destination,
    srcName: basename(profile.src),
    target: state.active === "left" ? "right" : "left",
  });
  setSyncEntries([]);
  setSyncConflictChoices({});
  setSyncPreviewReady(false);
  if (preview && transport === "filesystem") await reloadPreview();
  return true;
}

/** Führt ein gespeichertes Profil unabhängig von den aktuell geöffneten Panes
 * aus. Die Vorschau wird weiterhin vor dem Kopierjob erstellt, damit der
 * bestehende Ablauf für Änderungen, Löschungen und Konflikte erhalten bleibt.
 */
export async function runSyncProfile(id: string) {
  const profile = syncProfiles().find((item) => item.id === id);
  if (!profile || state.job) return;

  const applied = await applySyncProfile(profile.id, true);
  if (!applied) return;
  // `reloadPreview` kann bei einem Fehler den Dialog schließen. In diesem
  // Fall darf kein Job mit einer unvollständigen Vorschau gestartet werden.
  if (!syncDialog() || syncLoading()) return;
  await confirmSync();
}

export async function saveCurrentSyncProfile() {
  const dialog = syncDialog();
  if (!dialog) return;
  const activeId = activeSyncProfileId();
  const existing = activeId
    ? syncProfiles().find((profile) => profile.id === activeId)
    : undefined;
  const name =
    existing?.name ??
    (await askPrompt({
      title: t("sync.profileSaveTitle"),
      label: t("sync.profileSaveLabel"),
      defaultValue: dialog.srcName,
      okLabel: t("sync.profileSave"),
    }));
  const trimmed = name?.trim();
  if (!trimmed) return;
  let remotePaths: SyncProfile["remotePaths"];
  let networkPaths: SyncProfile["networkPaths"];
  try {
    const mounts = await remoteMounts();
    const src = remotePathRef(dialog.src, mounts);
    const dst = remotePathRef(dialog.dst, mounts);
    if (src || dst) remotePaths = { src, dst };
  } catch {
    // Das Speichern eines lokalen Profils darf nicht an einer kurzzeitig
    // nicht erreichbaren Remote-Mount-Abfrage scheitern.
  }
  try {
    const bookmarks = await listNetworkBookmarks();
    const src = networkPathRef(dialog.src, bookmarks);
    const dst = networkPathRef(dialog.dst, bookmarks);
    if (src || dst) networkPaths = { src, dst };
  } catch {
    // Ein lokales Profil bleibt auch dann speicherbar, wenn die macOS-
    // Netzwerk-Laufwerke gerade nicht abgefragt werden können.
  }
  const profile: SyncProfile = {
    id: existing?.id ?? newSyncProfileId(),
    name: trimmed,
    src: dialog.src,
    dst: dialog.dst,
    deleteExtra: syncDeleteExtra(),
    ignorePatterns: syncIgnorePatterns(),
    mode: syncMode(),
    verifyChecksums: syncVerifyChecksums(),
    maxFileSizeMb: syncMaxFileSizeMb(),
    transport: syncTransport(),
    rsync:
      syncTransport() === "rsync"
        ? {
            host: syncRsyncHost().trim(),
            username: syncRsyncUsername().trim(),
            remotePath: syncRsyncRemotePath().trim(),
        }
        : undefined,
    remotePaths,
    networkPaths,
  };
  saveSyncProfile(profile);
  setActiveSyncProfileId(profile.id);
}

export async function deleteCurrentSyncProfile() {
  const id = activeSyncProfileId();
  const profile = id
    ? syncProfiles().find((item) => item.id === id)
    : undefined;
  if (!profile) return;
  const confirmed = await askConfirm({
    title: t("sync.profileDeleteTitle"),
    message: t("sync.profileDeleteConfirm", { name: profile.name }),
    okLabel: t("common.delete"),
    danger: true,
  });
  if (!confirmed) return;
  removeSyncProfile(profile.id);
  setActiveSyncProfileId(null);
}

export function cancelSync() {
  previewGeneration += 1;
  if (activePreviewId) {
    void cancelJob(activePreviewId);
    activePreviewId = null;
  }
  setSyncDialog(null);
  setSyncEntries([]);
  setSyncPreviewReady(false);
  setSyncLoading(false);
}

export async function confirmSync() {
  const s = syncDialog();
  if (!s) return;
  const entries = syncEntries();
  const mode = syncMode();
  const conflictChoices = syncConflictChoices();
  if (syncTransport() === "filesystem" && !syncPreviewReady()) return;

  if (syncTransport() === "rsync") {
    const host = syncRsyncHost().trim();
    const username = syncRsyncUsername().trim();
    const remotePath = syncRsyncRemotePath().trim();
    const password = syncRsyncPassword();
    if (!host || !username || !remotePath || !password) {
      // Dialog offen lassen: Die Meldung soll fehlende Pflichtfelder anmahnen,
      // ohne die bereits gemachten Eingaben zu verwerfen.
      await notifyError(t("sync.rsyncRequired"));
      return;
    }
    setSyncDialog(null);
    const id = newJobId();
    try {
      if (syncRsyncSavePassword()) {
        await saveRsyncPassword(host, username, password);
      }
      // rsync meldet nur tatsächlich übertragene Dateien; die komplette
      // Baumgröße wäre lediglich der Vergleich, nicht die Kopiermenge.
      setState("job", {
        id,
        kind: "rsync",
        done: 0,
        total: 0,
        filesDone: 0,
        current: `rsync: ${username}@${host}`,
      });
      await runRsync({
        jobId: id,
        localPath: s.src,
        host,
        remotePath,
        username,
        password,
        deleteExtra: syncDeleteExtra(),
        excludePatterns: ignorePatternList(),
        maxFileSize: maxFileSizeBytes(),
      });
    } catch (e) {
      // Ein bewusster Klick auf „Abbrechen“ ist kein Fehlerdialog.
      if (errMsg(e) !== t("err.rsyncCancelled")) {
        await notifyError(t("common.error", { msg: errMsg(e) }));
      }
    } finally {
      setState("job", null);
      await refreshPane("left");
      await refreshPane("right");
    }
    return;
  }

  setSyncDialog(null);

  if (mode === "twoWay") {
    const leftToRight = entries.filter(
      (entry) =>
        entry.action === "left_to_right" ||
        (entry.action === "conflict" && conflictChoices[entry.rel] === "left"),
    );
    const rightToLeft = entries.filter(
      (entry) =>
        entry.action === "right_to_left" ||
        (entry.action === "conflict" && conflictChoices[entry.rel] === "right"),
    );
    if (leftToRight.length === 0 && rightToLeft.length === 0) return;
    const id = newJobId();
    try {
      if (leftToRight.length > 0) {
        setState("job", {
          id,
          kind: "copy",
          done: 0,
          total: leftToRight.length,
          filesDone: 0,
          current: "",
        });
        const items = leftToRight.map((entry) => ({
          src: joinPath(s.src, entry.rel),
          dst: joinPath(s.dst, entry.rel),
          overwrite: true,
        }));
        await runJob(
          id,
          "copy",
          items,
          await objectStorageTransferTarget(items),
          await remoteStorageTransferTarget(items),
        );
      }
      if (rightToLeft.length > 0) {
        setState("job", {
          id,
          kind: "copy",
          done: 0,
          total: rightToLeft.length,
          filesDone: 0,
          current: "",
        });
        const items = rightToLeft.map((entry) => ({
          src: joinPath(s.dst, entry.rel),
          dst: joinPath(s.src, entry.rel),
          overwrite: true,
        }));
        await runJob(
          id,
          "copy",
          items,
          await objectStorageTransferTarget(items),
          await remoteStorageTransferTarget(items),
        );
      }
    } catch (e) {
      await notifyError(t("common.error", { msg: errMsg(e) }));
    } finally {
      setState("job", null);
      await refreshPane("left");
      await refreshPane("right");
    }
    return;
  }

  const copies = entries.filter(
    (e) => e.action === "copy" || e.action === "update",
  );
  // Löschungen nur ausführen, wenn der Nutzer sie ausdrücklich bestätigt hat.
  const deletes = syncDeleteExtra()
    ? entries.filter((e) => e.action === "delete")
    : [];
  if (copies.length === 0 && deletes.length === 0) return;

  const id = newJobId();
  try {
    if (copies.length > 0) {
      const items = copies.map((e) => ({
        src: joinPath(s.src, e.rel),
        dst: joinPath(s.dst, e.rel),
        // Sync muss denselben robusten Überschreibpfad wie der normale
        // Kopierprozess verwenden. Die SFTP-Verzeichnisansicht kann kurzzeitig
        // veraltet sein und eine vorhandene Datei als "copy" statt "update"
        // melden. `overwrite: true` lädt deshalb zunächst unter einem
        // temporären Namen hoch und ersetzt das Ziel anschließend sicher.
        overwrite: true,
      }));
      setState("job", {
        id,
        kind: "copy",
        done: 0,
        total: items.length,
        filesDone: 0,
        current: "",
      });
      await runJob(
        id,
        "copy",
        items,
        await objectStorageTransferTarget(items),
        await remoteStorageTransferTarget(items),
      );
    }
    if (deletes.length > 0) {
      const deletePaths = deletes.map((e) => joinPath(s.dst, e.rel));
      let targetIsNetwork = isHiDriveWebDavPath(s.dst);
      try {
        targetIsNetwork = (await pathIsNetwork(s.dst)) || targetIsNetwork;
      } catch {}
      setState("job", {
        id,
        kind: "delete",
        done: 0,
        total: targetIsNetwork ? 0 : deletes.length,
        filesDone: 0,
        current: "",
      });
      if (targetIsNetwork) {
        let objectStorage;
        let remoteStorage;
        try {
          objectStorage = await objectStorageDeleteTarget(
            deletePaths,
            deletes
              .filter((entry) => entry.isDir)
              .map((entry) => joinPath(s.dst, entry.rel)),
          );
          if (!objectStorage) remoteStorage = await remoteStorageDeleteTarget(deletePaths);
        } catch {
          // Für nicht verwaltete Netzlaufwerke gibt es keinen Objekt-Speicher-
          // Schnellpfad; sie werden weiterhin über das Dateisystem gelöscht.
        }
        await runNetworkDelete(id, deletePaths, objectStorage, remoteStorage);
      } else {
        await moveToTrash(deletePaths);
        setState("job", "done", deletes.length);
      }
    }
  } catch (e) {
    const raw = errMsg(e);
    // Time-Machine-Ziele sind auch beim Sync-Löschen geschützt.
    if (!(await reportKnownDeleteError(raw))) {
      await notifyError(t("common.error", { msg: raw }));
    }
  } finally {
    setState("job", null);
    await refreshPane("left");
    await refreshPane("right");
  }
}

/** Startet die Synchronisation des ausgewählten Ordners im aktiven Pane in den anderen Pane. */
export async function syncToOther() {
  if (state.job) return;
  const srcPane = state.active;
  const dstPane: PaneId = srcPane === "left" ? "right" : "left";
  const p = state[srcPane];
  const cur = p.entries.filter((e) => p.selected.has(e.path));
  const folder = cur.length > 0 ? cur[0] : p.entries[p.cursor];
  if (!folder || !folder.isDir) {
    await notifyError(t("sync.selectFolder"));
    return;
  }
  const dstCwd = state[dstPane].cwd;
  const dst = folderCopyDestination(folder.name, dstCwd);
  await openSyncDialog(folder.path, dst, folder.name, dstPane);
}
