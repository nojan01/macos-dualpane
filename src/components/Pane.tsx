import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import {
  state,
  setActive,
  selectOnly,
  toggleSelect,
  selectRange,
  loadPane,
  setSort,
  selTick,
  setFilter,
  focusFilterTick,
  refreshPane,
  toggleHidden,
  togglePreview,
  compareStatus,
  bumpVolumes,
} from "../state";
import {
  commitRename,
  cancelRename,
  selectedEntries,
  beginRename,
  makeFolder,
  makeFile,
  startTransfer,
  deleteSelected,
  duplicateSelected,
  archiveAction,
  createLinksInOther,
  pasteFromClipboard,
} from "../jobs";
import { startPointerDrag, hoverTarget, dragEffect } from "../dnd";
import {
  openDefault,
  quickLook,
  mountDmg,
  findDmgMount,
  detachDmg,
  openPrivacySettings,
  clipboardWriteFiles,
  dragIconPath,
  openTerminal,
  openInEditor,
  listOpenWithApps,
  openWithApp,
  chooseApplication,
  setDefaultApplicationFor,
  type OpenWithApp,
} from "../ipc";
import { openSearch } from "./SearchDialog";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { askConfirm, notifyError } from "./Dialogs";
import { openProperties } from "./PropertiesDialog";
import { syncToOther } from "../sync";
import { openRenameDialog } from "../rename";
import type { PaneId } from "../types";
import { PathBar } from "./PathBar";
import { TabBar } from "./TabBar";
import { FileIcon } from "./FileIcon";
import { t, errMsg } from "../i18n";

// Breite des "Öffnen mit"-Untermenüs. Muss zur Regel `.ctx-submenu` in
// styles.css passen, damit die Platzprüfung an der rechten Fensterkante stimmt.
const OPEN_WITH_WIDTH = 240;

function fmtSize(n: number, isDir: boolean): string {
  if (isDir) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}

function fmtDate(unix: number): string {
  if (!unix) return "";
  const d = new Date(unix * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// Netzprotokolle liefern über NSWorkspace unterschiedlich aussehende (und bei
// virtuellen S3/Swift-Pfaden oft nur weiße) Symbole. DualBeam verwendet daher
// für alle Netzlaufwerke dieselben dateitypabhängigen Symbole. Lokale Dateien
// behalten ihre nativen macOS-Icons.

function entryFallbackIcon(entry: {
  name: string;
  isDir: boolean;
  isSymlink: boolean;
}): string {
  if (entry.isDir) {
    return entry.name.toLowerCase().endsWith(".app") ? "🟦" : "📁";
  }
  if (entry.isSymlink) return "🔗";
  const extension = entry.name.split(".").pop()?.toLowerCase() ?? "";
  if (["zip", "7z", "rar", "tar", "gz", "bz2", "xz"].includes(extension)) return "📦";
  if (["pdf"].includes(extension)) return "📕";
  if (["doc", "docx", "odt", "rtf"].includes(extension)) return "📘";
  if (["xls", "xlsx", "ods", "csv"].includes(extension)) return "📗";
  if (["ppt", "pptx", "odp"].includes(extension)) return "📙";
  if (["jpg", "jpeg", "png", "gif", "webp", "heic", "tiff", "svg"].includes(extension)) return "🖼️";
  if (["mp3", "m4a", "aac", "wav", "flac", "ogg"].includes(extension)) return "🎵";
  if (["mp4", "mov", "mkv", "avi", "webm"].includes(extension)) return "🎬";
  if (["dmg", "iso"].includes(extension)) return "💿";
  if (["txt", "md", "json", "xml", "yaml", "yml", "log"].includes(extension)) return "📝";
  return "📄";
}

export function Pane(props: { id: PaneId }) {
  const id = props.id;
  const pane = () => state[id];
  const isActive = () => state.active === id;

  let rowsEl: HTMLDivElement | undefined;
  let filterEl: HTMLInputElement | undefined;
  let scrollEl: HTMLDivElement | undefined;

  const VIRT_THRESHOLD = 500;
  const ROW_H = 22;
  const OVERSCAN = 10;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [viewH, setViewH] = createSignal(600);

  const virt = createMemo(() => {
    const entries = pane().entries;
    if (entries.length <= VIRT_THRESHOLD) {
      return { enabled: false, items: entries, start: 0, padTop: 0, padBot: 0 };
    }
    const start = Math.max(0, Math.floor(scrollTop() / ROW_H) - OVERSCAN);
    const visible = Math.ceil(viewH() / ROW_H) + 2 * OVERSCAN;
    const end = Math.min(entries.length, start + visible);
    return {
      enabled: true,
      items: entries.slice(start, end),
      start,
      padTop: start * ROW_H,
      padBot: (entries.length - end) * ROW_H,
    };
  });

  function onScroll(ev: Event) {
    const el = ev.currentTarget as HTMLDivElement;
    setScrollTop(el.scrollTop);
    setViewH(el.clientHeight);
  }

  onMount(() => {
    if (scrollEl) {
      setViewH(scrollEl.clientHeight);
      const ro = new ResizeObserver(() => {
        if (scrollEl) setViewH(scrollEl.clientHeight);
      });
      ro.observe(scrollEl);
      onCleanup(() => ro.disconnect());
    }
  });

  // Bei cwd-Wechsel Scroll zurücksetzen, damit Windowing-Index stimmt.
  createEffect(() => {
    pane().cwd;
    if (scrollEl) scrollEl.scrollTop = 0;
    setScrollTop(0);
  });

  createEffect(() => {
    const sig = focusFilterTick();
    if (sig && sig.pane === id && filterEl) {
      filterEl.focus();
      filterEl.select();
    }
  });

  const handleDmg = async (filePath: string) => {
    try {
      const existing = await findDmgMount(filePath);
      if (existing) {
        const eject = await askConfirm({
          title: t("pane.dmg.mountedTitle"),
          message: t("pane.dmg.mountedMessage", { path: existing }),
          okLabel: t("pane.dmg.eject"),
          cancelLabel: t("pane.dmg.openInstead"),
          danger: true,
        });
        if (eject) {
          await detachDmg(filePath);
          bumpVolumes();
        } else {
          await loadPane(id, existing);
        }
        return;
      }
      const mp = await mountDmg(filePath);
      bumpVolumes();
      await loadPane(id, mp);
    } catch (err) {
      console.error("mountDmg:", err);
      await askConfirm({
        title: t("pane.dmg.failed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    }
  };

  const activate = async (idx: number) => {
    const e = pane().entries[idx];
    if (!e) return;
    // .app-Bundles sind technisch Verzeichnisse, sollen aber wie Programme gestartet werden.
    const isApp = e.isDir && e.name.toLowerCase().endsWith(".app");
    if (isApp) await openDefault(e.path);
    else if (e.isDir) await loadPane(id, e.path);
    else await openDefault(e.path);
  };

  const onRowClick = (ev: MouseEvent, idx: number) => {
    setActive(id);
    if (ev.metaKey) toggleSelect(id, idx);
    else if (ev.shiftKey) selectRange(id, idx);
    else selectOnly(id, idx);
  };

  const onRowDblClick = (_ev: MouseEvent, idx: number) => {
    void activate(idx);
  };

  const onRowDragStart = (ev: DragEvent, idx: number) => {
    const p = pane();
    const e = p.entries[idx];
    if (!e) {
      ev.preventDefault();
      return;
    }
    if (!p.selected.has(e.path)) selectOnly(id, idx);
    const items = selectedEntries(id);
    if (items.length === 0) {
      ev.preventDefault();
      return;
    }
    // HTML5-Drag verhindern und nativen macOS-Drag via tauri-plugin-drag starten.
    // Konflikte (Überschreiben) übernimmt das Drop-Ziel (z. B. Finder) selbst.
    ev.preventDefault();
    const paths = items.map((it) => it.path);
    void (async () => {
      try {
        const icon = await dragIconPath();
        await startDrag({ item: paths, icon, mode: "copy" });
      } catch (e) {
        console.error("native drag failed", e);
      }
    })();
  };

  const onRowMouseDown = (ev: MouseEvent, idx: number) => {
    // Nur linke Maustaste startet einen potentiellen Drag.
    if (ev.button !== 0) return;
    // Beim Umbenennen kein Drag.
    if (state.editing && state.editing.pane === id && state.editing.idx === idx)
      return;
    setActive(id);
    const p = pane();
    const e = p.entries[idx];
    if (!e) return;
    // Wenn der Eintrag noch nicht in der Auswahl ist, ihn allein selektieren.
    // Bei Cmd/Shift greift die normale Click-Logik (toggle/range), kein Drag.
    if (ev.metaKey || ev.shiftKey) return;
    if (!p.selected.has(e.path)) selectOnly(id, idx);
    const items = selectedEntries(id);
    if (items.length === 0) return;
    startPointerDrag({ srcPane: id, items }, { x: ev.clientX, y: ev.clientY });
  };

  // ---------- Kontextmenü ----------
  type MenuKind = "row" | "empty";
  const [menu, setMenu] = createSignal<{
    kind: MenuKind;
    x: number;
    y: number;
  } | null>(null);
  const closeMenu = () => {
    setMenu(null);
    resetOpenWith();
  };

  // ---------- Untermenü "Öffnen mit" ----------
  // Die Programmliste kostet einen LaunchServices-Aufruf. Sie wird deshalb
  // erst geholt, wenn der Zeiger den Menüpunkt erreicht, nicht schon beim
  // Öffnen des Kontextmenüs.
  const [owApps, setOwApps] = createSignal<OpenWithApp[] | null>(null);
  const [owFlip, setOwFlip] = createSignal(false);
  let owLoading = false;

  const resetOpenWith = () => {
    setOwApps(null);
    setOwFlip(false);
    owLoading = false;
  };

  const loadOpenWith = (ev: MouseEvent) => {
    // Reicht der Platz rechts nicht, klappt das Untermenü nach links auf.
    const el = ev.currentTarget as HTMLElement;
    const r = el.getBoundingClientRect();
    setOwFlip(r.right + OPEN_WITH_WIDTH > window.innerWidth - 8);

    if (owApps() !== null || owLoading) return;
    const e = firstSel();
    if (!e) return;
    owLoading = true;
    listOpenWithApps(e.path)
      .then(setOwApps)
      // Ein Fehlschlag darf das Menü nicht offen hängen lassen; die leere
      // Liste zeigt dem Nutzer, dass nichts zur Auswahl steht.
      .catch(() => setOwApps([]))
      .finally(() => {
        owLoading = false;
      });
  };

  const onRowContextMenu = (ev: MouseEvent, idx: number) => {
    ev.preventDefault();
    ev.stopPropagation();
    setActive(id);
    const p = pane();
    const e = p.entries[idx];
    if (!e) return;
    if (!p.selected.has(e.path)) selectOnly(id, idx);
    resetOpenWith();
    setMenu({ kind: "row", x: ev.clientX, y: ev.clientY });
  };
  const onEmptyContextMenu = (ev: MouseEvent) => {
    ev.preventDefault();
    setActive(id);
    setMenu({ kind: "empty", x: ev.clientX, y: ev.clientY });
  };

  const sel = () => selectedEntries(id);
  const firstSel = () => sel()[0];
  const isZipSel = () => {
    const e = firstSel();
    return !!e && !e.isDir && e.name.toLowerCase().endsWith(".zip");
  };
  const isDmgSel = () => {
    const e = firstSel();
    return !!e && !e.isDir && e.name.toLowerCase().endsWith(".dmg");
  };
  const isAppSel = () => {
    const e = firstSel();
    return !!e && e.isDir && e.name.toLowerCase().endsWith(".app");
  };

  const actOpen = async () => {
    closeMenu();
    const e = firstSel();
    if (!e) return;
    const isApp = e.isDir && e.name.toLowerCase().endsWith(".app");
    if (isApp) await openDefault(e.path);
    else if (e.isDir) await loadPane(id, e.path);
    else await openDefault(e.path);
  };
  const actQuickLook = async () => {
    closeMenu();
    const e = firstSel();
    if (e) await quickLook(e.path);
  };
  /** Programmname aus dem Bündelpfad, wenn die Liste keinen mitliefert. */
  const appLabel = (p: string) =>
    (p.split("/").pop() ?? p).replace(/\.app$/i, "");
  /** Endung samt Punkt; ohne Endung der ganze Name, damit die Rückfrage
   *  trotzdem etwas Greifbares nennt. */
  const fileExt = (name: string) => {
    const i = name.lastIndexOf(".");
    return i > 0 ? name.slice(i) : name;
  };

  const runOpenWith = async (
    entries: ReturnType<typeof sel>,
    appPath: string,
    always: boolean,
    appName?: string,
  ) => {
    const first = entries[0];
    if (!first) return;
    try {
      if (always) {
        // Die Zuordnung gilt systemweit, nicht nur in DualBeam – das sollte
        // niemand versehentlich auslösen.
        const ok = await askConfirm({
          title: t("pane.ctx.setDefaultTitle"),
          message: t("pane.ctx.setDefaultMsg", {
            ext: fileExt(first.name),
            app: appName ?? appLabel(appPath),
          }),
          okLabel: t("pane.ctx.setDefaultOk"),
        });
        if (!ok) return;
        await setDefaultApplicationFor(appPath, first.path);
      }
      await openWithApp(
        entries.map((e) => e.path),
        appPath,
      );
    } catch (e) {
      await notifyError(errMsg(e));
    }
  };

  const actOpenWith = (appPath: string, always: boolean, appName?: string) => {
    // Verzeichnisse bleiben aussen vor: LaunchServices würde sie an den Finder
    // weiterreichen statt an das gewählte Programm.
    const entries = sel().filter((e) => !e.isDir);
    closeMenu();
    void runOpenWith(entries, appPath, always, appName);
  };

  const actOpenWithOther = async (always: boolean) => {
    // Die Auswahl vor dem Schliessen sichern: Der Dialog läuft asynchron, bis
    // zu seiner Rückkehr kann sich die Markierung geändert haben.
    const entries = sel().filter((e) => !e.isDir);
    closeMenu();
    if (entries.length === 0) return;
    try {
      const app = await chooseApplication();
      // Abbruch ist kein Fehler und bleibt deshalb ohne Meldung.
      if (!app) return;
      await runOpenWith(entries, app, always);
    } catch (e) {
      await notifyError(errMsg(e));
    }
  };
  /** Menüeintrag samt Untermenü – einmal zum einmaligen Öffnen, einmal zum
   *  dauerhaften Festlegen. Beide teilen sich dieselbe Programmliste. */
  const openWithEntry = (always: boolean) => (
    <div class="ctx-item ctx-sub" onMouseEnter={loadOpenWith}>
      <span>
        {always ? t("pane.ctx.openWithAlways") : t("pane.ctx.openWith")}
      </span>
      <span class="ctx-sub-arrow" aria-hidden="true">
        ›
      </span>
      <div class="ctx-submenu" classList={{ flip: owFlip() }}>
        <Show
          when={owApps()}
          fallback={
            <div class="ctx-hint">{t("pane.ctx.openWithLoading")}</div>
          }
        >
          {(apps) => (
            <Show
              when={apps().length > 0}
              fallback={
                <div class="ctx-hint">{t("pane.ctx.openWithEmpty")}</div>
              }
            >
              <For each={apps()}>
                {(a) => (
                  <div
                    class="ctx-item ctx-app"
                    onClick={() => actOpenWith(a.path, always, a.name)}
                  >
                    <FileIcon path={a.path} fallback="📦" />
                    <span class="ctx-app-name">
                      {a.isDefault && !always
                        ? t("pane.ctx.openWithDefault", { name: a.name })
                        : a.name}
                    </span>
                  </div>
                )}
              </For>
            </Show>
          )}
        </Show>
        <div class="ctx-sep" />
        <div class="ctx-item" onClick={() => void actOpenWithOther(always)}>
          {t("pane.ctx.openWithOther")}
        </div>
      </div>
    </div>
  );

  const actRename = () => {
    closeMenu();
    beginRename();
  };
  const actBulkRename = () => {
    closeMenu();
    openRenameDialog();
  };
  const actDuplicate = async () => {
    closeMenu();
    await duplicateSelected();
  };
  const actCopy = async () => {
    closeMenu();
    await startTransfer("copy");
  };
  const actMove = async () => {
    closeMenu();
    await startTransfer("move");
  };
  const actSymlink = async () => {
    closeMenu();
    await createLinksInOther("symlink");
  };
  const actAlias = async () => {
    closeMenu();
    await createLinksInOther("alias");
  };
  const actSync = async () => {
    closeMenu();
    await syncToOther();
  };
  const actMountDmg = async () => {
    closeMenu();
    const e = firstSel();
    if (!e) return;
    await handleDmg(e.path);
  };
  const actArchive = async () => {
    closeMenu();
    await archiveAction();
  };
  const actDelete = async (skipConfirm: boolean) => {
    closeMenu();
    await deleteSelected(skipConfirm);
  };
  const actNewFolder = async () => {
    closeMenu();
    await makeFolder();
  };
  const actNewFile = async () => {
    closeMenu();
    await makeFile();
  };
  const actRefresh = async () => {
    closeMenu();
    await refreshPane(id);
  };
  const actToggleHidden = () => {
    closeMenu();
    toggleHidden();
  };
  const actTogglePreview = () => {
    closeMenu();
    togglePreview();
  };
  const actPaste = async () => {
    closeMenu();
    await pasteFromClipboard(id);
  };
  const actProperties = () => {
    closeMenu();
    const e = firstSel();
    if (e) void openProperties(e.path);
  };
  const actCopyPath = async () => {
    closeMenu();
    const paths = sel().map((e) => e.path);
    if (paths.length === 0) return;
    const text = paths.join("\n");
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Fallback via versteckte Textarea
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        /* ignore */
      }
      document.body.removeChild(ta);
    }
  };

  const actCopyFiles = async () => {
    closeMenu();
    const paths = sel().map((e) => e.path);
    if (paths.length === 0) return;
    try {
      await clipboardWriteFiles(paths);
    } catch (e) {
      console.error("clipboardWriteFiles failed", e);
    }
  };

  const actOpenTerminal = async () => {
    closeMenu();
    const e = firstSel();
    const target = e ? e.path : state[id].cwd;
    try {
      await openTerminal(target);
    } catch (err) {
      console.error("openTerminal failed", err);
    }
  };
  const actOpenInEditor = async () => {
    closeMenu();
    const e = firstSel();
    if (!e) return;
    try {
      await openInEditor(e.path);
    } catch (err) {
      console.error("openInEditor failed", err);
    }
  };

  return (
    <div
      class={`pane ${isActive() ? "active" : ""} ${
        hoverTarget()?.pane === id
          ? `dnd-target dnd-${dragEffect() ?? "move"}`
          : ""
      } ${state.extendedView ? "extended" : ""}`}
      onMouseDown={() => setActive(id)}
    >
      <TabBar id={id} />
      <PathBar id={id} />
      <div class="filter-bar">
        <span class="filter-icon">🔍</span>
        <input
          ref={filterEl}
          class="filter-input"
          type="text"
          placeholder={t("pane.filter.placeholder")}
          title={t("pane.filter.placeholder")}
          value={pane().filter}
          onInput={(e) =>
            setFilter(id, (e.currentTarget as HTMLInputElement).value)
          }
          onKeyDown={(ev) => {
            ev.stopPropagation();
            if (ev.key === "Escape") {
              ev.preventDefault();
              setFilter(id, "");
              (ev.currentTarget as HTMLInputElement).blur();
            } else if (ev.key === "Enter") {
              ev.preventDefault();
              (ev.currentTarget as HTMLInputElement).blur();
            }
          }}
          onFocus={() => setActive(id)}
        />
        <Show when={pane().filter}>
          <button
            class="filter-clear"
            title={t("pane.filter.clear")}
            onClick={() => setFilter(id, "")}
          >
            ✕
          </button>
        </Show>
        <button
          class="filter-search"
          title={t("pane.filter.searchRecursive")}
          onClick={() => {
            setActive(id);
            openSearch();
          }}
        >
          🔎
        </button>
      </div>
      <div
        class="table-scroll"
        ref={scrollEl}
        onScroll={onScroll}
        onContextMenu={onEmptyContextMenu}
      >
        <div
          class="rows"
          ref={rowsEl}
          tabIndex={-1}
          onContextMenu={onEmptyContextMenu}
        >
          <div class="col-header">
            <div onClick={() => setSort(id, "name")}>
              {t("pane.col.name")}{" "}
              <span class="sort-ind">
                {pane().sortKey === "name"
                  ? pane().sortDir === "asc"
                    ? "▲"
                    : "▼"
                  : ""}
              </span>
            </div>
            <div onClick={() => setSort(id, "size")}>
              {t("pane.col.size")}{" "}
              <span class="sort-ind">
                {pane().sortKey === "size"
                  ? pane().sortDir === "asc"
                    ? "▲"
                    : "▼"
                  : ""}
              </span>
            </div>
            <div onClick={() => setSort(id, "mtime")}>
              {t("pane.col.modified")}{" "}
              <span class="sort-ind">
                {pane().sortKey === "mtime"
                  ? pane().sortDir === "asc"
                    ? "▲"
                    : "▼"
                  : ""}
              </span>
            </div>
            <Show when={state.extendedView}>
              <div>{t("pane.col.kind")}</div>
              <div>{t("pane.col.created")}</div>
              <div>{t("pane.col.owner")}</div>
              <div>{t("pane.col.perms")}</div>
            </Show>
          </div>
          <Show when={pane().error}>
            <div class="error">
              {(() => {
                const msg = pane().error || "";
                const perm =
                  /permission denied|operation not permitted|os error 1\b|os error 13\b|EACCES|EPERM/i.test(
                    msg,
                  );
                if (!perm) return <span>{msg}</span>;
                return (
                  <>
                    <div>{t("pane.error.permission")}</div>
                    <div class="pane-error-detail">{msg}</div>
                    <button
                      class="pane-error-btn"
                      onClick={() => {
                        void openPrivacySettings();
                      }}
                    >
                      {t("pane.error.openSettings")}
                    </button>
                  </>
                );
              })()}
            </div>
          </Show>
          <Show when={virt().enabled}>
            <div
              class="virt-pad"
              ref={(el) =>
                createEffect(() =>
                  el.style.setProperty("--vh", `${virt().padTop}px`),
                )
              }
            />
          </Show>
          <For each={virt().items}>
            {(e, i) => {
              // selTick als unsichtbare Abhängigkeit, damit Set-Mutationen Re-render auslösen
              selTick();
              const idx = () => virt().start + i();
              const isCursor = () => pane().cursor === idx() && isActive();
              const isSel = () => pane().selected.has(e.path);
              const cmp = () => compareStatus(id, e);
              return (
                <div
                  data-index={idx()}
                  class={`row ${isCursor() ? "cursor" : ""} ${isSel() ? "selected" : ""} ${
                    hoverTarget()?.pane === id &&
                    hoverTarget()?.folderIdx === idx()
                      ? "dnd-over"
                      : ""
                  } ${cmp() ? `cmp-${cmp()}` : ""}`}
                  onMouseDown={(ev) => onRowMouseDown(ev, idx())}
                  onClick={(ev) => onRowClick(ev, idx())}
                  onDblClick={(ev) => onRowDblClick(ev, idx())}
                  onContextMenu={(ev) => onRowContextMenu(ev, idx())}
                  draggable={true}
                  onDragStart={(ev) => onRowDragStart(ev, idx())}
                >
                  <div class="name">
                    <FileIcon
                      path={e.path}
                      fallback={entryFallbackIcon(e)}
                      nativeIcon={!pane().isNetwork}
                    />
                    <Show
                      when={
                        state.editing &&
                        state.editing.pane === id &&
                        state.editing.idx === idx()
                      }
                      fallback={<span>{e.name}</span>}
                    >
                      <input
                        class="rename-input"
                        title={t("pane.rename.title")}
                        aria-label={t("pane.rename.title")}
                        placeholder={t("pane.rename.placeholder")}
                        value={e.name}
                        autofocus
                        ref={(el) => {
                          queueMicrotask(() => {
                            el.focus();
                            const dot = e.name.lastIndexOf(".");
                            el.setSelectionRange(
                              0,
                              dot > 0 ? dot : e.name.length,
                            );
                          });
                        }}
                        onClick={(ev) => ev.stopPropagation()}
                        onDblClick={(ev) => ev.stopPropagation()}
                        onKeyDown={(ev) => {
                          ev.stopPropagation();
                          if (ev.key === "Enter") {
                            ev.preventDefault();
                            void commitRename(
                              (ev.currentTarget as HTMLInputElement).value,
                            );
                          } else if (ev.key === "Escape") {
                            ev.preventDefault();
                            cancelRename();
                          }
                        }}
                        onBlur={(ev) => {
                          // Commit on blur
                          void commitRename(
                            (ev.currentTarget as HTMLInputElement).value,
                          );
                        }}
                      />
                    </Show>
                  </div>
                  <div class="size">{fmtSize(e.size, e.isDir)}</div>
                  <div class="mtime">{fmtDate(e.mtime)}</div>
                  <Show when={state.extendedView}>
                    <div class="kind">{e.kind ?? ""}</div>
                    <div class="ctime">
                      {e.birthTime ? fmtDate(e.birthTime) : ""}
                    </div>
                    <div class="owner">
                      {e.owner ?? ""}
                      {e.group ? `:${e.group}` : ""}
                    </div>
                    <div class="mode">{e.modeStr ?? ""}</div>
                  </Show>
                </div>
              );
            }}
          </For>
          <Show when={virt().enabled}>
            <div
              class="virt-pad"
              ref={(el) =>
                createEffect(() =>
                  el.style.setProperty("--vh", `${virt().padBot}px`),
                )
              }
            />
          </Show>
        </div>
      </div>
      <Show when={menu()}>
        {(m) => (
          <>
            <div
              class="ctx-backdrop"
              onMouseDown={(ev) => {
                ev.preventDefault();
                closeMenu();
              }}
              onContextMenu={(ev) => {
                ev.preventDefault();
                closeMenu();
              }}
            />
            <div
              class="ctx-menu ctx-pos"
              onMouseDown={(ev) => ev.stopPropagation()}
              onContextMenu={(ev) => ev.preventDefault()}
              ref={(el) => {
                el.style.setProperty("--cx", `${m().x}px`);
                el.style.setProperty("--cy", `${m().y}px`);
                queueMicrotask(() => {
                  const r = el.getBoundingClientRect();
                  const pad = 8;
                  let nx = m().x;
                  let ny = m().y;
                  if (r.bottom > window.innerHeight - pad) {
                    ny = Math.max(pad, window.innerHeight - r.height - pad);
                  }
                  if (r.right > window.innerWidth - pad) {
                    nx = Math.max(pad, window.innerWidth - r.width - pad);
                  }
                  el.style.left = `${nx}px`;
                  el.style.top = `${ny}px`;
                });
              }}
            >
              <Show when={m().kind === "row"}>
                <div class="ctx-item" onClick={() => void actOpen()}>
                  {t("pane.ctx.open")}
                </div>
                <Show when={firstSel() && !firstSel()!.isDir}>
                  {openWithEntry(false)}
                  {openWithEntry(true)}
                </Show>
                <div class="ctx-item" onClick={() => void actQuickLook()}>
                  {t("pane.ctx.quickLook")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={actRename}>
                  {t("pane.ctx.rename")}
                </div>
                <Show when={sel().length >= 2}>
                  <div class="ctx-item" onClick={actBulkRename}>
                    {t("pane.ctx.multiRename", { count: sel().length })}
                  </div>
                </Show>
                <div class="ctx-item" onClick={() => void actDuplicate()}>
                  {t("pane.ctx.duplicate")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={() => void actCopy()}>
                  {id === "left"
                    ? t("pane.ctx.copyToRight")
                    : t("pane.ctx.copyToLeft")}
                </div>
                <div class="ctx-item" onClick={() => void actMove()}>
                  {id === "left"
                    ? t("pane.ctx.moveToRight")
                    : t("pane.ctx.moveToLeft")}
                </div>
                <div class="ctx-item" onClick={() => void actSymlink()}>
                  {id === "left"
                    ? t("pane.ctx.symlinkToRight")
                    : t("pane.ctx.symlinkToLeft")}
                </div>
                <div class="ctx-item" onClick={() => void actAlias()}>
                  {id === "left"
                    ? t("pane.ctx.aliasToRight")
                    : t("pane.ctx.aliasToLeft")}
                </div>
                <Show when={sel().length === 1 && firstSel()?.isDir}>
                  <div class="ctx-item" onClick={() => void actSync()}>
                    {id === "left"
                      ? t("pane.ctx.syncToRight")
                      : t("pane.ctx.syncToLeft")}
                  </div>
                </Show>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={() => void actNewFolder()}>
                  {t("pane.ctx.newFolder")}
                </div>
                <div class="ctx-item" onClick={() => void actNewFile()}>
                  {t("pane.ctx.newFile")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={() => void actArchive()}>
                  {isZipSel() ? t("pane.ctx.extract") : t("pane.ctx.zip")}
                </div>
                <Show when={isDmgSel()}>
                  <div class="ctx-item" onClick={() => void actMountDmg()}>
                    {t("pane.ctx.mountDmg")}
                  </div>
                </Show>
                <Show when={isAppSel()}>
                  <div
                    class="ctx-item"
                    onClick={async () => {
                      closeMenu();
                      const e = firstSel();
                      if (e) await loadPane(id, e.path);
                    }}
                  >
                    {t("pane.ctx.packageContents")}
                  </div>
                </Show>
                <div class="ctx-sep" />
                <div
                  class="ctx-item danger"
                  onClick={(ev) => void actDelete(ev.shiftKey)}
                >
                  {sel().length > 1
                    ? t("pane.ctx.trashN", { count: sel().length })
                    : t("pane.ctx.trash")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={() => void actCopyFiles()}>
                  {sel().length > 1
                    ? t("pane.ctx.copyN", { count: sel().length })
                    : t("pane.ctx.copy")}
                </div>
                <div class="ctx-item" onClick={() => void actPaste()}>
                  {t("pane.ctx.paste")}
                </div>
                <div class="ctx-item" onClick={() => void actCopyPath()}>
                  {sel().length > 1
                    ? t("pane.ctx.copyPathN", { count: sel().length })
                    : t("pane.ctx.copyPath")}
                </div>
                <Show when={sel().length === 1 && !firstSel()?.isDir}>
                  <div class="ctx-item" onClick={() => void actOpenInEditor()}>
                    {t("pane.ctx.openInEditor")}
                  </div>
                </Show>
                <div class="ctx-item" onClick={() => void actOpenTerminal()}>
                  {t("pane.ctx.openTerminal")}
                </div>
                <div class="ctx-item" onClick={actProperties}>
                  {t("pane.ctx.properties")}
                </div>
              </Show>
              <Show when={m().kind === "empty"}>
                <div class="ctx-item" onClick={() => void actPaste()}>
                  {t("pane.ctx.paste")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={() => void actNewFolder()}>
                  {t("pane.ctx.newFolder")}
                </div>
                <div class="ctx-item" onClick={() => void actNewFile()}>
                  {t("pane.ctx.newFile")}
                </div>
                <div class="ctx-item" onClick={() => void actRefresh()}>
                  {t("pane.ctx.refresh")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={() => void actOpenTerminal()}>
                  {t("pane.ctx.openTerminal")}
                </div>
                <div class="ctx-sep" />
                <div class="ctx-item" onClick={actToggleHidden}>
                  {state.showHidden
                    ? t("pane.ctx.hiddenHide")
                    : t("pane.ctx.hiddenShow")}
                </div>
                <div class="ctx-item" onClick={actTogglePreview}>
                  {state.previewVisible
                    ? t("pane.ctx.previewHide")
                    : t("pane.ctx.previewShow")}
                </div>
              </Show>
            </div>
          </>
        )}
      </Show>
    </div>
  );
}
