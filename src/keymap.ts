import {
  state,
  setActive,
  setCursor,
  selectOnly,
  loadPane,
  toggleHidden,
  forceRefreshPane,
  setFilter,
  toggleSidebar,
  togglePreview,
  toggleHelp,
  newTab,
  closeActiveTab,
  switchTab,
} from "./state";
import { openDefault, quickLook, clipboardWriteFiles } from "./ipc";
import { openProperties } from "./components/PropertiesDialog";
import {
  startTransfer,
  deleteSelected,
  makeFolder,
  beginRename,
  duplicateSelected,
  archiveAction,
  pasteFromClipboard,
} from "./jobs";
import { openRenameDialog } from "./rename";
import { openSearch } from "./components/SearchDialog";
import { connectToServer } from "./network";
import { undoAction, undoLastAction } from "./undo";
import type { PaneId } from "./types";

function parentDir(path: string): string {
  if (path === "/" || path === "") return "/";
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  const i = trimmed.lastIndexOf("/");
  return i <= 0 ? "/" : trimmed.slice(0, i);
}

async function activateEntry(pane: PaneId) {
  const p = state[pane];
  const e = p.entries[p.cursor];
  if (!e) return;
  const isApp = e.isDir && e.name.toLowerCase().endsWith(".app");
  if (isApp) {
    await openDefault(e.path);
  } else if (e.isDir) {
    await loadPane(pane, e.path);
  } else {
    await openDefault(e.path);
  }
}

async function goUp(pane: PaneId) {
  const p = state[pane];
  const parent = parentDir(p.cwd);
  if (parent !== p.cwd) await loadPane(pane, parent);
}

// Type-ahead: getippte Buchstaben springen zum nächsten passenden Eintrag.
const TYPE_AHEAD_RESET_MS = 800;
let typeAheadBuffer = "";
let typeAheadAt = 0;

function typeAhead(pane: PaneId, ch: string) {
  const now = Date.now();
  if (now - typeAheadAt > TYPE_AHEAD_RESET_MS) typeAheadBuffer = "";
  typeAheadAt = now;

  const p = state[pane];
  const n = p.entries.length;
  if (n === 0) return;

  // Wiederholt man denselben Buchstaben, zyklisch zum nächsten Treffer springen.
  const sameChar =
    typeAheadBuffer.length === 1 && typeAheadBuffer === ch.toLowerCase();
  typeAheadBuffer = sameChar
    ? typeAheadBuffer
    : typeAheadBuffer + ch.toLowerCase();
  const prefix = typeAheadBuffer;

  const start = sameChar ? p.cursor + 1 : p.cursor;
  for (let i = 0; i < n; i++) {
    const idx = (start + i) % n;
    if (p.entries[idx].name.toLowerCase().startsWith(prefix)) {
      selectOnly(pane, idx);
      return;
    }
  }
}

export function attachKeymap() {
  window.addEventListener("keydown", async (ev) => {
    // Eingaben in Inputs nicht abfangen
    const t = ev.target as HTMLElement | null;
    if (
      t &&
      (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)
    )
      return;

    const pane = state.active;
    const p = state[pane];

    // Die ersten neun Favoriten sind ohne Maus erreichbar. `code` statt
    // `key` funktioniert auch bei Tastaturlayouts, bei denen ⌥+Ziffern
    // Sonderzeichen erzeugen.
    if (
      ev.altKey &&
      !ev.metaKey &&
      !ev.ctrlKey &&
      /^Digit[1-9]$/.test(ev.code)
    ) {
      ev.preventDefault();
      window.dispatchEvent(
        new CustomEvent<number>("dualbeam:open-favorite", {
          detail: Number(ev.code.slice(-1)) - 1,
        }),
      );
      return;
    }

    switch (ev.key) {
      case "Tab":
        ev.preventDefault();
        setActive(pane === "left" ? "right" : "left");
        return;
      case "ArrowDown":
        ev.preventDefault();
        setCursor(pane, p.cursor + 1);
        selectOnly(pane, Math.min(p.entries.length - 1, p.cursor + 1));
        return;
      case "ArrowUp":
        ev.preventDefault();
        setCursor(pane, p.cursor - 1);
        selectOnly(pane, Math.max(0, p.cursor - 1));
        return;
      case "PageDown":
        ev.preventDefault();
        selectOnly(pane, Math.min(p.entries.length - 1, p.cursor + 20));
        return;
      case "PageUp":
        ev.preventDefault();
        selectOnly(pane, Math.max(0, p.cursor - 20));
        return;
      case "Home":
        ev.preventDefault();
        selectOnly(pane, 0);
        return;
      case "End":
        ev.preventDefault();
        selectOnly(pane, p.entries.length - 1);
        return;
      case "Enter":
        ev.preventDefault();
        await activateEntry(pane);
        return;
      case "F2":
        ev.preventDefault();
        beginRename();
        return;
      case "F1":
        ev.preventDefault();
        toggleHelp();
        return;
      case "F5":
        ev.preventDefault();
        await startTransfer("copy");
        return;
      case "F6":
        ev.preventDefault();
        await startTransfer("move");
        return;
      case "F7":
        ev.preventDefault();
        await makeFolder();
        return;
      case "F8":
      case "Delete":
      case "Backspace":
        if (ev.key === "Backspace" && !ev.metaKey) {
          // Plain Backspace = nach oben (alte Verhaltensweise)
          ev.preventDefault();
          await goUp(pane);
          return;
        }
        // Delete / F8 / ⌘Backspace = löschen
        // Shift = ohne Nachfrage
        ev.preventDefault();
        await deleteSelected(ev.shiftKey);
        return;
      case " ": {
        ev.preventDefault();
        const e = p.entries[p.cursor];
        if (e) await quickLook(e.path).catch(() => {});
        return;
      }
      case "F3": {
        ev.preventDefault();
        const e = p.entries[p.cursor];
        if (e) await quickLook(e.path).catch(() => {});
        return;
      }
      case "Escape":
        if (p.filter) {
          ev.preventDefault();
          setFilter(pane, "");
        }
        return;
    }

    // Cmd-Kombinationen
    if (ev.metaKey) {
      switch (ev.key) {
        case "z":
        case "Z":
          if (undoAction()) {
            ev.preventDefault();
            await undoLastAction();
          }
          return;
        case ".":
          ev.preventDefault();
          toggleHidden();
          return;
        case "r":
          ev.preventDefault();
          if (ev.shiftKey) {
            await forceRefreshPane(pane);
          } else {
            openRenameDialog();
          }
          return;
        case "R":
          // Manche Layouts liefern "R" bei Cmd+Shift+R.
          ev.preventDefault();
          await forceRefreshPane(pane);
          return;
        case "d":
          ev.preventDefault();
          await duplicateSelected();
          return;
        case "c":
        case "C": {
          ev.preventDefault();
          const sel = p.entries.filter((e) => p.selected.has(e.path));
          const items =
            sel.length > 0
              ? sel
              : p.entries[p.cursor]
                ? [p.entries[p.cursor]]
                : [];
          if (items.length === 0) return;
          try {
            await clipboardWriteFiles(items.map((e) => e.path));
          } catch (e) {
            console.error("clipboardWriteFiles failed", e);
          }
          return;
        }
        case "v":
        case "V": {
          ev.preventDefault();
          await pasteFromClipboard(pane);
          return;
        }
        case "b":
          ev.preventDefault();
          toggleSidebar();
          return;
        case "k":
        case "K":
          ev.preventDefault();
          await connectToServer();
          return;
        case "i":
        case "I":
          ev.preventDefault();
          if (ev.altKey) {
            const sel = p.entries[p.cursor];
            if (sel) void openProperties(sel.path);
          } else {
            togglePreview();
          }
          return;
        case "f":
          ev.preventDefault();
          openSearch();
          return;
        case "F":
          ev.preventDefault();
          openSearch();
          return;
        case "t":
          ev.preventDefault();
          newTab(pane);
          return;
        case "w":
          ev.preventDefault();
          closeActiveTab(pane);
          return;
        case "e":
        case "E":
          ev.preventDefault();
          await archiveAction();
          return;
        case "ArrowUp":
          ev.preventDefault();
          await goUp(pane);
          return;
      }
      // ⌘1..9 = Tab wechseln
      if (/^[1-9]$/.test(ev.key)) {
        ev.preventDefault();
        switchTab(pane, parseInt(ev.key, 10) - 1);
        return;
      }
    }

    // Type-ahead: einzelnes druckbares Zeichen ohne Modifier -> zum Eintrag springen.
    if (
      !ev.metaKey &&
      !ev.ctrlKey &&
      !ev.altKey &&
      ev.key.length === 1 &&
      ev.key !== " "
    ) {
      typeAhead(pane, ev.key);
      return;
    }
  });
}
