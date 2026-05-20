import { For, Show, createEffect, createSignal, onMount, onCleanup } from "solid-js";
import { state, loadPane, volumesTick, handleVolumeGone } from "../state";
import {
  homeDir,
  listVolumes,
  ejectVolume,
  loadFavorites,
  saveFavorites,
  type Volume,
  type Favorite,
} from "../ipc";
import { askConfirm } from "./Dialogs";
import { t } from "../i18n";

function basename(p: string): string {
  const trimmed = p.endsWith("/") ? p.slice(0, -1) : p;
  const i = trimmed.lastIndexOf("/");
  return i < 0 ? trimmed : trimmed.slice(i + 1) || "/";
}

type Menu = { idx: number; x: number; y: number } | null;
type VolMenu = { vol: Volume; x: number; y: number } | null;

export function Sidebar() {
  const [favs, setFavs] = createSignal<Favorite[]>([]);
  const [vols, setVols] = createSignal<Volume[]>([]);
  const [editIdx, setEditIdx] = createSignal<number | null>(null);
  const [editValue, setEditValue] = createSignal("");
  const [menu, setMenu] = createSignal<Menu>(null);
  const [volMenu, setVolMenu] = createSignal<VolMenu>(null);
  const [dragIdx, setDragIdx] = createSignal<number | null>(null);
  const [overIdx, setOverIdx] = createSignal<number | null>(null);

  async function refreshVols() {
    try {
      setVols(await listVolumes());
    } catch {
      setVols([]);
    }
  }

  async function persist(next: Favorite[]) {
    setFavs(next);
    try {
      await saveFavorites(next);
    } catch (err: any) {
      console.error("saveFavorites:", err);
    }
  }

  async function addCurrent() {
    const cwd = state[state.active].cwd;
    const name = basename(cwd) || cwd;
    if (favs().some((f) => f.path === cwd)) return;
    await persist([...favs(), { name, icon: "📁", path: cwd }]);
  }

  async function removeAt(idx: number) {
    const next = favs().slice();
    next.splice(idx, 1);
    await persist(next);
  }

  function startRename(idx: number) {
    setEditIdx(idx);
    setEditValue(favs()[idx].name);
  }

  async function commitRename() {
    const idx = editIdx();
    if (idx == null) return;
    const v = editValue().trim();
    setEditIdx(null);
    if (!v || v === favs()[idx].name) return;
    const next = favs().slice();
    next[idx] = { ...next[idx], name: v };
    await persist(next);
  }

  function cancelRename() {
    setEditIdx(null);
  }

  function onDragStart(i: number, ev: DragEvent) {
    setDragIdx(i);
    if (ev.dataTransfer) {
      ev.dataTransfer.setData("application/x-fav-idx", String(i));
      ev.dataTransfer.effectAllowed = "move";
    }
  }

  function onDragOver(i: number, ev: DragEvent) {
    if (dragIdx() == null) return;
    ev.preventDefault();
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = "move";
    if (overIdx() !== i) setOverIdx(i);
  }

  function onDragLeave(i: number) {
    if (overIdx() === i) setOverIdx(null);
  }

  async function onDrop(target: number, ev: DragEvent) {
    ev.preventDefault();
    const from = dragIdx();
    setDragIdx(null);
    setOverIdx(null);
    if (from == null || from === target) return;
    const next = favs().slice();
    const [m] = next.splice(from, 1);
    next.splice(target, 0, m);
    await persist(next);
  }

  function onDragEnd() {
    setDragIdx(null);
    setOverIdx(null);
  }

  function openMenu(idx: number, ev: MouseEvent) {
    ev.preventDefault();
    ev.stopPropagation();
    setMenu({ idx, x: ev.clientX, y: ev.clientY });
  }

  function closeMenu() {
    setMenu(null);
    setVolMenu(null);
  }

  function openVolMenu(vol: Volume, ev: MouseEvent) {
    ev.preventDefault();
    ev.stopPropagation();
    setMenu(null);
    setVolMenu({ vol, x: ev.clientX, y: ev.clientY });
  }

  async function doEject(vol: Volume) {
    const ok = await askConfirm({
      title: t("sidebar.ejectTitle"),
      message: t("sidebar.ejectConfirm", { name: vol.name }),
      okLabel: t("sidebar.eject"),
      danger: true,
    });
    if (!ok) return;
    try {
      await ejectVolume(vol.path);
      await handleVolumeGone(vol.path);
      await refreshVols();
    } catch (err: any) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: String(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    }
  }

  const onGlobalClick = () => closeMenu();
  const onGlobalKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") closeMenu();
  };

  onMount(async () => {
    try {
      setFavs(await loadFavorites());
    } catch {
      const home = await homeDir();
      setFavs([{ name: "Home", icon: "🏠", path: home }]);
    }
    await refreshVols();
    const t = setInterval(refreshVols, 5000);
    createEffect(() => {
      volumesTick();
      void refreshVols();
    });
    window.addEventListener("click", onGlobalClick);
    window.addEventListener("keydown", onGlobalKey);
    onCleanup(() => {
      clearInterval(t);
      window.removeEventListener("click", onGlobalClick);
      window.removeEventListener("keydown", onGlobalKey);
    });
  });

  const go = (path: string) => loadPane(state.active, path);

  return (
    <Show when={state.sidebarVisible}>
      <aside class="sidebar">
        <div class="sb-section">
          <span>{t("sidebar.favorites")}</span>
          <button class="sb-add" title={t("sidebar.addCurrent")} onClick={addCurrent}>＋</button>
        </div>
        <For each={favs()}>
          {(f, i) => (
            <div
              class={`sb-item ${state[state.active].cwd === f.path ? "active" : ""} ${overIdx() === i() ? "drop-target" : ""}`}
              onClick={() => {
                if (editIdx() === i()) return;
                go(f.path);
              }}
              title={f.path}
              draggable={editIdx() !== i()}
              onDragStart={(ev) => onDragStart(i(), ev)}
              onDragEnter={(ev) => onDragOver(i(), ev)}
              onDragOver={(ev) => onDragOver(i(), ev)}
              onDragLeave={() => onDragLeave(i())}
              onDrop={(ev) => onDrop(i(), ev)}
              onDragEnd={onDragEnd}
              onContextMenu={(ev) => openMenu(i(), ev)}
              onDblClick={(ev) => {
                ev.stopPropagation();
                startRename(i());
              }}
            >
              <span class="sb-icon">{f.icon}</span>
              <Show
                when={editIdx() === i()}
                fallback={<span class="sb-label">{f.name}</span>}
              >
                <input
                  class="sb-edit"
                  title={t("sidebar.renameFav")}
                  value={editValue()}
                  autofocus
                  onInput={(e) => setEditValue(e.currentTarget.value)}
                  onClick={(e) => e.stopPropagation()}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") commitRename();
                    else if (e.key === "Escape") cancelRename();
                  }}
                  onBlur={commitRename}
                />
              </Show>
            </div>
          )}
        </For>
        <div class="sb-section">{t("sidebar.volumes")}</div>
        <Show when={vols().length > 0} fallback={<div class="sb-empty">{t("sidebar.none")}</div>}>
          <For each={vols()}>
            {(v) => (
              <div
                class={`sb-item ${state[state.active].cwd === v.path ? "active" : ""}`}
                onClick={() => go(v.path)}
                onContextMenu={(ev) => openVolMenu(v, ev)}
                title={v.path}
              >
                <span class="sb-icon">💽</span>
                <span class="sb-label">{v.name}</span>
              </div>
            )}
          </For>
        </Show>
        <Show when={menu()}>
          {(m) => (
            <div
              class="ctx-menu"
              style={{ left: `${m().x}px`, top: `${m().y}px` }}
              onClick={(e) => e.stopPropagation()}
            >
              <div
                class="ctx-item"
                onClick={() => {
                  const idx = m().idx;
                  closeMenu();
                  startRename(idx);
                }}
              >
                {t("sidebar.rename")}
              </div>
              <div
                class="ctx-item danger"
                onClick={() => {
                  const idx = m().idx;
                  closeMenu();
                  removeAt(idx);
                }}
              >
                {t("sidebar.remove")}
              </div>
            </div>
          )}
        </Show>
        <Show when={volMenu()}>
          {(m) => (
            <div
              class="ctx-menu"
              style={{ left: `${m().x}px`, top: `${m().y}px` }}
              onClick={(e) => e.stopPropagation()}
            >
              <div
                class="ctx-item"
                onClick={() => {
                  const v = m().vol;
                  closeMenu();
                  void doEject(v);
                }}
              >
                {t("sidebar.eject")}
              </div>
            </div>
          )}
        </Show>
      </aside>
    </Show>
  );
}


