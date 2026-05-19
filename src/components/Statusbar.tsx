import { state, selTick, refreshPane, toggleHidden, toggleHelp } from "../state";
import { beginRename, makeFolder, startTransfer, deleteSelected, selectedEntries, archiveAction } from "../jobs";
import { quickLook, openTerminal } from "../ipc";
import { Show } from "solid-js";
import { t } from "../i18n";

async function doQuickLook() {
  const sel = selectedEntries(state.active);
  if (sel[0]) await quickLook(sel[0].path);
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"]; let v = n / 1024; let i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${u[i]}`;
}

export function Statusbar() {
  const stats = () => {
    selTick();
    const p = state[state.active];
    const sel = p.entries.filter((e) => p.selected.has(e.path));
    const bytes = sel.reduce((s, e) => s + (e.isDir ? 0 : e.size), 0);
    return { total: p.entries.length, selCount: sel.length, bytes };
  };
  return (
    <div class="statusbar">
      {stats().total}{t("status.items")}
      {stats().selCount > 0 && <>{t("status.selected", { count: stats().selCount, size: fmtBytes(stats().bytes) })}</>}
      <span style="margin-left:auto;color:var(--text-dim)">
        {state.active === "left"
          ? t("status.activeLeft", { hidden: state.showHidden ? t("status.hiddenOn") : t("status.hiddenOff") })
          : t("status.activeRight", { hidden: state.showHidden ? t("status.hiddenOn") : t("status.hiddenOff") })}
      </span>
    </div>
  );
}

const SHORTCUTS: { key: string; descKey: string }[] = [
  { key: "F1", descKey: "hb.help" },
  { key: "F2", descKey: "hb.rename" },
  { key: "__F3__", descKey: "hb.preview" },
  { key: "F5", descKey: "hb.copy" },
  { key: "F6", descKey: "hb.move" },
  { key: "F7", descKey: "hb.newFolder" },
  { key: "F8 / ⌫", descKey: "hb.delete" },
  { key: "Tab", descKey: "hb.switchPane" },
  { key: "↵", descKey: "hb.open" },
  { key: "⌫", descKey: "hb.up" },
  { key: "⌘T", descKey: "hb.newTab" },
  { key: "⌘W", descKey: "hb.closeTab" },
  { key: "⌘1…9", descKey: "hb.switchTab" },
  { key: "⌘B", descKey: "hb.sidebar" },
  { key: "⌘I", descKey: "hb.previewToggle" },
  { key: "⌘.", descKey: "hb.hidden" },
  { key: "⌘R", descKey: "hb.refresh" },
  { key: "⌘⇧F", descKey: "hb.search" },
  { key: "⌘E", descKey: "hb.zip" },
  { key: "/", descKey: "hb.filter" },
];

export function HelpBar() {
  return (
    <Show when={state.helpVisible}>
      <div class="helpbar">
        {SHORTCUTS.map((s) => (
          <span class="help-item"><kbd>{s.key === "__F3__" ? t("hb.keys.f3") : s.key}</kbd> {t(s.descKey)}</span>
        ))}
      </div>
    </Show>
  );
}

export function FnBar() {
  return (
    <div class="fnbar">
      <button class="fn-btn" title={t("fn.help.title")} onClick={() => toggleHelp()}>
        <b>F1</b> {t("fn.help")}
      </button>
      <button class="fn-btn" title={t("fn.rename.title")} onClick={() => beginRename()}>
        <b>F2</b> {t("fn.rename")}
      </button>
      <button class="fn-btn" title={t("fn.preview.title")} onClick={() => void doQuickLook()}>
        <b>F3</b> {t("fn.preview")}
      </button>
      <button class="fn-btn" title={t("fn.copy.title")} onClick={() => void startTransfer("copy")}>
        <b>F5</b> {t("fn.copy")}
      </button>
      <button class="fn-btn" title={t("fn.move.title")} onClick={() => void startTransfer("move")}>
        <b>F6</b> {t("fn.move")}
      </button>
      <button class="fn-btn" title={t("fn.newFolder.title")} onClick={() => void makeFolder()}>
        <b>F7</b> {t("fn.newFolder")}
      </button>
      <button class="fn-btn danger" title={t("fn.delete.title")} onClick={(ev) => void deleteSelected(ev.shiftKey)}>
        <b>F8</b> {t("fn.delete")}
      </button>
      <button class="fn-btn" title={t("fn.zip.title")} onClick={() => void archiveAction()}>
        <b>⌘E</b> {t("fn.zip")}
      </button>
      <button class="fn-btn" title={t("fn.refresh.title")} onClick={() => void refreshPane(state.active)}>
        <b>⌘R</b> {t("fn.refresh")}
      </button>
      <button class="fn-btn" title={t("fn.terminal.title")} onClick={() => { const cwd = state[state.active]?.cwd; if (cwd) void openTerminal(cwd); }}>
        <b>⌃T</b> {t("fn.terminal")}
      </button>
      <button class="fn-btn" title={t("fn.hidden.title")} onClick={() => toggleHidden()}>
        <b>⌘.</b> {t("fn.hidden")}
      </button>
      <span style="margin-left:auto;padding:0 8px;color:var(--text-dim);opacity:0.7;font-size:11px;white-space:nowrap" title="DualBeam — MIT License">
        © 2026 N.J. · MIT
      </span>
    </div>
  );
}
