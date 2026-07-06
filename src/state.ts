import { createSignal } from "solid-js";
import { createStore, type SetStoreFunction } from "solid-js/store";
import type { Entry, PaneId, SortKey, SortDir } from "./types";
import { listDir, watchPath, pathExists, pathIsNetwork, homeDir, unwatchPane } from "./ipc";
import { errMsg } from "./i18n";

export type PaneState = {
  cwd: string;
  entriesRaw: Entry[];      // ungefiltert, sortiert
  entries: Entry[];         // sichtbar nach Filter
  cursor: number;          // Index in entries (sorted)
  selected: Set<string>;   // Paths
  anchor: number | null;   // Anker für Shift-Klick
  loading: boolean;
  error: string | null;
  sortKey: SortKey;
  sortDir: SortDir;
  filter: string;          // Substring-Filter (case-insensitive)
};

export type Tab = {
  cwd: string;
  sortKey: SortKey;
  sortDir: SortDir;
  filter: string;
};

export type AppState = {
  left: PaneState;
  right: PaneState;
  tabs: { left: Tab[]; right: Tab[] };
  activeTab: { left: number; right: number };
  active: PaneId;
  showHidden: boolean;
  sidebarVisible: boolean;
  previewVisible: boolean;
  helpVisible: boolean;
  extendedView: boolean;
  compareMode: boolean;
  syncMode: "nav" | "merge";
  sidebarWidth: number;
  previewWidth: number;
  paneSplit: number; // 0..1 Anteil der linken Pane an der verfügbaren Pane-Breite
  editing: { pane: PaneId; idx: number } | null;
  job: {
    id: string;
    kind: "copy" | "move" | "delete";
    done: number;
    total: number;
    current: string;
  } | null;
};

const emptyPane = (): PaneState => ({
  cwd: "",
  entriesRaw: [],
  entries: [],
  cursor: 0,
  selected: new Set<string>(),
  anchor: null,
  loading: false,
  error: null,
  sortKey: "name",
  sortDir: "asc",
  filter: "",
});

const emptyTab = (): Tab => ({ cwd: "", sortKey: "name", sortDir: "asc", filter: "" });

export const [state, setState] = createStore<AppState>({
  left: emptyPane(),
  right: emptyPane(),
  tabs: { left: [emptyTab()], right: [emptyTab()] },
  activeTab: { left: 0, right: 0 },
  active: "left",
  showHidden: false,
  sidebarVisible: true,
  previewVisible: false,
  helpVisible: false,
  extendedView: false,
  compareMode: false,
  syncMode: "nav",
  sidebarWidth: 200,
  previewWidth: 280,
  paneSplit: 0.5,
  editing: null,
  job: null,
});

// Force-update tick zum Re-Rendern wenn sich Set-Inhalte ändern
export const [selTick, setSelTick] = createSignal(0);
const bumpSel = () => setSelTick((n) => n + 1);

// Signal um Filter-Input in einer Pane zu fokussieren.
export const [focusFilterTick, setFocusFilterTick] = createSignal<{ pane: PaneId; n: number } | null>(null);
let ffCounter = 0;
export function requestFocusFilter(pane: PaneId) {
  ffCounter += 1;
  setFocusFilterTick({ pane, n: ffCounter });
}

// Signal um die Volumes-Liste in der Sidebar sofort neu zu laden.
export const [volumesTick, setVolumesTick] = createSignal(0);
export function bumpVolumes() {
  setVolumesTick((n) => n + 1);
}

function applyFilter(raw: Entry[], filter: string): Entry[] {
  if (!filter) return raw;
  const f = filter.toLowerCase();
  return raw.filter((e) => e.name.toLowerCase().includes(f));
}

export function sortEntries(entries: Entry[], key: SortKey, dir: SortDir): Entry[] {
  const sign = dir === "asc" ? 1 : -1;
  const group = (e: Entry) => {
    if (e.isDir && e.ext !== "app") return 0; // Ordner
    if (e.isDir && e.ext === "app") return 1; // Apps
    return 2;                                  // Dateien
  };
  return [...entries].sort((a, b) => {
    const ga = group(a), gb = group(b);
    if (ga !== gb) return ga - gb;
    let cmp = 0;
    switch (key) {
      case "name":
        cmp = a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
        break;
      case "size":
        cmp = a.size - b.size;
        break;
      case "mtime":
        cmp = a.mtime - b.mtime;
        break;
    }
    return cmp * sign;
  });
}

export async function loadPane(pane: PaneId, path: string) {
  setState(pane, "loading", true);
  setState(pane, "error", null);
  let target = path;
  // Netzlaufwerke (HiDrive/WebDAV/SMB…, i. d. R. unter /Volumes) sind langsam:
  // ein zusätzliches pathExists wäre ein weiterer Server-Roundtrip vor listDir.
  // pathIsNetwork liest dagegen nur die lokale Mount-Tabelle (kein Server-Zugriff).
  // Für erkannte Netzpfade daher die Existenz-/Eltern-Prüfung überspringen und
  // direkt listen; das Ausweichen passiert erst bei einem echten listDir-Fehler.
  let isNet = false;
  if (target.startsWith("/Volumes/")) {
    try {
      isNet = await pathIsNetwork(target);
    } catch {}
  }
  if (!isNet) {
    // Fallback, falls Pfad verschwunden ist (z.B. ausgeworfenes Volume / unmounted DMG):
    // an erstes existierendes Eltern-Verzeichnis (oder Home) ausweichen.
    try {
      if (!(await pathExists(target))) {
        let probe = target;
        while (probe && probe !== "/" && !(await pathExists(probe))) {
          const idx = probe.lastIndexOf("/");
          probe = idx <= 0 ? "/" : probe.slice(0, idx);
        }
        if (!probe || !(await pathExists(probe))) {
          probe = await homeDir();
        }
        target = probe;
      }
    } catch {
      // pathExists/homeDir-Fehler ignorieren; listDir liefert ggf. eigene Fehlermeldung.
    }
  }
  try {
    const raw = await listDir(target, state.showHidden);
    const sorted = sortEntries(raw, state[pane].sortKey, state[pane].sortDir);
    const filter = state[pane].filter;
    const visible = applyFilter(sorted, filter);
    setState(pane, {
      cwd: target,
      entriesRaw: sorted,
      entries: visible,
      cursor: 0,
      selected: new Set(),
      anchor: null,
      loading: false,
    });
    syncActiveTab(pane);
    bumpSel();
    watchPath(pane, target).catch(() => {});
  } catch (e) {
    // Netzpfad nicht erreichbar (z. B. HiDrive ausgehängt): auf Home ausweichen,
    // damit die App sofort nutzbar bleibt, statt nur eine Fehlermeldung zu zeigen.
    if (isNet) {
      try {
        const home = await homeDir();
        if (home && home !== target) {
          await loadPane(pane, home);
          return;
        }
      } catch {}
    }
    setState(pane, { loading: false, error: errMsg(e) });
  }
}

export async function refreshPane(pane: PaneId) {
  await loadPane(pane, state[pane].cwd);
}

// Aktualisiert beide Panes. Damit wird auch ein im inaktiven Pane geöffnetes
// Netzlaufwerk (z. B. HiDrive/WebDAV) neu eingelesen. Jeder loadPane-Aufruf
// löst im Backend ein frisches read_dir aus, was bei webdavfs einen neuen
// PROPFIND und damit einen serverseitigen Refresh bewirkt.
export async function refreshAll() {
  await Promise.all([refreshPane("left"), refreshPane("right")]);
}

export async function handleVolumeGone(volPath: string) {
  const norm = volPath.endsWith("/") ? volPath : volPath + "/";
  const panes: PaneId[] = ["left", "right"];
  for (const pane of panes) {
    const cwd = state[pane].cwd;
    if (cwd === volPath || cwd.startsWith(norm)) {
      try { await unwatchPane(pane); } catch {}
      await loadPane(pane, cwd);
    }
  }
}

export function setActive(pane: PaneId) {
  setState("active", pane);
}

export function setCursor(pane: PaneId, idx: number) {
  const max = state[pane].entries.length - 1;
  const clamped = Math.max(0, Math.min(max, idx));
  setState(pane, "cursor", clamped);
}

export function selectOnly(pane: PaneId, idx: number) {
  const e = state[pane].entries[idx];
  if (!e) return;
  const sel = new Set<string>([e.path]);
  setState(pane, { selected: sel, cursor: idx, anchor: idx });
  bumpSel();
}

export function toggleSelect(pane: PaneId, idx: number) {
  const e = state[pane].entries[idx];
  if (!e) return;
  const sel = new Set(state[pane].selected);
  if (sel.has(e.path)) sel.delete(e.path);
  else sel.add(e.path);
  setState(pane, { selected: sel, cursor: idx, anchor: idx });
  bumpSel();
}

export function selectRange(pane: PaneId, idx: number) {
  const anchor = state[pane].anchor ?? state[pane].cursor;
  const [a, b] = anchor <= idx ? [anchor, idx] : [idx, anchor];
  const sel = new Set<string>();
  for (let i = a; i <= b; i++) {
    const e = state[pane].entries[i];
    if (e) sel.add(e.path);
  }
  setState(pane, { selected: sel, cursor: idx });
  bumpSel();
}

export function clearSelection(pane: PaneId) {
  setState(pane, { selected: new Set(), anchor: null });
  bumpSel();
}

export function toggleHidden() {
  setState("showHidden", (v) => !v);
  refreshPane("left");
  refreshPane("right");
}

export function setSort(pane: PaneId, key: SortKey) {
  const cur = state[pane];
  const dir: SortDir =
    cur.sortKey === key ? (cur.sortDir === "asc" ? "desc" : "asc") : "asc";
  const sortedRaw = sortEntries(cur.entriesRaw, key, dir);
  const visible = applyFilter(sortedRaw, cur.filter);
  setState(pane, { sortKey: key, sortDir: dir, entriesRaw: sortedRaw, entries: visible, cursor: 0 });
}

export function setFilter(pane: PaneId, filter: string) {
  const cur = state[pane];
  const visible = applyFilter(cur.entriesRaw, filter);
  // Auswahl auf sichtbare Pfade beschränken.
  const visibleSet = new Set(visible.map((e) => e.path));
  const newSel = new Set<string>();
  for (const p of cur.selected) if (visibleSet.has(p)) newSel.add(p);
  setState(pane, { filter, entries: visible, cursor: 0, selected: newSel, anchor: null });
  bumpSel();
}

export function toggleSidebar() {
  setState("sidebarVisible", (v) => !v);
}

export function togglePreview() {
  setState("previewVisible", (v) => !v);
}

export function toggleHelp() {
  setState("helpVisible", (v) => !v);
}

export function toggleCompareMode() {
  setState("compareMode", (v) => !v);
}

export function compareStatus(
  paneId: PaneId,
  e: Entry,
): "only" | "diff" | "same" | null {
  if (!state.compareMode) return null;
  const other = paneId === "left" ? "right" : "left";
  const list = state[other].entriesRaw;
  for (const o of list) {
    if (o.name !== e.name) continue;
    if (o.isDir !== e.isDir) return "only";
    if (e.isDir) return "same";
    const sameSize = o.size === e.size;
    const sameMt = Math.abs(o.mtime - e.mtime) < 2;
    return sameSize && sameMt ? "same" : "diff";
  }
  return "only";
}

// ---------- Tabs ----------

function syncActiveTab(pane: PaneId) {
  const idx = state.activeTab[pane];
  const s = state[pane];
  setState("tabs", pane, idx, {
    cwd: s.cwd,
    sortKey: s.sortKey,
    sortDir: s.sortDir,
    filter: s.filter,
  });
}

export function newTab(pane: PaneId, path?: string) {
  // Aktuelle Tab zuerst synchronisieren
  syncActiveTab(pane);
  const target = path ?? state[pane].cwd;
  const newTab: Tab = { cwd: target, sortKey: "name", sortDir: "asc", filter: "" };
  setState("tabs", pane, (arr) => [...arr, newTab]);
  const newIdx = state.tabs[pane].length - 1;
  setState("activeTab", pane, newIdx);
  // PaneState auf Defaults zurücksetzen für neuen Tab
  setState(pane, { sortKey: "name", sortDir: "asc", filter: "" });
  loadPane(pane, target);
}

export function closeTab(pane: PaneId, idx: number) {
  const tabs = state.tabs[pane];
  if (tabs.length <= 1) return;
  const arr = [...tabs];
  arr.splice(idx, 1);
  setState("tabs", pane, arr);
  let active = state.activeTab[pane];
  if (active === idx) {
    const newActive = Math.max(0, idx - 1);
    setState("activeTab", pane, newActive);
    const t = arr[newActive];
    setState(pane, { sortKey: t.sortKey, sortDir: t.sortDir, filter: t.filter });
    loadPane(pane, t.cwd);
  } else if (active > idx) {
    setState("activeTab", pane, active - 1);
  }
}

export function switchTab(pane: PaneId, idx: number) {
  if (idx < 0 || idx >= state.tabs[pane].length) return;
  if (idx === state.activeTab[pane]) return;
  syncActiveTab(pane);
  setState("activeTab", pane, idx);
  const t = state.tabs[pane][idx];
  setState(pane, { sortKey: t.sortKey, sortDir: t.sortDir, filter: t.filter });
  loadPane(pane, t.cwd);
}

export function closeActiveTab(pane: PaneId) {
  closeTab(pane, state.activeTab[pane]);
}

// Hilfs-Setter falls außerhalb benötigt
export const _set: SetStoreFunction<AppState> = setState;
