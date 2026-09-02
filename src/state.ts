import { createSignal } from "solid-js";
import { createStore, type SetStoreFunction } from "solid-js/store";
import type { Entry, PaneId, SortKey, SortDir } from "./types";
import {
  listDir,
  watchPath,
  pathExists,
  pathIsNetwork,
  navigationRoot,
  homeDir,
  unwatchPane,
} from "./ipc";
import { errMsg } from "./i18n";

export type PaneState = {
  cwd: string;
  history: string[];
  historyIndex: number;
  entriesRaw: Entry[]; // ungefiltert, sortiert
  entries: Entry[]; // sichtbar nach Filter
  cursor: number; // Index in entries (sorted)
  selected: Set<string>; // Paths
  anchor: number | null; // Anker für Shift-Klick
  /** Netzlaufwerke nutzen ein einheitliches DualBeam-Symbolsystem. */
  isNetwork: boolean;
  /** Oberhalb dieser WebDAV-/Objekt-Speicherwurzel gibt es keine Pane-Navigation. */
  navigationRoot?: string;
  loading: boolean;
  error: string | null;
  sortKey: SortKey;
  sortDir: SortDir;
  filter: string; // Substring-Filter (case-insensitive)
};

export type Tab = {
  cwd: string;
  history: string[];
  historyIndex: number;
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
  followMode: boolean;
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
    filesDone: number;
    transferPercent?: number;
    indeterminate?: boolean;
    current: string;
  } | null;
};

const emptyPane = (): PaneState => ({
  cwd: "",
  history: [],
  historyIndex: -1,
  entriesRaw: [],
  entries: [],
  cursor: 0,
  selected: new Set<string>(),
  anchor: null,
  isNetwork: false,
  loading: false,
  error: null,
  sortKey: "name",
  sortDir: "asc",
  filter: "",
});

const emptyTab = (): Tab => ({
  cwd: "",
  history: [],
  historyIndex: -1,
  sortKey: "name",
  sortDir: "asc",
  filter: "",
});

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
  followMode: false,
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

// Direkte SFTP-Löschungen passieren außerhalb des NFS-Mounts. Bis dessen
// Verzeichnis-Cache den fehlenden Eintrag selbst liefert, darf eine alte
// Listing-Antwort einen bereits serverseitig bestätigten Ordner nicht wieder
// sichtbar machen. Der Marker wird beim ersten frischen Listing ohne diesen
// Pfad automatisch entfernt.
const pendingSftpDeletes = new Set<string>();
const pendingSftpCopies = new Map<string, Entry>();

export function suppressDeletedSftpEntries(paths: string[]) {
  for (const path of paths) pendingSftpDeletes.add(path);
}

/** Zeigt serverseitig bestätigte SFTP-Kopien, bis der NFS-Mount sie selbst
 * auflistet. So führt ein direktes rclone-Upload nicht zu einer leeren Pane,
 * nur weil deren NFS-Listing noch im Cache liegt. */
export function retainConfirmedSftpCopies(entries: Entry[]) {
  for (const entry of entries) {
    pendingSftpDeletes.delete(entry.path);
    pendingSftpCopies.set(entry.path, entry);
  }
}

function isDirectChild(path: string, candidate: string) {
  const prefix = path.endsWith("/") ? path : `${path}/`;
  return candidate.startsWith(prefix) && !candidate.slice(prefix.length).includes("/");
}

function reconcilePendingSftpEntries(path: string, raw: Entry[]): Entry[] {
  if (pendingSftpDeletes.size === 0 && pendingSftpCopies.size === 0) return raw;
  const listed = new Set(raw.map((entry) => entry.path));
  for (const deleted of pendingSftpDeletes) {
    // Nur der aktuell eingelesene Elternordner kann zuverlässig bestätigen,
    // dass sein direkter Eintrag nicht mehr vorhanden ist.
    if (isDirectChild(path, deleted) && !listed.has(deleted)) pendingSftpDeletes.delete(deleted);
  }
  for (const copied of pendingSftpCopies.keys()) {
    // Sobald der Mount den Eintrag selbst liefert, wird der temporäre
    // Anzeigewert nicht mehr benötigt.
    if (isDirectChild(path, copied) && listed.has(copied)) pendingSftpCopies.delete(copied);
  }
  const visible = raw.filter((entry) => !pendingSftpDeletes.has(entry.path));
  for (const [copiedPath, copied] of pendingSftpCopies) {
    if (isDirectChild(path, copiedPath) && !listed.has(copiedPath)) visible.push(copied);
  }
  return visible;
}

// Signal um Filter-Input in einer Pane zu fokussieren.
export const [focusFilterTick, setFocusFilterTick] = createSignal<{
  pane: PaneId;
  n: number;
} | null>(null);
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

export function sortEntries(
  entries: Entry[],
  key: SortKey,
  dir: SortDir,
): Entry[] {
  const sign = dir === "asc" ? 1 : -1;
  const group = (e: Entry) => {
    if (e.isDir && e.ext !== "app") return 0; // Ordner
    if (e.isDir && e.ext === "app") return 1; // Apps
    return 2; // Dateien
  };
  return [...entries].sort((a, b) => {
    const ga = group(a),
      gb = group(b);
    if (ga !== gb) return ga - gb;
    let cmp = 0;
    switch (key) {
      case "name":
        cmp = a.name.localeCompare(b.name, undefined, {
          numeric: true,
          sensitivity: "base",
        });
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

function normalizedPath(path: string): string {
  return path.length > 1 ? path.replace(/\/+$/, "") : path;
}

/** macOS löst vorhandene Pfade unter `/Users` häufig als
 * `/System/Volumes/Data/Users` auf. Für die Navigationsgrenze beschreibt
 * beides denselben Ort; ohne diese Normalisierung wäre der Aufwärts-Button
 * an einem Objekt-Speicher-Mount fälschlich wieder aktiv. */
function comparablePath(path: string): string {
  const normalized = normalizedPath(path);
  const dataPrefix = "/System/Volumes/Data";
  return normalized.startsWith(`${dataPrefix}/`)
    ? normalized.slice(dataPrefix.length)
    : normalized;
}

/** Der Aufwärtsweg endet an der sichtbaren WebDAV-/Objekt-Speicherwurzel. */
export function canNavigateUp(pane: PaneId): boolean {
  const current = comparablePath(state[pane].cwd);
  const root = state[pane].navigationRoot;
  return !!current && current !== "/" && (!root || current !== comparablePath(root));
}

const MAX_HISTORY_ENTRIES = 100;
const paneLoadGeneration: Record<PaneId, number> = { left: 0, right: 0 };

type LoadPaneOptions = {
  recordHistory?: boolean;
  historyIndex?: number;
  /** Beim Öffnen eines Objekt-Speichers explizit gesetzte sichtbare Wurzel. */
  navigationRoot?: string;
};

function pushHistory(pane: PaneId, path: string) {
  const { history, historyIndex } = state[pane];
  if (historyIndex >= 0 && history[historyIndex] === path) return;

  const next = history.slice(0, historyIndex + 1);
  next.push(path);
  const trimmed = next.slice(-MAX_HISTORY_ENTRIES);
  setState(pane, { history: trimmed, historyIndex: trimmed.length - 1 });
}

export async function loadPane(
  pane: PaneId,
  path: string,
  options: LoadPaneOptions = {},
) {
  const generation = ++paneLoadGeneration[pane];
  const isCurrent = () => paneLoadGeneration[pane] === generation;
  setState(pane, "loading", true);
  setState(pane, "error", null);
  let target = path;
  // Netzlaufwerke (WebDAV/SMB…, i. d. R. unter /Volumes) sind langsam:
  // ein zusätzliches pathExists wäre ein weiterer Server-Roundtrip vor listDir.
  // pathIsNetwork liest dagegen nur die lokale Mount-Tabelle (kein Server-Zugriff).
  // Für erkannte Netzpfade daher die Existenz-/Eltern-Prüfung überspringen und
  // direkt listen; das Ausweichen passiert erst bei einem echten listDir-Fehler.
  let isNet = false;
  let root: string | undefined;
  // Eigene direkte S3-/Swift-Dateiräume liegen bewusst unter Application
  // Support statt unter /Volumes. Auch sie dürfen nicht mit `pathExists`
  // geprüft werden: ihre Unterordner sind virtuelle Objekt-Präfixe und haben
  // keinen lokalen Verzeichniseintrag. Die Backend-Prüfung ist lokal und
  // erkennt sowohl diese Dateiräume als auch klassische Netz-Mounts.
  try {
    isNet = await pathIsNetwork(target);
  } catch {}
  if (!isCurrent()) return;
  setState(pane, "isNetwork", isNet);
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
    if (!isCurrent()) return;
  }
  const existingRoot = state[pane].navigationRoot;
  if (options.navigationRoot !== undefined) {
    root = options.navigationRoot;
  } else if (
    existingRoot &&
    (comparablePath(target) === comparablePath(existingRoot) ||
      comparablePath(target).startsWith(`${comparablePath(existingRoot)}/`))
  ) {
    // Einmal beim Öffnen festgelegte Objekt-Speicherwurzeln bleiben für alle
    // untergeordneten Ordner erhalten. macOS kennt diese virtuellen Pfade
    // nicht und darf die UI-Grenze deshalb nicht wieder auf den technischen
    // App-Support-Ordner zurücksetzen.
    root = existingRoot;
  } else {
    try {
      root = (await navigationRoot(target)) ?? undefined;
    } catch {
      // Ohne lokale Mount-Information bleibt die allgemeine Navigation erhalten.
    }
  }
  if (!isCurrent()) return;
  try {
    const raw = await listDir(target, state.showHidden);
    if (!isCurrent()) return;
    const freshRaw = reconcilePendingSftpEntries(target, raw);
    const sorted = sortEntries(freshRaw, state[pane].sortKey, state[pane].sortDir);
    const filter = state[pane].filter;
    const visible = applyFilter(sorted, filter);
    setState(pane, {
      cwd: target,
      entriesRaw: sorted,
      entries: visible,
      cursor: 0,
      selected: new Set(),
      anchor: null,
      navigationRoot: root,
      loading: false,
    });
    if (options.historyIndex !== undefined) {
      setState(pane, "historyIndex", options.historyIndex);
    } else if (options.recordHistory !== false) {
      pushHistory(pane, target);
    }
    syncActiveTab(pane);
    bumpSel();
    if (isCurrent()) watchPath(pane, target).catch(() => {});
  } catch (e) {
    if (!isCurrent()) return;
    // Netzpfad nicht erreichbar (z. B. Freigabe ausgehängt): auf Home ausweichen,
    // damit die App sofort nutzbar bleibt, statt nur eine Fehlermeldung zu zeigen.
    if (isNet) {
      try {
        const home = await homeDir();
        if (!isCurrent()) return;
        if (home && home !== target) {
          await loadPane(pane, home, options);
          return;
        }
      } catch {}
    }
    setState(pane, { loading: false, error: errMsg(e) });
  }
}

export async function refreshPane(pane: PaneId) {
  await loadPane(pane, state[pane].cwd, { recordHistory: false });
}

// Ein manueller Refresh liest nur neu; er verändert auch auf Netzlaufwerken
// keine Dateien, um den Cache des Betriebssystems anzustoßen.
export async function forceRefreshPane(pane: PaneId) {
  await loadPane(pane, state[pane].cwd, { recordHistory: false });
}

export async function forceRefreshAll() {
  await Promise.all([forceRefreshPane("left"), forceRefreshPane("right")]);
}

// Aktualisiert beide Panes. Damit wird auch ein im inaktiven Pane geöffnetes
// Netzlaufwerk (z. B. WebDAV/SMB) neu eingelesen. Jeder loadPane-Aufruf
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
      try {
        await unwatchPane(pane);
      } catch {}
      await loadPane(pane, cwd, { recordHistory: false });
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
  followFrom(pane);
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
  setState(pane, {
    sortKey: key,
    sortDir: dir,
    entriesRaw: sortedRaw,
    entries: visible,
    cursor: 0,
  });
}

export function setFilter(pane: PaneId, filter: string) {
  const cur = state[pane];
  const visible = applyFilter(cur.entriesRaw, filter);
  // Auswahl auf sichtbare Pfade beschränken.
  const visibleSet = new Set(visible.map((e) => e.path));
  const newSel = new Set<string>();
  for (const p of cur.selected) if (visibleSet.has(p)) newSel.add(p);
  setState(pane, {
    filter,
    entries: visible,
    cursor: 0,
    selected: newSel,
    anchor: null,
  });
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

let followTimer: ReturnType<typeof setTimeout> | undefined;

// Folgemodus ("Follow"): Wird im aktiven Pane ein Ordner ausgewählt (per Klick
// oder Cursor-Taste), öffnet der jeweils andere Pane genau diesen Ordner und
// zeigt dessen Inhalt. So lässt sich ein Verzeichnisbaum bequem zweispaltig
// durchblättern, ohne den aktiven Pane zu verlassen.
export function followFrom(pane: PaneId) {
  if (!state.followMode) return;
  const p = state[pane];
  const e = p.entries[p.cursor];
  if (!e || !e.isDir) return;
  // .app-Bundles sind technisch Ordner, sollen aber wie Programme behandelt
  // und nicht "betreten" werden.
  if (e.name.toLowerCase().endsWith(".app")) return;
  const other: PaneId = pane === "left" ? "right" : "left";
  if (state[other].cwd === e.path) return; // schon dort – kein Reload nötig
  const target = e.path;
  // Kleiner Debounce: schnelles Durchscrollen mit den Pfeiltasten soll nicht
  // für jeden Zwischenschritt ein (evtl. langsames Netz-)listDir auslösen.
  if (followTimer) clearTimeout(followTimer);
  followTimer = setTimeout(() => {
    followTimer = undefined;
    // Zustand erneut prüfen: Der Modus kann inzwischen ausgeschaltet oder die
    // Auswahl weitergewandert sein – dann nichts mehr laden.
    if (!state.followMode) return;
    const cur = state[pane];
    const nowSelected = cur.entries[cur.cursor];
    if (!nowSelected || nowSelected.path !== target) return;
    if (state[other].cwd === target) return;
    void loadPane(other, target, { recordHistory: false });
  }, 60);
}

export function toggleFollowMode() {
  const next = !state.followMode;
  setState("followMode", next);
  if (next) {
    // Beim Einschalten sofort den aktuell markierten Ordner spiegeln.
    followFrom(state.active);
  } else if (followTimer) {
    // Beim Ausschalten einen noch ausstehenden Folge-Ladevorgang verwerfen.
    clearTimeout(followTimer);
    followTimer = undefined;
  }
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
    history: s.history,
    historyIndex: s.historyIndex,
    sortKey: s.sortKey,
    sortDir: s.sortDir,
    filter: s.filter,
  });
}

export function newTab(pane: PaneId, path?: string) {
  // Aktuelle Tab zuerst synchronisieren
  syncActiveTab(pane);
  const target = path ?? state[pane].cwd;
  const newTab: Tab = {
    cwd: target,
    history: [],
    historyIndex: -1,
    sortKey: "name",
    sortDir: "asc",
    filter: "",
  };
  setState("tabs", pane, (arr) => [...arr, newTab]);
  const newIdx = state.tabs[pane].length - 1;
  setState("activeTab", pane, newIdx);
  // PaneState auf Defaults zurücksetzen für neuen Tab
  setState(pane, {
    history: [],
    historyIndex: -1,
    sortKey: "name",
    sortDir: "asc",
    filter: "",
  });
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
    setState(pane, {
      history: t.history,
      historyIndex: t.historyIndex,
      sortKey: t.sortKey,
      sortDir: t.sortDir,
      filter: t.filter,
    });
    loadPane(pane, t.cwd, { recordHistory: false });
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
  setState(pane, {
    history: t.history,
    historyIndex: t.historyIndex,
    sortKey: t.sortKey,
    sortDir: t.sortDir,
    filter: t.filter,
  });
  loadPane(pane, t.cwd, { recordHistory: false });
}

export function closeActiveTab(pane: PaneId) {
  closeTab(pane, state.activeTab[pane]);
}

export async function goBackInHistory(pane: PaneId) {
  const { history, historyIndex } = state[pane];
  if (historyIndex <= 0) return;
  await loadPane(pane, history[historyIndex - 1], {
    recordHistory: false,
    historyIndex: historyIndex - 1,
  });
}

// Hilfs-Setter falls außerhalb benötigt
export const _set: SetStoreFunction<AppState> = setState;
