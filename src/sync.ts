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
  runNetworkDelete,
  type SyncEntry,
} from "./ipc";
import type { PaneId } from "./types";
import { t, errMsg } from "./i18n";
import { joinPath } from "./paths";
import { askConfirm, askPrompt, notifyError } from "./components/Dialogs";
import {
  newSyncProfileId,
  removeSyncProfile,
  saveSyncProfile,
  syncProfiles,
  type SyncProfile,
} from "./syncProfiles";

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
          )
        : await syncPreview(
            previewId,
            s.src,
            s.dst,
            true,
            ignorePatternList(),
            syncVerifyChecksums(),
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
  const profile = syncProfiles().find((item) => item.id === id);
  if (!profile || state.job) return;
  setSyncDeleteExtra(profile.deleteExtra);
  setSyncIgnorePatterns(profile.ignorePatterns);
  setSyncMode(profile.mode);
  setSyncVerifyChecksums(profile.verifyChecksums);
  // Sicherheitsnetz: rsync gilt nur für HiDrive-Ziele. Ein (altes) Profil mit
  // lokalem Ziel fällt auf den Dateisystem-Transport zurück.
  const transport =
    profile.transport === "rsync" && !isHiDriveWebDavPath(profile.dst)
      ? "filesystem"
      : profile.transport;
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
    setRsyncDefaults(profile.dst);
    setSyncRsyncPassword("");
  }
  setActiveSyncProfileId(profile.id);
  setSyncDialog({
    src: profile.src,
    dst: profile.dst,
    srcName: basename(profile.src),
    target: state.active === "left" ? "right" : "left",
  });
  setSyncEntries([]);
  setSyncConflictChoices({});
  setSyncPreviewReady(false);
  if (preview && transport === "filesystem") await reloadPreview();
}

/** Führt ein gespeichertes Profil unabhängig von den aktuell geöffneten Panes
 * aus. Die Vorschau wird weiterhin vor dem Kopierjob erstellt, damit der
 * bestehende Ablauf für Änderungen, Löschungen und Konflikte erhalten bleibt.
 */
export async function runSyncProfile(id: string) {
  const profile = syncProfiles().find((item) => item.id === id);
  if (!profile || state.job) return;

  await applySyncProfile(profile.id, true);
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
  const profile: SyncProfile = {
    id: existing?.id ?? newSyncProfileId(),
    name: trimmed,
    src: dialog.src,
    dst: dialog.dst,
    deleteExtra: syncDeleteExtra(),
    ignorePatterns: syncIgnorePatterns(),
    mode: syncMode(),
    verifyChecksums: syncVerifyChecksums(),
    transport: syncTransport(),
    rsync:
      syncTransport() === "rsync"
        ? {
            host: syncRsyncHost().trim(),
            username: syncRsyncUsername().trim(),
            remotePath: syncRsyncRemotePath().trim(),
          }
        : undefined,
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
        await runJob(
          id,
          "copy",
          leftToRight.map((entry) => ({
            src: joinPath(s.src, entry.rel),
            dst: joinPath(s.dst, entry.rel),
            overwrite: true,
          })),
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
        await runJob(
          id,
          "copy",
          rightToLeft.map((entry) => ({
            src: joinPath(s.dst, entry.rel),
            dst: joinPath(s.src, entry.rel),
            overwrite: true,
          })),
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
        overwrite: e.action === "update",
      }));
      setState("job", {
        id,
        kind: "copy",
        done: 0,
        total: items.length,
        filesDone: 0,
        current: "",
      });
      await runJob(id, "copy", items);
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
        await runNetworkDelete(id, deletePaths);
      } else {
        await moveToTrash(deletePaths);
        setState("job", "done", deletes.length);
      }
    }
  } catch (e) {
    await notifyError(t("common.error", { msg: errMsg(e) }));
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
  const dst = joinPath(dstCwd, folder.name);
  await openSyncDialog(folder.path, dst, folder.name, dstPane);
}
