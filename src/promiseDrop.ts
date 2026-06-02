import { listen } from "@tauri-apps/api/event";
import { pathExists, resolvePromiseDrop } from "./ipc";
import { askConflict } from "./jobs";

type PromiseDropPayload = { id: number; src: string; dest: string };

let _registered = false;

export async function attachPromiseDropHandler(): Promise<void> {
  if (_registered) return;
  _registered = true;
  await listen<PromiseDropPayload>("dualbeam://promise-drop", async (ev) => {
    const { id, dest } = ev.payload;
    try {
      const exists = await pathExists(dest);
      if (!exists) {
        await resolvePromiseDrop(id, "overwrite");
        return;
      }
      const name = dest.split("/").pop() || dest;
      const choice = await askConflict(1, [name]);
      if (choice === "cancel") {
        await resolvePromiseDrop(id, "cancel");
      } else if (choice === "overwrite") {
        await resolvePromiseDrop(id, "overwrite");
      } else {
        // "skip" oder "rename" → wir lassen native Seite einen freien Namen wählen.
        await resolvePromiseDrop(id, "keep_both");
      }
    } catch (e) {
      console.error("promise-drop handler failed", e);
      try {
        await resolvePromiseDrop(id, "cancel");
      } catch { /* ignore */ }
    }
  });
}
