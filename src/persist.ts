import { createEffect } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { state, setState, type Tab } from "./state";
import type { PaneId, SortKey, SortDir } from "./types";

// Zustand pro Fenster getrennt speichern. Das Hauptfenster ("main")
// behält den ursprünglichen Schlüssel; weitere Fenster bekommen einen
// Suffix mit ihrem Label und starten dadurch unabhängig.
const BASE_KEY = "dualbeam:v1";
const KEY = (() => {
  try {
    const label = getCurrentWindow().label;
    return label === "main" ? BASE_KEY : `${BASE_KEY}:${label}`;
  } catch {
    return BASE_KEY;
  }
})();

type Persisted = {
  version: 1;
  active: PaneId;
  sidebarVisible: boolean;
  previewVisible: boolean;
  showHidden: boolean;
  helpVisible: boolean;
  extendedView: boolean;
  sidebarWidth: number;
  previewWidth: number;
  paneSplit: number;
  panes: {
    left: { activeTab: number; tabs: Tab[] };
    right: { activeTab: number; tabs: Tab[] };
  };
};

function sanitizeTab(t: any): Tab | null {
  if (!t || typeof t.cwd !== "string" || !t.cwd) return null;
  const sortKey: SortKey =
    t.sortKey === "size" || t.sortKey === "mtime" ? t.sortKey : "name";
  const sortDir: SortDir = t.sortDir === "desc" ? "desc" : "asc";
  const filter = typeof t.filter === "string" ? t.filter : "";
  const storedHistory = Array.isArray(t.history)
    ? t.history.filter((path: unknown): path is string => typeof path === "string" && path.length > 0).slice(-100)
    : [];
  const history = storedHistory.length ? storedHistory : [t.cwd];
  const historyIndex = typeof t.historyIndex === "number"
    ? Math.max(0, Math.min(history.length - 1, t.historyIndex))
    : history.length - 1;
  return { cwd: t.cwd, history, historyIndex, sortKey, sortDir, filter };
}

export function loadPersisted(): Persisted | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const p = JSON.parse(raw);
    if (!p || p.version !== 1) return null;
    const left = (p.panes?.left?.tabs ?? []).map(sanitizeTab).filter(Boolean) as Tab[];
    const right = (p.panes?.right?.tabs ?? []).map(sanitizeTab).filter(Boolean) as Tab[];
    if (!left.length || !right.length) return null;
    const clamp = (i: any, n: number) =>
      typeof i === "number" && i >= 0 && i < n ? i : 0;
    return {
      version: 1,
      active: p.active === "right" ? "right" : "left",
      sidebarVisible: p.sidebarVisible !== false,
      previewVisible: !!p.previewVisible,
      showHidden: !!p.showHidden,
      helpVisible: !!p.helpVisible,
      extendedView: !!p.extendedView,
      sidebarWidth: typeof p.sidebarWidth === "number" && p.sidebarWidth >= 120 && p.sidebarWidth <= 500 ? p.sidebarWidth : 200,
      previewWidth: typeof p.previewWidth === "number" && p.previewWidth >= 160 && p.previewWidth <= 700 ? p.previewWidth : 280,
      paneSplit: typeof p.paneSplit === "number" && p.paneSplit > 0.05 && p.paneSplit < 0.95 ? p.paneSplit : 0.5,
      panes: {
        left: { activeTab: clamp(p.panes.left.activeTab, left.length), tabs: left },
        right: { activeTab: clamp(p.panes.right.activeTab, right.length), tabs: right },
      },
    };
  } catch {
    return null;
  }
}

function snapshot(): Persisted {
  // Aktuellen Pane-Status in den aktiven Tab spiegeln, damit cwd/sort/filter aktuell sind.
  const mirror = (pane: PaneId): { activeTab: number; tabs: Tab[] } => {
    const tabs = state.tabs[pane].map((t, i) => {
      if (i !== state.activeTab[pane]) return { ...t };
      const s = state[pane];
      return {
        cwd: s.cwd || t.cwd,
        history: s.history,
        historyIndex: s.historyIndex,
        sortKey: s.sortKey,
        sortDir: s.sortDir,
        filter: s.filter,
      };
    });
    return { activeTab: state.activeTab[pane], tabs };
  };
  return {
    version: 1,
    active: state.active,
    sidebarVisible: state.sidebarVisible,
    previewVisible: state.previewVisible,
    showHidden: state.showHidden,
    helpVisible: state.helpVisible,
    extendedView: state.extendedView,
    sidebarWidth: state.sidebarWidth,
    previewWidth: state.previewWidth,
    paneSplit: state.paneSplit,
    panes: { left: mirror("left"), right: mirror("right") },
  };
}

let saveTimer: number | null = null;
function scheduleSave() {
  if (saveTimer !== null) window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    saveTimer = null;
    try {
      localStorage.setItem(KEY, JSON.stringify(snapshot()));
    } catch {
      /* ignore quota / private mode */
    }
  }, 300);
}

export function attachPersist() {
  // Reaktiv auf alle relevanten Felder reagieren.
  createEffect(() => {
    // Felder lesen, damit createEffect sie trackt.
    void state.active;
    void state.sidebarVisible;
    void state.previewVisible;
    void state.showHidden;
    void state.helpVisible;
    void state.extendedView;
    void state.sidebarWidth;
    void state.previewWidth;
    void state.paneSplit;
    void state.activeTab.left;
    void state.activeTab.right;
    void state.tabs.left.length;
    void state.tabs.right.length;
    void state.left.cwd;
    void state.right.cwd;
    void state.left.history.length;
    void state.right.history.length;
    void state.left.historyIndex;
    void state.right.historyIndex;
    void state.left.sortKey;
    void state.left.sortDir;
    void state.left.filter;
    void state.right.sortKey;
    void state.right.sortDir;
    void state.right.filter;
    scheduleSave();
  });
}

export function applyPersisted(p: Persisted) {
  setState({
    sidebarVisible: p.sidebarVisible,
    previewVisible: p.previewVisible,
    showHidden: p.showHidden,
    helpVisible: p.helpVisible,
    extendedView: p.extendedView,
    sidebarWidth: p.sidebarWidth,
    previewWidth: p.previewWidth,
    paneSplit: p.paneSplit,
    active: p.active,
    tabs: { left: p.panes.left.tabs, right: p.panes.right.tabs },
    activeTab: { left: p.panes.left.activeTab, right: p.panes.right.activeTab },
  });
  // Aktiven Tab-State in PaneState spiegeln (cwd lädt der Aufrufer via loadPane).
  const lt = p.panes.left.tabs[p.panes.left.activeTab];
  const rt = p.panes.right.tabs[p.panes.right.activeTab];
  setState("left", {
    history: lt.history,
    historyIndex: lt.historyIndex,
    sortKey: lt.sortKey,
    sortDir: lt.sortDir,
    filter: lt.filter,
  });
  setState("right", {
    history: rt.history,
    historyIndex: rt.historyIndex,
    sortKey: rt.sortKey,
    sortDir: rt.sortDir,
    filter: rt.filter,
  });
}
