// Verzeichnis-Synchronisation: aktiver Pane → anderer Pane (z. B. HiDrive).
import { createSignal } from "solid-js";
import { state, setState, refreshPane } from "./state";
import { syncPreview, runJob, moveToTrash, type SyncEntry } from "./ipc";
import type { PaneId } from "./types";
import { t, errMsg } from "./i18n";
import { joinPath } from "./paths";
import { notifyError } from "./components/Dialogs";

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

export { syncDialog, syncEntries, syncDeleteExtra, syncLoading };

const newJobId = () => `job-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

async function reloadPreview() {
  const s = syncDialog();
  if (!s) return;
  setSyncLoading(true);
  try {
    // Immer mit delete_extra=true vorschauen, damit überzählige Ziel-Dateien
    // (in der Quelle gelöscht/nicht vorhanden) stets erkannt und dem Nutzer
    // angezeigt werden. Ob sie tatsächlich gelöscht werden, entscheidet erst
    // die Checkbox (syncDeleteExtra) in confirmSync.
    const entries = await syncPreview(s.src, s.dst, true);
    setSyncEntries(entries);
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
  setSyncEntries([]);
  setSyncDialog({ src, dst, srcName, target });
  await reloadPreview();
}

export function setSyncDelete(v: boolean) {
  // Nur den Schalter umlegen – die Extras sind bereits in der Vorschau enthalten,
  // ein erneuter (bei Netzlaufwerken langsamer) Preview-Roundtrip entfällt.
  setSyncDeleteExtra(v);
}

export function cancelSync() {
  setSyncDialog(null);
  setSyncEntries([]);
}

export async function confirmSync() {
  const s = syncDialog();
  if (!s) return;
  const entries = syncEntries();
  setSyncDialog(null);

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
