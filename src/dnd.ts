// Drag & Drop Zustand zwischen Panes.
import { createSignal } from "solid-js";
import type { Entry, PaneId } from "./types";
import { state } from "./state";
import { transferEntries } from "./jobs";

export type DragPayload = {
  srcPane: PaneId;
  items: Entry[];
};

let payload: DragPayload | null = null;

// Live-Effekt: aktualisiert sich aus dragover-Modifier.
export const [dragEffect, setDragEffect] = createSignal<"copy" | "move" | null>(null);

// Standard-Maus-DnD-Effekt (durch Alt invertiert). Persistent in localStorage.
const DRAG_MODE_KEY = "dualbeam:dragmode:v1";
function loadDragMode(): "copy" | "move" {
  try {
    const v = localStorage.getItem(DRAG_MODE_KEY);
    if (v === "copy" || v === "move") return v;
  } catch {}
  return "move";
}
export const [defaultDragMode, setDefaultDragMode] = createSignal<"copy" | "move">(loadDragMode());
export function toggleDefaultDragMode() {
  const next = defaultDragMode() === "move" ? "copy" : "move";
  setDefaultDragMode(next);
  try { localStorage.setItem(DRAG_MODE_KEY, next); } catch {}
}
function effectiveEffect(altKey: boolean): "copy" | "move" {
  const def = defaultDragMode();
  if (!altKey) return def;
  return def === "move" ? "copy" : "move";
}

// Hover-Ziel: Pane + optional Index eines Ordner-Eintrags (für Drop-in-Folder).
export type HoverTarget = { pane: PaneId; folderIdx: number | null } | null;
export const [hoverTarget, setHoverTarget] = createSignal<HoverTarget>(null);

export function beginDrag(p: DragPayload) {
  payload = p;
  setDragEffect(defaultDragMode());
}

export function getDrag(): DragPayload | null {
  return payload;
}

export function endDrag() {
  payload = null;
  setDragEffect(null);
  setHoverTarget(null);
}

// ---------- Pointer-basiertes internes DnD ----------
// (HTML5-DnD wird durch Tauris natives File-Drop-Handling geschluckt,
//  daher steuern wir Pane-zu-Pane-Drags manuell mit Mouse-Events.)

const DRAG_THRESHOLD = 4;

function findInternalTarget(x: number, y: number, drag: DragPayload): HoverTarget {
  const el = document.elementFromPoint(x, y);
  if (!el) return null;
  const paneEl = (el as HTMLElement).closest(".pane") as HTMLElement | null;
  if (!paneEl) return null;
  const allPanes = Array.from(document.querySelectorAll(".panes > .pane"));
  const paneIdx = allPanes.indexOf(paneEl);
  if (paneIdx < 0) return null;
  const pane: PaneId = paneIdx === 0 ? "left" : "right";
  const rowEl = (el as HTMLElement).closest(".row") as HTMLElement | null;
  let folderIdx: number | null = null;
  if (rowEl) {
    // data-index trägt den echten Eintrags-Index – bei aktiver Virtualisierung
    // entspricht die DOM-Position nicht dem Index in state.entries.
    const i = Number(rowEl.dataset.index);
    const entry = Number.isInteger(i) ? state[pane].entries[i] : undefined;
    if (entry && entry.isDir && !drag.items.some((it) => it.path === entry.path)) {
      folderIdx = i;
    }
  }
  return { pane, folderIdx };
}

export function startPointerDrag(initial: DragPayload, origin: { x: number; y: number }) {
  let started = false;

  const cleanup = () => {
    window.removeEventListener("mousemove", onMove, true);
    window.removeEventListener("mouseup", onUp, true);
    window.removeEventListener("keydown", onKey, true);
    document.body.classList.remove("dragging");
  };

  const onMove = (ev: MouseEvent) => {
    if (!started) {
      const dx = ev.clientX - origin.x;
      const dy = ev.clientY - origin.y;
      if (dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD) return;
      started = true;
      beginDrag(initial);
      document.body.classList.add("dragging");
    }
    setDragEffect(effectiveEffect(ev.altKey));
    setHoverTarget(findInternalTarget(ev.clientX, ev.clientY, initial));
  };

  const onUp = async (ev: MouseEvent) => {
    cleanup();
    if (!started) {
      endDrag();
      return;
    }
    const eff: "copy" | "move" = effectiveEffect(ev.altKey);
    const t = findInternalTarget(ev.clientX, ev.clientY, initial);
    const items = initial.items;
    endDrag();
    if (!t) return;
    let dstCwd = state[t.pane].cwd;
    if (t.folderIdx !== null) {
      const folder = state[t.pane].entries[t.folderIdx];
      if (folder && folder.isDir) dstCwd = folder.path;
    }
    if (!dstCwd) return;
    await transferEntries(eff, items, dstCwd, ["left", "right"]);
  };

  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") {
      cleanup();
      endDrag();
    }
  };

  window.addEventListener("mousemove", onMove, true);
  window.addEventListener("mouseup", onUp, true);
  window.addEventListener("keydown", onKey, true);
}

