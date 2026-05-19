import { For } from "solid-js";
import { state, setActive, loadPane } from "../state";
import type { PaneId } from "../types";

function segments(path: string): { label: string; path: string }[] {
  if (!path) return [];
  const parts = path.split("/").filter(Boolean);
  const segs: { label: string; path: string }[] = [{ label: "/", path: "/" }];
  let acc = "";
  for (const p of parts) {
    acc += "/" + p;
    segs.push({ label: p, path: acc });
  }
  return segs;
}

export function PathBar(props: { id: PaneId }) {
  const id = props.id;
  const segs = () => segments(state[id].cwd);
  return (
    <div class="path-bar" onMouseDown={() => setActive(id)}>
      <For each={segs()}>
        {(s, i) => (
          <>
            {i() > 0 && <span class="sep">/</span>}
            <span class="seg" onClick={() => loadPane(id, s.path)}>{s.label}</span>
          </>
        )}
      </For>
    </div>
  );
}
