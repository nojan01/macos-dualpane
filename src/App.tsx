import { createSignal, onMount } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { cycleThemeMode, getThemeMode, onThemeChange, themeIcon, themeLabel, setThemeMode, type ThemeMode } from "./theme";
import { cycleLangMode, getLangMode, onLangChange, langIcon, langLabel, setLangMode, type LangMode, t } from "./i18n";
import { Pane } from "./components/Pane";
import { Sidebar } from "./components/Sidebar";
import { PreviewPane } from "./components/PreviewPane";
import { Statusbar, FnBar, HelpBar } from "./components/Statusbar";
import { ConflictDialog } from "./components/ConflictDialog";
import { RenameDialog } from "./components/RenameDialog";
import { SearchDialog } from "./components/SearchDialog";
import { Dialogs } from "./components/Dialogs";
import { PropertiesDialog } from "./components/PropertiesDialog";
import { JobBar } from "./components/JobBar";
import { TimeMachineDialog, openTimeMachine } from "./components/TimeMachineDialog";
import { loadPane, state, setActive, setState, refreshPane, toggleCompareMode } from "./state";
import { homeDir, openTerminal, type JobProgress, type PaneChanged } from "./ipc";
import { attachKeymap } from "./keymap";
import { setHoverTarget, setDragEffect, defaultDragMode, toggleDefaultDragMode } from "./dnd";
import { transferEntries } from "./jobs";
import { loadPersisted, applyPersisted, attachPersist } from "./persist";
import { attachWindowState } from "./windowState";
import type { Entry, PaneId } from "./types";

type TauriDragPos = { x: number; y: number };
type TauriDragPayload = { paths: string[]; position: TauriDragPos };

function findDropTarget(cssX: number, cssY: number): { pane: PaneId; folderIdx: number | null } | null {
  const el = document.elementFromPoint(cssX, cssY);
  if (!el) return null;
  const paneEl = (el as HTMLElement).closest(".pane") as HTMLElement | null;
  if (!paneEl) return null;
  const allPanes = Array.from(document.querySelectorAll(".panes > .pane"));
  const paneIdx = allPanes.indexOf(paneEl);
  if (paneIdx < 0) return null;
  const pane: PaneId = paneIdx === 0 ? "left" : "right";
  const rowEl = (el as HTMLElement).closest(".row") as HTMLElement | null;
  let folderIdx: number | null = null;
  if (rowEl && rowEl.parentElement) {
    const rows = Array.from(rowEl.parentElement.querySelectorAll(".row"));
    const i = rows.indexOf(rowEl);
    const entry = state[pane].entries[i];
    if (entry && entry.isDir) folderIdx = i;
  }
  return { pane, folderIdx };
}

export function App() {
  const [themeMode, setThemeModeSig] = createSignal<ThemeMode>(getThemeMode());
  onThemeChange((m) => setThemeModeSig(m));
  const [langMode, setLangModeSig] = createSignal<LangMode>(getLangMode());
  onLangChange((m) => setLangModeSig(m));

  const panesTemplate = () => {
    const sw = Math.round(state.sidebarWidth);
    const pw = Math.round(state.previewWidth);
    const split = Math.max(0.1, Math.min(0.9, state.paneSplit));
    const leftFr = split.toFixed(4);
    const rightFr = (1 - split).toFixed(4);
    const parts: string[] = [];
    if (state.sidebarVisible) parts.push(`${sw}px`, "4px");
    parts.push(`${leftFr}fr`, "4px", `${rightFr}fr`);
    if (state.previewVisible) parts.push("4px", `${pw}px`);
    return parts.join(" ");
  };

  const startColResize = (ev: MouseEvent, kind: "sidebar" | "split" | "preview") => {
    ev.preventDefault();
    const startX = ev.clientX;
    const startSidebar = state.sidebarWidth;
    const startPreview = state.previewWidth;
    const startSplit = state.paneSplit;
    const panesEl = (ev.currentTarget as HTMLElement).parentElement as HTMLElement | null;
    document.body.classList.add("col-resizing");
    (ev.currentTarget as HTMLElement).classList.add("dragging");
    const target = ev.currentTarget as HTMLElement;

    const onMove = (mv: MouseEvent) => {
      const dx = mv.clientX - startX;
      if (kind === "sidebar") {
        const w = Math.max(120, Math.min(500, startSidebar + dx));
        setState("sidebarWidth", w);
      } else if (kind === "preview") {
        const w = Math.max(160, Math.min(700, startPreview - dx));
        setState("previewWidth", w);
      } else if (panesEl) {
        // Verfügbare Pane-Breite = panesEl.clientWidth minus Sidebar/Preview/Splitter
        const total = panesEl.clientWidth;
        const used = (state.sidebarVisible ? state.sidebarWidth + 4 : 0)
          + 4
          + (state.previewVisible ? state.previewWidth + 4 : 0);
        const avail = Math.max(200, total - used);
        const leftBase = startSplit * avail;
        const newLeft = Math.max(120, Math.min(avail - 120, leftBase + dx));
        setState("paneSplit", newLeft / avail);
      }
    };
    const onUp = () => {
      document.body.classList.remove("col-resizing");
      target.classList.remove("dragging");
      window.removeEventListener("mousemove", onMove, true);
      window.removeEventListener("mouseup", onUp, true);
    };
    window.addEventListener("mousemove", onMove, true);
    window.addEventListener("mouseup", onUp, true);
  };

  onMount(async () => {
    attachKeymap();
    void attachWindowState();
    await listen<JobProgress>("job-progress", (ev) => {
      const p = ev.payload;
      if (p.finished) {
        setState("job", null);
        return;
      }
      if (state.job && state.job.id === p.jobId) {
        setState("job", { ...state.job, done: p.done, total: p.total, current: p.current });
      }
    });

    const refreshTimers = new Map<string, number>();
    await listen<PaneChanged>("pane-changed", (ev) => {
      const { paneId, path } = ev.payload;
      const pane = paneId as "left" | "right";
      if (state[pane].cwd !== path) return; // stale
      const prev = refreshTimers.get(paneId);
      if (prev) clearTimeout(prev);
      const handle = window.setTimeout(() => {
        refreshTimers.delete(paneId);
        if (state[pane].cwd === path) refreshPane(pane);
      }, 150);
      refreshTimers.set(paneId, handle);
    });

    // Native OS-Datei-Drops (Finder etc.) via Tauri-Events.
    // Verhindert, dass das WebView bei einem nicht abgefangenen Drop
    // zur Datei-URL navigiert (weisser Screen).
    window.addEventListener("dragover", (ev) => ev.preventDefault());
    window.addEventListener("drop", (ev) => ev.preventDefault());
    const cssPos = (p: TauriDragPos) => {
      const dpr = window.devicePixelRatio || 1;
      return { x: p.x / dpr, y: p.y / dpr };
    };
    await listen<TauriDragPayload>("tauri://drag-enter", (ev) => {
      const { x, y } = cssPos(ev.payload.position);
      const t = findDropTarget(x, y);
      setDragEffect("copy");
      setHoverTarget(t ? { pane: t.pane, folderIdx: t.folderIdx } : null);
    });
    await listen<TauriDragPayload>("tauri://drag-over", (ev) => {
      const { x, y } = cssPos(ev.payload.position);
      const t = findDropTarget(x, y);
      setDragEffect("copy");
      setHoverTarget(t ? { pane: t.pane, folderIdx: t.folderIdx } : null);
    });
    await listen("tauri://drag-leave", () => {
      setHoverTarget(null);
      setDragEffect(null);
    });
    await listen<TauriDragPayload>("tauri://drag-drop", async (ev) => {
      const { x, y } = cssPos(ev.payload.position);
      const t = findDropTarget(x, y);
      setHoverTarget(null);
      setDragEffect(null);
      if (!t) return;
      let dstCwd = state[t.pane].cwd;
      if (t.folderIdx !== null) {
        const folder = state[t.pane].entries[t.folderIdx];
        if (folder && folder.isDir) dstCwd = folder.path;
      }
      if (!dstCwd) return;
      const entries: Entry[] = ev.payload.paths.map((p) => {
        const name = p.split("/").pop() || p;
        return {
          path: p,
          name,
          isDir: false,
          isSymlink: false,
          size: 0,
          mtime: 0,
          ext: "",
          hidden: false,
        } as Entry;
      });
      await transferEntries("copy", entries, dstCwd, [t.pane]);
    });

    const home = await homeDir();
    const persisted = loadPersisted();
    if (persisted) {
      applyPersisted(persisted);
      const lCwd = persisted.panes.left.tabs[persisted.panes.left.activeTab].cwd;
      const rCwd = persisted.panes.right.tabs[persisted.panes.right.activeTab].cwd;
      await loadPane("left", lCwd || home);
      await loadPane("right", rCwd || home);
      setActive(persisted.active);
    } else {
      await loadPane("left", home);
      await loadPane("right", home);
      setActive("left");
    }
    attachPersist();

    // Theme-Wechsel via macOS-Menü
    await listen<string>("dualbeam://theme", (ev) => {
      const m = ev.payload as ThemeMode;
      if (m === "auto" || m === "light" || m === "dark") setThemeMode(m);
    });
    // Sprach-Wechsel via macOS-Menü
    await listen<string>("dualbeam://lang", (ev) => {
      const m = ev.payload as LangMode;
      if (m === "auto" || m === "de" || m === "en") setLangMode(m);
    });
  });

  return (
    <div class="app">
      <div class="toolbar">
        <button
          class="tb-glyph"
          classList={{ active: state.sidebarVisible }}
          onClick={() => setState("sidebarVisible", (v) => !v)}
          title={t("toolbar.sidebar")}
        >
          <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
            <rect x="2" y="3" width="20" height="18" rx="3" fill="#2a6fb8" />
            <rect x="2" y="3" width="9" height="18" rx="3" fill="#5cd0f5" />
            <rect x="4" y="6" width="1.6" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="6.4" y="6" width="3.2" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="4" y="9" width="1.6" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="6.4" y="9" width="3.2" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="4" y="12" width="1.6" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="6.4" y="12" width="3.2" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="4" y="15" width="1.6" height="1.6" rx="0.3" fill="#0a5a82" />
            <rect x="6.4" y="15" width="3.2" height="1.6" rx="0.3" fill="#0a5a82" />
          </svg>
        </button>
        <button
          class="tb-glyph"
          classList={{ active: state.previewVisible }}
          onClick={() => setState("previewVisible", (v) => !v)}
          title={t("toolbar.preview")}
        >
          🔍
        </button>
        <button
          class="tb-glyph"
          onClick={() => refreshPane(state.active)}
          title={t("toolbar.refresh")}
        >
          🔄
        </button>
        <button
          class="tb-glyph"
          onClick={() => { const cwd = state[state.active]?.cwd; if (cwd) void openTerminal(cwd); }}
          title={t("toolbar.terminal")}
        >
          🖥️
        </button>
        <button
          class="tb-glyph"
          classList={{ active: state.extendedView }}
          onClick={() => setState("extendedView", (v) => !v)}
          title={t("toolbar.columns")}
        >
          📋
        </button>
        <button
          class="tb-glyph"
          classList={{ active: state.compareMode }}
          onClick={() => toggleCompareMode()}
          title={t("toolbar.compare")}
        >
          <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
            <rect x="2" y="4" width="9" height="16" rx="1.6" fill="#5cd0f5" stroke="#2a6fb8" stroke-width="1.2" />
            <rect x="13" y="4" width="9" height="16" rx="1.6" fill="#2a6fb8" stroke="#0a5a82" stroke-width="1.2" />
            <path d="M11 12 L13 12" stroke="#e87a2a" stroke-width="2" stroke-linecap="round" />
            <circle cx="6.5" cy="9" r="1" fill="#fff" />
            <circle cx="17.5" cy="9" r="1" fill="#fff" />
            <circle cx="6.5" cy="15" r="1" fill="#fff" />
            <circle cx="17.5" cy="15" r="1" fill="#fff" />
          </svg>
        </button>
        <button
          class="tb-glyph"
          classList={{ active: defaultDragMode() === "copy" }}
          onClick={() => toggleDefaultDragMode()}
          title={defaultDragMode() === "copy" ? t("toolbar.dragCopy") : t("toolbar.dragMove")}
        >
          {defaultDragMode() === "copy" ? (
            <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
              <rect x="2" y="2" width="15" height="18" rx="2" fill="#5cd0f5" stroke="#2a6fb8" stroke-width="1.2" />
              <rect x="7" y="6" width="15" height="18" rx="2" fill="#2a6fb8" stroke="#1a4a7a" stroke-width="1.2" />
              <line x1="10" y1="11" x2="19" y2="11" stroke="#fff" stroke-width="1.4" />
              <line x1="10" y1="15" x2="19" y2="15" stroke="#fff" stroke-width="1.4" />
              <line x1="10" y1="19" x2="16" y2="19" stroke="#fff" stroke-width="1.4" />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
              <rect x="1" y="5" width="13" height="14" rx="1.6" fill="#5cd0f5" stroke="#2a6fb8" stroke-width="1.2" />
              <path d="M14 12 L23 12 M18 7 L23 12 L18 17" fill="none" stroke="#e87a2a" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          )}
        </button>
        <div class="spacer" />
        <button
          class="tb-glyph"
          onClick={() => openTimeMachine()}
          title={t("toolbar.timeMachine")}
        >
          <span style="font-size:16px;line-height:1">🕒</span>
        </button>
        <button
          class="tb-glyph"
          onClick={() => cycleThemeMode()}
          title={t("toolbar.themeTitle", { label: themeLabel(themeMode()) })}
        >
          <span style="font-size:16px;line-height:1">{themeIcon(themeMode())}</span>
        </button>
        <button
          class="tb-glyph"
          onClick={() => cycleLangMode()}
          title={t("toolbar.langTitle", { label: langLabel(langMode()) })}
        >
          <span style="font-size:14px;line-height:1;font-weight:600">{langIcon(langMode())}</span>
        </button>
      </div>
      <div class={`panes ${state.sidebarVisible ? "" : "no-sidebar"} ${state.previewVisible ? "with-preview" : ""}`}
        style={{ "grid-template-columns": panesTemplate() }}>
        {state.sidebarVisible && <Sidebar />}
        {state.sidebarVisible && <div class="splitter" onMouseDown={(ev) => startColResize(ev, "sidebar")} />}
        <Pane id="left" />
        <div class="splitter" onMouseDown={(ev) => startColResize(ev, "split")} />
        <Pane id="right" />
        {state.previewVisible && <div class="splitter" onMouseDown={(ev) => startColResize(ev, "preview")} />}
        {state.previewVisible && <PreviewPane />}
      </div>
      <JobBar />
      <Statusbar />
      <FnBar />
      <HelpBar />
      <ConflictDialog />
      <RenameDialog />
      <SearchDialog />
      <Dialogs />
      <PropertiesDialog />
      <TimeMachineDialog />
      <div class="resize-grip" aria-hidden="true" />
    </div>
  );
}
