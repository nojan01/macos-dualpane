import { For } from "solid-js";
import { state, newTab, closeTab, switchTab, setActive } from "../state";
import type { PaneId } from "../types";
import { t as tr } from "../i18n";

function basename(p: string): string {
  if (!p || p === "/") return "/";
  const trimmed = p.endsWith("/") ? p.slice(0, -1) : p;
  const i = trimmed.lastIndexOf("/");
  return i < 0 ? trimmed : trimmed.slice(i + 1) || "/";
}

export function TabBar(props: { id: PaneId }) {
  const id = props.id;
  return (
    <div class="tab-bar" onMouseDown={() => setActive(id)}>
      <For each={state.tabs[id]}>
        {(t, idx) => {
          const active = () => state.activeTab[id] === idx();
          return (
            <div
              class={`tab ${active() ? "active" : ""}`}
              title={t.cwd}
              onClick={() => {
                setActive(id);
                switchTab(id, idx());
              }}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  closeTab(id, idx());
                }
              }}
            >
              <span class="tab-label">{basename(t.cwd) || "—"}</span>
              {state.tabs[id].length > 1 && (
                <button
                  class="tab-close"
                  title={tr("tab.close")}
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(id, idx());
                  }}
                >
                  ×
                </button>
              )}
            </div>
          );
        }}
      </For>
      <button class="tab-new" title={tr("tab.new")} onClick={() => newTab(id)}>+</button>
    </div>
  );
}
