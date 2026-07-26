import { createSignal } from "solid-js";
import {
  cleanupExpiredUndo,
  finalizeStagedDelete,
  undoStagedDelete,
  type UndoDeleteItem,
} from "./ipc";
import { notifyError } from "./components/Dialogs";
import { errMsg } from "./i18n";
import { refreshPane } from "./state";

const UNDO_TTL_MS = 10 * 60 * 1000;

type UndoAction = { label: string; items: UndoDeleteItem[]; timer: number; expiresAt: number };
const [undoAction, setUndoAction] = createSignal<UndoAction | null>(null);
export { undoAction };

export async function cleanupUndoBuffer() {
  await cleanupExpiredUndo();
}

export function rememberStagedDelete(items: UndoDeleteItem[]) {
  const previous = undoAction();
  if (previous) {
    clearTimeout(previous.timer);
    void finalizeStagedDelete(previous.items).catch(() => {});
  }
  setUndoAction(armUndoAction(items, Date.now() + UNDO_TTL_MS));
}

/** Baut einen Undo-Eintrag samt Ablauf-Timer für den verbleibenden Zeitraum. */
function armUndoAction(items: UndoDeleteItem[], expiresAt: number): UndoAction {
  const action: UndoAction = {
    label: "Löschen",
    items,
    expiresAt,
    timer: window.setTimeout(
      () => {
        if (undoAction() !== action) return;
        setUndoAction(null);
        void finalizeStagedDelete(items).catch(() => {});
      },
      Math.max(0, expiresAt - Date.now()),
    ),
  };
  return action;
}

export async function undoLastAction() {
  const action = undoAction();
  if (!action) return;
  clearTimeout(action.timer);
  try {
    await undoStagedDelete(action.items);
  } catch (err) {
    // Wiederherstellen fehlgeschlagen: Eintrag mit Restlaufzeit erhalten,
    // damit der Nutzer es erneut versuchen kann.
    setUndoAction(armUndoAction(action.items, action.expiresAt));
    await Promise.all([refreshPane("left"), refreshPane("right")]);
    await notifyError(errMsg(err));
    return;
  }
  setUndoAction(null);
  await Promise.all([refreshPane("left"), refreshPane("right")]);
}
