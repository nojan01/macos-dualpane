// Orchestriert Datei-Operationen: Konfliktprüfung, Job-Lauf, Refresh.
import { createSignal } from "solid-js";
import { state, setState, refreshPane, loadPane } from "./state";
import { askPrompt, askConfirm, notifyError } from "./components/Dialogs";
import type { Entry, PaneId } from "./types";
import { t, errMsg } from "./i18n";
import { joinPath, splitName, uniqueName } from "./paths";
import {
  checkConflicts,
  runJob,
  createDir,
  createFile,
  renamePath,
  moveToTrash,
  forceDeleteAdmin,
  pathExists,
  zipCreate,
  zipExtract,
  createSymlink,
  createFinderAlias,
  clipboardReadFiles,
  type JobItem,
  type JobKind,
} from "./ipc";

const newJobId = () => `job-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

export function selectedEntries(pane: PaneId) {
  const p = state[pane];
  const sel = p.entries.filter((e) => p.selected.has(e.path));
  if (sel.length > 0) return sel;
  const cur = p.entries[p.cursor];
  return cur ? [cur] : [];
}

export type ConflictChoice = "overwrite" | "skip" | "rename" | "cancel";
export type ConflictPrompt = { count: number; sample: string[] } | null;

export const [conflictPrompt, setConflictPrompt] = createSignal<ConflictPrompt>(null);
let pendingResolve: ((c: ConflictChoice) => void) | null = null;

export function askConflict(count: number, sample: string[]): Promise<ConflictChoice> {
  setConflictPrompt({ count, sample });
  return new Promise<ConflictChoice>((resolve) => {
    pendingResolve = resolve;
  });
}

export function resolveConflict(choice: ConflictChoice) {
  setConflictPrompt(null);
  if (pendingResolve) {
    const r = pendingResolve;
    pendingResolve = null;
    r(choice);
  }
}

// ----- Public ops -----

export async function transferEntries(
  kind: JobKind,
  srcEntries: Entry[],
  dstCwd: string,
  refreshPanes: PaneId[] = ["left", "right"],
  conflictMode: "ask" | "skip" | "overwrite" = "ask",
) {
  if (state.job) return;
  if (srcEntries.length === 0) return;

  // Selbst-Drop in den exakten Quell-Ordner ignorieren
  const sameDir = srcEntries.every((e) => {
    const parent = e.path.slice(0, -e.name.length).replace(/\/$/, "");
    return parent === dstCwd.replace(/\/$/, "");
  });
  if (sameDir) return;

  let items: JobItem[] = srcEntries.map((e) => ({
    src: e.path,
    dst: joinPath(dstCwd, e.name),
    overwrite: false,
  }));

  const conflicts = await checkConflicts(items);
  if (conflicts.length > 0) {
    let choice: ConflictChoice;
    if (conflictMode === "skip") choice = "skip";
    else if (conflictMode === "overwrite") choice = "overwrite";
    else choice = await askConflict(
      conflicts.length,
      conflicts.slice(0, 5).map((p) => p.split("/").pop() || p),
    );
    if (choice === "cancel") return;
    if (choice === "skip") {
      items = items.filter((i) => !conflicts.includes(i.dst));
      if (items.length === 0) return;
    } else if (choice === "overwrite") {
      items = items.map((i) =>
        conflicts.includes(i.dst) ? { ...i, overwrite: true } : i,
      );
    } else if (choice === "rename") {
      const resolved: JobItem[] = [];
      for (const i of items) {
        if (conflicts.includes(i.dst)) {
          const name = i.dst.split("/").pop()!;
          const fresh = await uniqueName(dstCwd, name);
          resolved.push({ src: i.src, dst: joinPath(dstCwd, fresh), overwrite: false });
        } else {
          resolved.push(i);
        }
      }
      items = resolved;
    }
  }

  const id = newJobId();
  setState("job", { id, kind, done: 0, total: 0, current: "" });
  try {
    await runJob(id, kind, items);
  } catch (e) {
    await notifyError(t("common.error", { msg: errMsg(e) }));
  } finally {
    setState("job", null);
    for (const p of refreshPanes) await refreshPane(p);
  }
}

export async function startTransfer(kind: JobKind) {
  if (state.job) return;
  const srcPane = state.active;
  const dstPane: PaneId = srcPane === "left" ? "right" : "left";
  const srcCwd = state[srcPane].cwd;
  const dstCwd = state[dstPane].cwd;
  if (srcCwd === dstCwd) {
    await notifyError(t("jobs.sameSrcDst"));
    return;
  }
  const sel = selectedEntries(srcPane);
  if (sel.length === 0) return;
  await transferEntries(kind, sel, dstCwd, [srcPane, dstPane]);
}

export async function createLinksInOther(kind: "symlink" | "alias") {
  if (state.job) return;
  const srcPane = state.active;
  const dstPane: PaneId = srcPane === "left" ? "right" : "left";
  const srcCwd = state[srcPane].cwd;
  const dstCwd = state[dstPane].cwd;
  if (srcCwd === dstCwd) {
    await notifyError(t("jobs.sameSrcDst"));
    return;
  }
  const sel = selectedEntries(srcPane);
  if (sel.length === 0) return;
  const errors: string[] = [];
  for (const e of sel) {
    let name = e.name;
    if (await pathExists(joinPath(dstCwd, name))) {
      name = await uniqueName(dstCwd, name);
    }
    const dst = joinPath(dstCwd, name);
    try {
      if (kind === "symlink") await createSymlink(e.path, dst);
      else await createFinderAlias(e.path, dst);
    } catch (err) {
      errors.push(`${e.name}: ${errMsg(err)}`);
    }
  }
  if (errors.length) await notifyError(errors.join("\n"));
  await refreshPane(dstPane);
}

export async function duplicateSelected() {
  if (state.job) return;
  const pane = state.active;
  const sel = selectedEntries(pane);
  if (sel.length === 0) return;
  const dir = state[pane].cwd;

  const items: JobItem[] = [];
  for (const e of sel) {
    const fresh = await uniqueName(dir, e.name);
    items.push({ src: e.path, dst: joinPath(dir, fresh), overwrite: false });
  }

  const id = newJobId();
  setState("job", { id, kind: "copy", done: 0, total: 0, current: "" });
  try {
    await runJob(id, "copy", items);
  } catch (err) {
    await notifyError(t("common.error", { msg: errMsg(err) }));
  } finally {
    setState("job", null);
    await refreshPane(pane);
  }
}

export async function deleteSelected(skipConfirm = false) {
  if (state.job) return;
  const pane = state.active;
  const sel = selectedEntries(pane);
  if (sel.length === 0) return;

  if (!skipConfirm) {
    const ok = await askConfirm({
      title: t("jobs.trash.title"),
      message:
        sel.length === 1
          ? t("jobs.trash.one", { name: sel[0].name })
          : t("jobs.trash.many", { count: sel.length }),
      okLabel: t("common.delete"),
      danger: true,
    });
    if (!ok) return;
  }
  try {
    await moveToTrash(sel.map((e) => e.path));
  } catch (e) {
    const raw = errMsg(e);
    const isProtected = raw.includes("NEEDS_ADMIN");
    const retry = await askConfirm({
      title: t("jobs.trash.title"),
      message: isProtected
        ? t("jobs.trash.protectedAdmin", {
            count: String(sel.length),
            name: sel[0].name,
          })
        : t("jobs.trash.forceAdmin", { msg: raw }),
      okLabel: t("jobs.trash.deleteAsAdmin"),
      danger: true,
    });
    if (retry) {
      try {
        await forceDeleteAdmin(sel.map((e) => e.path));
      } catch (e2) {
        await notifyError(t("common.error", { msg: errMsg(e2) }));
      }
    }
  }
  await refreshPane(pane);
}

export async function makeFolder() {
  if (state.job) return;
  const pane = state.active;
  const name = await askPrompt({
    title: t("jobs.newFolder.title"),
    label: t("jobs.newFolder.prompt"),
    defaultValue: t("jobs.newFolder.placeholder"),
    okLabel: t("common.create"),
  });
  if (!name) return;
  const full = joinPath(state[pane].cwd, name);
  try {
    await createDir(full);
  } catch (e) {
    await notifyError(t("common.error", { msg: errMsg(e) }));
    return;
  }
  await refreshPane(pane);
}

export async function makeFile() {
  if (state.job) return;
  const pane = state.active;
  const name = await askPrompt({
    title: t("jobs.newFile.title"),
    label: t("jobs.newFile.prompt"),
    defaultValue: t("jobs.newFile.placeholder"),
    okLabel: t("common.create"),
  });
  if (!name) return;
  const full = joinPath(state[pane].cwd, name);
  try {
    await createFile(full);
  } catch (e) {
    await notifyError(t("common.error", { msg: errMsg(e) }));
    return;
  }
  await refreshPane(pane);
}

export function beginRename() {
  if (state.job) return;
  const pane = state.active;
  const p = state[pane];
  if (p.entries[p.cursor]) {
    setState("editing", { pane, idx: p.cursor });
  }
}

export async function commitRename(newName: string) {
  const ed = state.editing;
  if (!ed) return;
  const p = state[ed.pane];
  const e = p.entries[ed.idx];
  setState("editing", null);
  if (!e || !newName || newName === e.name) return;
  const parent = e.path.slice(0, -e.name.length);
  const newPath = parent + newName;
  try {
    await renamePath(e.path, newPath);
  } catch (err) {
    await notifyError(t("common.error", { msg: errMsg(err) }));
  }
  await refreshPane(ed.pane);
}

export function cancelRename() {
  setState("editing", null);
}

async function uniqueFileName(dir: string, name: string): Promise<string> {
  if (!(await pathExists(joinPath(dir, name)))) return name;
  const { base, ext } = splitName(name);
  let n = 2;
  while (await pathExists(joinPath(dir, `${base} ${n}${ext}`))) n++;
  return `${base} ${n}${ext}`;
}

async function uniqueDirName(dir: string, name: string): Promise<string> {
  if (!(await pathExists(joinPath(dir, name)))) return name;
  let n = 2;
  while (await pathExists(joinPath(dir, `${name} ${n}`))) n++;
  return `${name} ${n}`;
}

export async function archiveAction() {
  const pane = state.active;
  const p = state[pane];
  const sel = selectedEntries(pane);
  if (sel.length === 0) return;

  // Wenn genau ein .zip ausgewählt ist → entpacken
  if (sel.length === 1 && !sel[0].isDir && sel[0].name.toLowerCase().endsWith(".zip")) {
    const entry = sel[0];
    const baseName = entry.name.slice(0, -4);
    const target = await uniqueDirName(p.cwd, baseName);
    const dstDir = joinPath(p.cwd, target);
    try {
      await zipExtract(entry.path, dstDir);
    } catch (err) {
      await notifyError(t("jobs.extractFailed", { msg: errMsg(err) }));
    }
    await refreshPane(pane);
    return;
  }

  // Sonst: ZIP erstellen
  const defaultName = sel.length === 1 ? `${sel[0].name}.zip` : "archiv.zip";
  const name = await uniqueFileName(p.cwd, defaultName);
  const dst = joinPath(p.cwd, name);
  try {
    await zipCreate(sel.map((e) => e.path), dst);
  } catch (err) {
    await notifyError(t("jobs.zipFailed", { msg: errMsg(err) }));
  }
  await refreshPane(pane);
}

export async function pasteFromClipboard(targetPane: PaneId = state.active) {
  if (state.job) return;
  let paths: string[] = [];
  try {
    paths = await clipboardReadFiles();
  } catch (e) {
    console.error("clipboardReadFiles failed", e);
    return;
  }
  if (!paths || paths.length === 0) return;
  const dstCwd = state[targetPane].cwd;
  const entries: Entry[] = paths.map((p) => {
    const clean = p.replace(/\/+$/, "");
    const name = clean.split("/").pop() || clean;
    return {
      name,
      path: clean,
      isDir: false,
      isSymlink: false,
      size: 0,
      mtime: 0,
      ext: "",
      hidden: name.startsWith("."),
    };
  });
  await transferEntries("copy", entries, dstCwd, [targetPane]);
}

// Synchronisiert von einer Pane zur anderen.
// direction: "right" = links -> rechts; "left" = rechts -> links.
// In "nav"-Modus wird nur das Verzeichnis im Ziel-Pane gewechselt.
// In "merge"-Modus werden fehlende Einträge der Quelle ins Ziel kopiert
// (vorhandene Dateien werden übersprungen).
export async function syncPanes(direction: "left" | "right") {
  const src: PaneId = direction === "right" ? "left" : "right";
  const dst: PaneId = direction === "right" ? "right" : "left";
  const srcCwd = state[src].cwd;
  if (!srcCwd) return;
  if (state.syncMode === "nav") {
    await loadPane(dst, srcCwd);
    return;
  }
  // merge
  const entries = state[src].entriesRaw;
  if (!entries || entries.length === 0) return;
  const dstCwd = state[dst].cwd;
  if (srcCwd === dstCwd) return;
  await transferEntries("copy", entries, dstCwd, [src, dst], "skip");
}
