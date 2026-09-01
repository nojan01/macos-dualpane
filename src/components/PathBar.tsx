import { For } from "solid-js";
import { state, setActive, loadPane, goBackInHistory, canNavigateUp } from "../state";
import { t } from "../i18n";
import type { PaneId } from "../types";

function segments(path: string, navigationRoot?: string): { label: string; path: string }[] {
  if (!path) return [];
  const normalizedPath = path.length > 1 ? path.replace(/\/+$/, "") : path;
  const normalizedRoot = navigationRoot && (navigationRoot.length > 1
    ? navigationRoot.replace(/\/+$/, "")
    : navigationRoot);
  if (
    normalizedRoot &&
    (normalizedPath === normalizedRoot || normalizedPath.startsWith(`${normalizedRoot}/`))
  ) {
    const rootLabel = normalizedRoot.split("/").filter(Boolean).pop() || "/";
    const segs: { label: string; path: string }[] = [{ label: rootLabel, path: normalizedRoot }];
    const relative = normalizedPath.slice(normalizedRoot.length).split("/").filter(Boolean);
    let acc = normalizedRoot;
    for (const part of relative) {
      acc += `/${part}`;
      segs.push({ label: part, path: acc });
    }
    return segs;
  }
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
  const segs = () => segments(state[id].cwd, state[id].navigationRoot);
  const goUp = () => {
    if (!canNavigateUp(id)) return;
    const cwd = state[id].cwd;
    if (!cwd || cwd === "/") return;
    const idx = cwd.lastIndexOf("/");
    const parent = idx <= 0 ? "/" : cwd.slice(0, idx);
    loadPane(id, parent);
  };
  return (
    <div class="path-bar" onMouseDown={() => setActive(id)}>
      <button
        class="path-up"
        title={t("path.up")}
        onClick={(e) => { e.stopPropagation(); goUp(); }}
        disabled={!canNavigateUp(id)}
      >↑</button>
      <button
        class="path-back"
        title={t("path.back")}
        onClick={(e) => { e.stopPropagation(); void goBackInHistory(id); }}
        disabled={state[id].historyIndex <= 0}
      >←</button>
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
