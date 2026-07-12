import { createSignal } from "solid-js";
import {
  cleanupExpiredUndo,
  finalizeStagedDelete,
  undoStagedDelete,
  type UndoDeleteItem,
} from "./ipc";
import { refreshPane } from "./state";

type UndoAction = { label: string; items: UndoDeleteItem[]; timer: number };
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
  const action: UndoAction = {
    label: "Löschen",
    items,
    timer: window.setTimeout(
      () => {
        if (undoAction() !== action) return;
        setUndoAction(null);
        void finalizeStagedDelete(items).catch(() => {});
      },
      10 * 60 * 1000,
    ),
  };
  setUndoAction(action);
}

export async function undoLastAction() {
  const action = undoAction();
  if (!action) return;
  clearTimeout(action.timer);
  setUndoAction(null);
  await undoStagedDelete(action.items);
  await Promise.all([refreshPane("left"), refreshPane("right")]);
}
