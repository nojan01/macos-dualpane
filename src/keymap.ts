import { state, setActive, setCursor, selectOnly, loadPane, toggleHidden, refreshPane, setFilter, requestFocusFilter, toggleSidebar, togglePreview, toggleHelp, newTab, closeActiveTab, switchTab } from "./state";
import { openDefault, quickLook } from "./ipc";
import { openProperties } from "./components/PropertiesDialog";
import { startTransfer, deleteSelected, makeFolder, beginRename, duplicateSelected, archiveAction } from "./jobs";
import { openRenameDialog } from "./rename";
import { openSearch } from "./components/SearchDialog";
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

export function attachKeymap() {
  window.addEventListener("keydown", async (ev) => {
    // Eingaben in Inputs nicht abfangen
    const t = ev.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;

    const pane = state.active;
    const p = state[pane];

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
        case ".":
          ev.preventDefault();
          toggleHidden();
          return;
        case "r":
          ev.preventDefault();
          if (ev.shiftKey) {
            await refreshPane(pane);
          } else {
            openRenameDialog();
          }
          return;
        case "R":
          // Manche Layouts liefern "R" bei Cmd+Shift+R.
          ev.preventDefault();
          await refreshPane(pane);
          return;
        case "d":
          ev.preventDefault();
          await duplicateSelected();
          return;
        case "b":
          ev.preventDefault();
          toggleSidebar();
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
          if (ev.shiftKey) {
            openSearch();
          } else {
            requestFocusFilter(pane);
          }
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
  });
}
