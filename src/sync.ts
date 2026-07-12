// Verzeichnis-Synchronisation: aktiver Pane → anderer Pane (z. B. HiDrive).
import { createSignal } from "solid-js";
import { state, setState, refreshPane } from "./state";
import {
  syncPreview,
  syncTwoWayPreview,
  runJob,
  moveToTrash,
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
const [syncIgnorePatterns, setSyncIgnorePatterns] = createSignal("");
const [syncMode, setSyncMode] = createSignal<"oneWay" | "twoWay">("oneWay");
const [syncVerifyChecksums, setSyncVerifyChecksums] = createSignal(false);
const [syncConflictChoices, setSyncConflictChoices] = createSignal<
  Record<string, "left" | "right" | "skip">
>({});
const [activeSyncProfileId, setActiveSyncProfileId] = createSignal<
  string | null
>(null);

export {
  syncDialog,
  syncEntries,
  syncDeleteExtra,
  syncLoading,
  syncIgnorePatterns,
  syncMode,
  syncVerifyChecksums,
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

async function reloadPreview() {
  const s = syncDialog();
  if (!s) return;
  setSyncLoading(true);
  try {
    // Immer mit delete_extra=true vorschauen, damit überzählige Ziel-Dateien
    // (in der Quelle gelöscht/nicht vorhanden) stets erkannt und dem Nutzer
    // angezeigt werden. Ob sie tatsächlich gelöscht werden, entscheidet erst
    // die Checkbox (syncDeleteExtra) in confirmSync.
    const preview =
      syncMode() === "twoWay"
        ? await syncTwoWayPreview(
            s.src,
            s.dst,
            ignorePatternList(),
            syncVerifyChecksums(),
          )
        : await syncPreview(
            s.src,
            s.dst,
            true,
            ignorePatternList(),
            syncVerifyChecksums(),
          );
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
  } catch (e) {
    await notifyError(t("common.error", { msg: errMsg(e) }));
    cancelSync();
  } finally {
    setSyncLoading(false);
  }
}

export async function openSyncDialog(
  src: string,
  dst: string,
  srcName: string,
  target: PaneId,
) {
  if (state.job) return;
  setSyncDeleteExtra(false);
  setSyncIgnorePatterns("");
  setSyncMode("oneWay");
  setSyncVerifyChecksums(false);
  setSyncConflictChoices({});
  setActiveSyncProfileId(null);
  setSyncEntries([]);
  setSyncDialog({ src, dst, srcName, target });
  await reloadPreview();
}

export function setSyncDelete(v: boolean) {
  // Nur den Schalter umlegen – die Extras sind bereits in der Vorschau enthalten,
  // ein erneuter (bei Netzlaufwerken langsamer) Preview-Roundtrip entfällt.
  setSyncDeleteExtra(v);
}

export function setSyncIgnoreText(value: string) {
  setSyncIgnorePatterns(value);
}

export async function setSyncModeAndRefresh(mode: "oneWay" | "twoWay") {
  setSyncMode(mode);
  await reloadPreview();
}

export async function setSyncVerifyChecksumsAndRefresh(value: boolean) {
  setSyncVerifyChecksums(value);
  await reloadPreview();
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

export async function applySyncProfile(id: string) {
  const profile = syncProfiles().find((item) => item.id === id);
  if (!profile || state.job) return;
  setSyncDeleteExtra(profile.deleteExtra);
  setSyncIgnorePatterns(profile.ignorePatterns);
  setSyncMode(profile.mode);
  setSyncVerifyChecksums(profile.verifyChecksums);
  setActiveSyncProfileId(profile.id);
  setSyncDialog({
    src: profile.src,
    dst: profile.dst,
    srcName: basename(profile.src),
    target: state.active === "left" ? "right" : "left",
  });
  await reloadPreview();
}

/** Führt ein gespeichertes Profil unabhängig von den aktuell geöffneten Panes
 * aus. Die Vorschau wird weiterhin vor dem Kopierjob erstellt, damit der
 * bestehende Ablauf für Änderungen, Löschungen und Konflikte erhalten bleibt.
 */
export async function runSyncProfile(id: string) {
  const profile = syncProfiles().find((item) => item.id === id);
  if (!profile || state.job) return;

  await applySyncProfile(profile.id);
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
  setSyncDialog(null);
  setSyncEntries([]);
}

export async function confirmSync() {
  const s = syncDialog();
  if (!s) return;
  const entries = syncEntries();
  const mode = syncMode();
  const conflictChoices = syncConflictChoices();
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
        current: "",
      });
      await runJob(id, "copy", items);
    }
    if (deletes.length > 0) {
      // Löschen läuft als einzelner Batch-Aufruf ohne Fortschrittsereignisse –
      // die Statusleiste soll trotzdem "Löschen" (nicht "Kopieren") anzeigen.
      setState("job", {
        id,
        kind: "delete",
        done: 0,
        total: deletes.length,
        current: "",
      });
      await moveToTrash(deletes.map((e) => joinPath(s.dst, e.rel)));
      setState("job", "done", deletes.length);
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
