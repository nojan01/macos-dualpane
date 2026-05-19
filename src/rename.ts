// Multi-Rename mit gestapelten Operationen (Finder-Stil).
import { createSignal, createEffect } from "solid-js";
import { state, setState, refreshPane } from "./state";
import type { Entry, PaneId } from "./types";
import { renamePath, pathExists } from "./ipc";
import { selectedEntries } from "./jobs";
import { t } from "./i18n";

export type Scope = "full" | "base" | "ext";
export type ReplaceMode = "all" | "first" | "last" | "start" | "end";
export type NumberPos = "none" | "before" | "after";
export type CaseMode = "keep" | "lower" | "upper" | "title";

export type OpReplace = {
  kind: "replace";
  scope: Scope;
  find: string;
  replace: string;
  mode: ReplaceMode;
  regex: boolean;
  caseSensitive: boolean;
};
export type OpAdd = {
  kind: "add";
  scope: Scope;
  prefix: string;
  suffix: string;
  numberPos: NumberPos;
  numberStart: number;
  numberPad: number;
};
export type OpCase = {
  kind: "case";
  scope: Scope;
  caseMode: CaseMode;
};
export type OpSetExt = {
  kind: "setExt";
  ext: string; // mit oder ohne führendem Punkt; leer = Endung entfernen
};

export type Op = OpReplace | OpAdd | OpCase | OpSetExt;

export type RenameOptions = { ops: Op[] };

export const defaultOpReplace = (): OpReplace => ({
  kind: "replace", scope: "full", find: "", replace: "",
  mode: "all", regex: false, caseSensitive: false,
});
export const defaultOpAdd = (): OpAdd => ({
  kind: "add", scope: "base", prefix: "", suffix: "",
  numberPos: "none", numberStart: 1, numberPad: 2,
});
export const defaultOpCase = (): OpCase => ({
  kind: "case", scope: "base", caseMode: "keep",
});
export const defaultOpSetExt = (): OpSetExt => ({
  kind: "setExt", ext: "",
});

export const defaultRenameOptions: RenameOptions = { ops: [defaultOpReplace()] };

// ---- Persistenz & Presets ----
const STORAGE_KEY = "dualbeam:rename:v1";

export type RenamePreset = { name: string; ops: Op[] };
type Stored = { last?: RenameOptions; presets?: RenamePreset[] };

function readStored(): Stored {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const v = JSON.parse(raw);
    return v && typeof v === "object" ? v : {};
  } catch { return {}; }
}
function writeStored(s: Stored) {
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify(s)); } catch {}
}
function loadLast(): RenameOptions {
  const s = readStored();
  if (s.last && Array.isArray(s.last.ops) && s.last.ops.length > 0) return s.last;
  return { ops: [defaultOpReplace()] };
}
export const [renamePresets, setRenamePresets] = createSignal<RenamePreset[]>(readStored().presets ?? []);
export function savePreset(name: string) {
  const n = name.trim();
  if (!n) return;
  const cur = renamePresets().filter((p) => p.name !== n);
  const next = [...cur, { name: n, ops: renameOptions().ops }];
  setRenamePresets(next);
  const s = readStored(); s.presets = next; writeStored(s);
}
export function loadPreset(name: string) {
  const p = renamePresets().find((x) => x.name === name);
  if (p) setRenameOptions({ ops: p.ops.slice() });
}
export function deletePreset(name: string) {
  const next = renamePresets().filter((p) => p.name !== name);
  setRenamePresets(next);
  const s = readStored(); s.presets = next; writeStored(s);
}

export type RenameSession = {
  pane: PaneId;
  dir: string;
  items: Entry[];
};

export const [renameSession, setRenameSession] = createSignal<RenameSession | null>(null);
export const [renameOptions, setRenameOptions] = createSignal<RenameOptions>(loadLast());

createEffect(() => {
  const opts = renameOptions();
  const s = readStored();
  s.last = opts;
  writeStored(s);
});

export function openRenameDialog() {
  if (state.job) return;
  const pane = state.active;
  const items = selectedEntries(pane);
  if (items.length === 0) return;
  // Letzte Einstellung beibehalten; nichts zurücksetzen.
  setRenameSession({ pane, dir: state[pane].cwd, items });
}

export function closeRenameDialog() {
  setRenameSession(null);
}

function splitName(name: string): { base: string; ext: string } {
  const i = name.lastIndexOf(".");
  if (i <= 0 || i === name.length - 1) return { base: name, ext: "" };
  return { base: name.slice(0, i), ext: name.slice(i) };
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function padNum(n: number, pad: number): string {
  return String(n).padStart(Math.max(0, pad), "0");
}

function applyNumberToken(s: string, n: number, pad: number): string {
  return s.replace(/\{n\}/g, padNum(n, pad));
}

function toTitleCase(s: string): string {
  return s.replace(/\b\w/g, (c) => c.toUpperCase());
}

function applyCase(s: string, mode: CaseMode): string {
  if (mode === "lower") return s.toLowerCase();
  if (mode === "upper") return s.toUpperCase();
  if (mode === "title") return toTitleCase(s);
  return s;
}

// Wendet eine Op auf den nach Scope ausgewählten Teil an.
function applyOp(base: string, ext: string, op: Op, index: number): { base: string; ext: string; err?: string } {
  if (op.kind === "setExt") {
    const raw = op.ext.trim();
    if (!raw) return { base, ext: "" };
    const e = raw.startsWith(".") ? raw : "." + raw;
    return { base, ext: e };
  }
  const transform = (target: string): { v: string; err?: string } => {
    if (op.kind === "replace") {
      if (!op.find) return { v: target };
      try {
        const repl = applyNumberToken(op.replace, index, 0);
        if (op.regex) {
          const flags = (op.mode === "all" ? "g" : "") + (op.caseSensitive ? "" : "i");
          const re = new RegExp(op.find, flags);
          return { v: target.replace(re, repl) };
        }
        const f = escapeRegExp(op.find);
        if (op.mode === "start") {
          const re = new RegExp("^" + f, op.caseSensitive ? "" : "i");
          return { v: target.replace(re, repl) };
        }
        if (op.mode === "end") {
          const re = new RegExp(f + "$", op.caseSensitive ? "" : "i");
          return { v: target.replace(re, repl) };
        }
        if (op.mode === "last") {
          const hay = op.caseSensitive ? target : target.toLowerCase();
          const needle = op.caseSensitive ? op.find : op.find.toLowerCase();
          const idx = hay.lastIndexOf(needle);
          if (idx < 0) return { v: target };
          return { v: target.slice(0, idx) + repl + target.slice(idx + op.find.length) };
        }
        const flags = (op.mode === "all" ? "g" : "") + (op.caseSensitive ? "" : "i");
        const re = new RegExp(f, flags);
        return { v: target.replace(re, repl) };
      } catch (e: any) {
        return { v: target, err: `Regex: ${e?.message ?? e}` };
      }
    }
    if (op.kind === "add") {
      const pre = applyNumberToken(op.prefix, index, op.numberPad);
      const suf = applyNumberToken(op.suffix, index, op.numberPad);
      const numStr = padNum(op.numberStart + index - 1, op.numberPad);
      let out = target;
      if (op.numberPos === "before") out = numStr + out;
      out = pre + out + suf;
      if (op.numberPos === "after") out = out + numStr;
      return { v: out };
    }
    return { v: applyCase(target, op.caseMode) };
  };

  if (op.scope === "full") {
    const r = transform(base + ext);
    const sp = splitName(r.v);
    return { base: sp.base, ext: sp.ext, err: r.err };
  }
  if (op.scope === "ext") {
    const r = transform(ext);
    return { base, ext: r.v, err: r.err };
  }
  const r = transform(base);
  return { base: r.v, ext, err: r.err };
}

export type Preview = {
  src: Entry;
  newName: string;
  changed: boolean;
  error?: string;
};

export function computePreviews(items: Entry[], opts: RenameOptions): Preview[] {
  const previews: Preview[] = items.map((src, i) => {
    const idx = i + 1;
    let { base, ext } = splitName(src.name);
    let err: string | undefined;
    for (const op of opts.ops) {
      const r = applyOp(base, ext, op, idx);
      base = r.base;
      ext = r.ext;
      if (r.err && !err) err = r.err;
    }
    const newName = base + ext;
    return { src, newName, changed: newName !== src.name, error: err };
  });

  const counts = new Map<string, number>();
  for (const p of previews) counts.set(p.newName, (counts.get(p.newName) ?? 0) + 1);
  for (const p of previews) {
    if (p.error) continue;
    if (!p.newName) p.error = t("rn.err.empty");
    else if (/[\/]/.test(p.newName)) p.error = t("rn.err.invalidChar");
    else if ((counts.get(p.newName) ?? 0) > 1) p.error = t("rn.err.dup");
  }
  return previews;
}

function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? dir + name : dir + "/" + name;
}

export async function applyRename(): Promise<{ ok: boolean; message?: string }> {
  const sess = renameSession();
  if (!sess) return { ok: false };
  if (state.job) return { ok: false, message: t("rn.err.jobRunning") };

  const opts = renameOptions();
  const previews = computePreviews(sess.items, opts);

  if (previews.some((p) => p.error)) {
    return { ok: false, message: t("rn.err.conflicts") };
  }
  const targets = previews.filter((p) => p.changed);
  if (targets.length === 0) return { ok: true };

  const srcSet = new Set(sess.items.map((e) => e.path));
  for (const p of targets) {
    const dst = joinPath(sess.dir, p.newName);
    if (srcSet.has(dst)) continue;
    if (await pathExists(dst)) {
      return { ok: false, message: t("rn.err.exists", { name: p.newName }) };
    }
  }

  const needsTwoPhase = targets.some((p) => srcSet.has(joinPath(sess.dir, p.newName)));
  const stamp = Date.now();

  try {
    if (needsTwoPhase) {
      const tempPaths: { tmp: string; finalName: string }[] = [];
      for (let i = 0; i < targets.length; i++) {
        const p = targets[i];
        const tmp = joinPath(sess.dir, `.__rn_${stamp}_${i}__`);
        await renamePath(p.src.path, tmp);
        tempPaths.push({ tmp, finalName: p.newName });
      }
      for (const t of tempPaths) {
        await renamePath(t.tmp, joinPath(sess.dir, t.finalName));
      }
    } else {
      for (const p of targets) {
        await renamePath(p.src.path, joinPath(sess.dir, p.newName));
      }
    }
  } catch (err: any) {
    await refreshPane(sess.pane);
    return { ok: false, message: t("common.error", { msg: err?.message ?? err }) };
  }

  const newPaths = new Set(previews.map((p) => joinPath(sess.dir, p.newName)));
  await refreshPane(sess.pane);
  const fresh = state[sess.pane].entries
    .map((e, i) => ({ e, i }))
    .filter(({ e }) => newPaths.has(e.path));
  if (fresh.length > 0) {
    setState(sess.pane, "selected", new Set(fresh.map(({ e }) => e.path)));
    setState(sess.pane, "cursor", fresh[0].i);
  }

  closeRenameDialog();
  return { ok: true };
}

export function addOp(kind: Op["kind"]) {
  const opts = renameOptions();
  const op: Op = kind === "replace" ? defaultOpReplace()
    : kind === "add" ? defaultOpAdd()
    : kind === "case" ? defaultOpCase()
    : defaultOpSetExt();
  setRenameOptions({ ops: [...opts.ops, op] });
}

export function removeOp(i: number) {
  const opts = renameOptions();
  if (opts.ops.length <= 1) return;
  const ops = opts.ops.slice();
  ops.splice(i, 1);
  setRenameOptions({ ops });
}

export function updateOp(i: number, patch: Partial<Op>) {
  const opts = renameOptions();
  const ops = opts.ops.slice();
  ops[i] = { ...ops[i], ...patch } as Op;
  setRenameOptions({ ops });
}

export function changeOpKind(i: number, kind: Op["kind"]) {
  const opts = renameOptions();
  const ops = opts.ops.slice();
  ops[i] = kind === "replace" ? defaultOpReplace()
    : kind === "add" ? defaultOpAdd()
    : kind === "case" ? defaultOpCase()
    : defaultOpSetExt();
  setRenameOptions({ ops });
}
