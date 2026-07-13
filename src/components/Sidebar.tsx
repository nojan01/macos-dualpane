import {
  For,
  Show,
  createEffect,
  createSignal,
  onMount,
  onCleanup,
} from "solid-js";
import { state, loadPane, volumesTick, handleVolumeGone } from "../state";
import {
  homeDir,
  listVolumes,
  ejectVolume,
  loadFavorites,
  saveFavorites,
  listNetworkBookmarks,
  removeNetworkBookmark,
  rememberNetworkVolume,
  mountNetworkUrl,
  type Volume,
  type Favorite,
  type NetworkBookmark,
} from "../ipc";
import { askConfirm, notify, notifyError } from "./Dialogs";
import { connectToServer } from "../network";
import { t, errMsg } from "../i18n";
import { runSyncProfile } from "../sync";
import { syncProfiles } from "../syncProfiles";

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
  const [bookmarks, setBookmarks] = createSignal<NetworkBookmark[]>([]);
  const [mounting, setMounting] = createSignal<string | null>(null);
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
    try {
      setBookmarks(await listNetworkBookmarks());
    } catch {
      setBookmarks([]);
    }
  }

  async function mountBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    setMounting(b.url);
    try {
      await mountNetworkUrl(b.url);
      await refreshVols();
      const fresh = bookmarks().find((x) => x.url === b.url);
      if (fresh?.connected) go(fresh.mountPath);
    } catch (err) {
      await askConfirm({
        title: t("sidebar.mountFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function ejectBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    const confirmed = await askConfirm({
      title: t("sidebar.unmount"),
      message: t("sidebar.unmountConfirm", { name: b.name }),
      okLabel: t("sidebar.unmount"),
      danger: true,
    });
    if (!confirmed) return;
    setMounting(b.url);
    try {
      await ejectVolume(b.mountPath);
      await handleVolumeGone(b.mountPath);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function removeBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    const confirmed = await askConfirm({
      title: t("sidebar.removeNetworkTitle"),
      message: t("sidebar.removeNetworkConfirm", { name: b.name }),
      okLabel: t("sidebar.removeNetwork"),
      danger: true,
    });
    if (!confirmed) return;
    setMounting(b.url);
    try {
      // Stufe 2 umfasst auch Stufe 1: Ein verbundenes Volume wird zuerst
      // sauber ausgehängt, erst dann verschwindet das Lesezeichen.
      if (b.connected) {
        await ejectVolume(b.mountPath);
        await handleVolumeGone(b.mountPath);
      }
      await removeNetworkBookmark(b.url);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function reconnectBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    setMounting(b.url);
    try {
      // Erst aushängen (Fehler ignorieren, falls schon weg), dann neu mounten.
      try {
        await ejectVolume(b.mountPath);
        await handleVolumeGone(b.mountPath);
      } catch {
        /* nicht gemountet oder bereits ausgehängt – weiter mit mount */
      }
      await mountNetworkUrl(b.url);
      await refreshVols();
      const fresh = bookmarks().find((x) => x.url === b.url);
      if (fresh?.connected) go(fresh.mountPath);
    } catch (err) {
      await askConfirm({
        title: t("sidebar.mountFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function persist(next: Favorite[]) {
    setFavs(next);
    try {
      await saveFavorites(next);
    } catch (err) {
      console.error("saveFavorites:", err);
      await notifyError(t("sidebar.addFailed"));
    }
  }

  async function addCurrent() {
    const cwd = state[state.active].cwd;
    if (!cwd) return;
    const name = basename(cwd) || cwd;
    if (favs().some((f) => f.path === cwd)) {
      await notify({
        title: t("sidebar.favorites"),
        message: t("sidebar.alreadyFav", { name }),
      });
      return;
    }
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
    const isNetwork = vol.kind === "network";
    const ok = await askConfirm({
      title: isNetwork ? t("sidebar.unmount") : t("sidebar.ejectTitle"),
      message: isNetwork
        ? t("sidebar.unmountConfirm", { name: vol.name })
        : t("sidebar.ejectConfirm", { name: vol.name }),
      okLabel: isNetwork ? t("sidebar.unmount") : t("sidebar.eject"),
      danger: true,
    });
    if (!ok) return;
    try {
      // Ein bislang nur flüchtig angezeigtes Netzlaufwerk wird vor dem
      // Aushängen als Lesezeichen gespeichert. Dadurch bleibt es danach in
      // der Sidebar sichtbar und lässt sich wieder verbinden.
      if (isNetwork) await rememberNetworkVolume(vol.path);
      await ejectVolume(vol.path);
      await handleVolumeGone(vol.path);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title:
          isNetwork
            ? t("sidebar.saveNetworkFailed")
            : t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    }
  }

  const onGlobalClick = () => closeMenu();
  const onGlobalKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") closeMenu();
  };
  const onOpenFavorite = (ev: Event) => {
    const idx = (ev as CustomEvent<number>).detail;
    const favorite = favs()[idx];
    if (favorite) go(favorite.path);
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
    window.addEventListener("click", onGlobalClick);
    window.addEventListener("keydown", onGlobalKey);
    window.addEventListener("dualbeam:open-favorite", onOpenFavorite);
    onCleanup(() => {
      clearInterval(t);
      window.removeEventListener("click", onGlobalClick);
      window.removeEventListener("keydown", onGlobalKey);
      window.removeEventListener("dualbeam:open-favorite", onOpenFavorite);
    });
  });

  createEffect(() => {
    volumesTick();
    void refreshVols();
  });

  const go = (path: string) => loadPane(state.active, path);

  return (
    <Show when={state.sidebarVisible}>
      <aside class="sidebar">
        <div class="sb-section">
          <span>{t("sidebar.favorites")}</span>
          <button
            class="sb-add"
            title={t("sidebar.addCurrent")}
            onClick={addCurrent}
          >
            ＋
          </button>
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
        <Show
          when={vols().filter((v) => v.kind !== "network").length > 0}
          fallback={<div class="sb-empty">{t("sidebar.none")}</div>}
        >
          <For each={vols().filter((v) => v.kind !== "network")}>
            {(v) => (
              <div
                class={`sb-item ${state[state.active].cwd === v.path ? "active" : ""}`}
                onClick={() => go(v.path)}
                onContextMenu={(ev) => openVolMenu(v, ev)}
                title={v.path}
              >
                <span class="sb-icon">💽</span>
                <span class="sb-label">{v.name}</span>
                <button
                  class="sb-eject"
                  title={t("sidebar.eject")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void doEject(v);
                  }}
                >
                  ⏏
                </button>
              </div>
            )}
          </For>
        </Show>
        <div class="sb-section sb-section-spaced">
          <span>{t("sidebar.network")}</span>
          <button
            class="sb-add"
            title={t("network.connectServer")}
            onClick={() => void connectToServer()}
          >
            ＋
          </button>
        </div>
        <For each={bookmarks()}>
          {(b) => (
            <div
              class={`sb-item ${state[state.active].cwd === b.mountPath ? "active" : ""} ${b.connected ? "" : "disconnected"}`}
              onClick={() => (b.connected ? go(b.mountPath) : mountBookmark(b))}
              title={
                b.connected
                  ? b.mountPath
                  : `${b.url} — ${t("sidebar.clickToMount")}`
              }
            >
              <span class="sb-icon">{b.connected ? "🌐" : "🔌"}</span>
              <span class="sb-label">{b.name}</span>
              <Show when={mounting() === b.url}>
                <span class="sb-spin">…</span>
              </Show>
              <Show when={b.connected && mounting() !== b.url}>
                <button
                  class="sb-eject"
                  title={t("sidebar.reconnect")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void reconnectBookmark(b);
                  }}
                >
                  ↻
                </button>
                <button
                  class="sb-eject"
                  title={t("sidebar.unmount")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void ejectBookmark(b);
                  }}
                >
                  ⏏
                </button>
              </Show>
              <Show when={mounting() !== b.url}>
                <button
                  class="sb-eject sb-remove"
                  title={t("sidebar.removeNetwork")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void removeBookmark(b);
                  }}
                >
                  ×
                </button>
              </Show>
            </div>
          )}
        </For>
        <For
          each={vols().filter(
            (v) =>
              v.kind === "network" &&
              !bookmarks().some((b) => b.mountPath === v.path),
          )}
        >
          {(v) => (
            <div
              class={`sb-item ${state[state.active].cwd === v.path ? "active" : ""}`}
              onClick={() => go(v.path)}
              onContextMenu={(ev) => openVolMenu(v, ev)}
              title={v.path}
            >
              <span class="sb-icon">🌐</span>
              <span class="sb-label">{v.name}</span>
                <button
                  class="sb-eject"
                  title={t("sidebar.unmount")}
                onClick={(ev) => {
                  ev.stopPropagation();
                  void doEject(v);
                }}
              >
                ⏏
              </button>
            </div>
            )}
          </For>
        <div class="sb-section">{t("sidebar.syncProfiles")}</div>
        <Show
          when={syncProfiles().length > 0}
          fallback={<div class="sb-empty">{t("sidebar.none")}</div>}
        >
          <For each={syncProfiles()}>
            {(profile) => (
              <button
                class="sb-item sb-sync-profile"
                disabled={!!state.job}
                onClick={() => void runSyncProfile(profile.id)}
                title={`${profile.src} → ${profile.dst}`}
              >
                <span class="sb-icon">⇄</span>
                <span class="sb-label">{profile.name}</span>
              </button>
            )}
          </For>
        </Show>
        <Show when={menu()}>
          {(m) => (
            <div
              class="ctx-menu sidebar-ctx"
              ref={(el) => {
                el.style.setProperty("--cx", `${m().x}px`);
                el.style.setProperty("--cy", `${m().y}px`);
              }}
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
              class="ctx-menu sidebar-ctx"
              ref={(el) => {
                el.style.setProperty("--cx", `${m().x}px`);
                el.style.setProperty("--cy", `${m().y}px`);
              }}
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
