import { Show, For, Index, createMemo, createSignal } from "solid-js";
import {
  renameSession, renameOptions, closeRenameDialog, applyRename,
  computePreviews, addOp, removeOp, updateOp, changeOpKind,
  renamePresets, savePreset, loadPreset, deletePreset,
  type Op, type OpReplace, type OpAdd, type OpCase, type OpSetExt,
  type Scope, type ReplaceMode, type CaseMode, type NumberPos,
} from "../rename";
import { askConfirm } from "./Dialogs";
import { t } from "../i18n";

const scopeLabel = (s: Scope) =>
  s === "full" ? t("rn.scope.full") : s === "base" ? t("rn.scope.name") : t("rn.scope.ext");

const modeLabel = (m: ReplaceMode) =>
  m === "all" ? t("rn.mode.every")
  : m === "first" ? t("rn.mode.first")
  : m === "last" ? t("rn.mode.last")
  : m === "start" ? t("rn.mode.start")
  : t("rn.mode.end");

const caseLabel = (c: CaseMode) =>
  c === "keep" ? t("rn.case.none")
  : c === "lower" ? t("rn.case.lower")
  : c === "upper" ? t("rn.case.upper")
  : t("rn.case.title");

const numPosLabel = (p: NumberPos) =>
  p === "none" ? t("rn.num.none") : p === "before" ? t("rn.num.before") : t("rn.num.after");

export function RenameDialog() {
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [saveMode, setSaveMode] = createSignal(false);
  const [saveName, setSaveName] = createSignal("");
  const commitSave = () => {
    const n = saveName().trim();
    if (n) savePreset(n);
    setSaveName("");
    setSaveMode(false);
  };

  const previews = createMemo(() => {
    const s = renameSession();
    if (!s) return [];
    return computePreviews(s.items, renameOptions());
  });

  const hasError = createMemo(() => previews().some((p) => p.error));
  const changedCount = createMemo(() => previews().filter((p) => p.changed && !p.error).length);
  const totalCount = createMemo(() => previews().length);

  const onApply = async () => {
    setError(null);
    setBusy(true);
    try {
      const r = await applyRename();
      if (!r.ok) setError(r.message ?? t("common.unknownError"));
    } finally {
      setBusy(false);
    }
  };

  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") { ev.preventDefault(); closeRenameDialog(); }
    else if (ev.key === "Enter" && (ev.metaKey || ev.ctrlKey)) {
      ev.preventDefault();
      void onApply();
    }
  };

  return (
    <Show when={renameSession()}>
      {(s) => (
        <div class="modal-backdrop" onClick={() => closeRenameDialog()}>
          <div
            class="modal modal-rename2"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={onKey}
          >
            <div class="rn-presets">
              <label>{t("rn.preset.label")}</label>
              <select
                title={t("rn.preset.load")}
                value=""
                onChange={(e) => {
                  const v = e.currentTarget.value;
                  if (v) loadPreset(v);
                  e.currentTarget.value = "";
                }}
              >
                <option value="">{t("rn.preset.choose")}</option>
                <For each={renamePresets()}>
                  {(p) => <option value={p.name}>{p.name}</option>}
                </For>
              </select>
              <button
                class="secondary"
                title={t("rn.preset.saveTitle")}
                onClick={() => { setSaveName(""); setSaveMode(true); }}
              >{t("rn.preset.save")}</button>
              <Show when={saveMode()}>
                <input
                  type="text"
                  placeholder={t("rn.preset.nameLabel")}
                  title={t("rn.preset.nameLabel")}
                  autofocus
                  value={saveName()}
                  onInput={(e) => setSaveName(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") { e.preventDefault(); commitSave(); }
                    else if (e.key === "Escape") { e.preventDefault(); setSaveMode(false); setSaveName(""); }
                  }}
                />
                <button class="primary" onClick={commitSave} disabled={!saveName().trim()}>{t("common.ok")}</button>
                <button class="secondary" onClick={() => { setSaveMode(false); setSaveName(""); }}>{t("common.cancel")}</button>
              </Show>
              <Show when={renamePresets().length > 0}>
                <select
                  title={t("rn.preset.deleteTitle")}
                  value=""
                  onChange={async (e) => {
                    const v = e.currentTarget.value;
                    e.currentTarget.value = "";
                    if (v && await askConfirm({ title: t("rn.preset.deleteConfirm", { name: v }), danger: true, okLabel: t("common.delete") })) deletePreset(v);
                  }}
                >
                  <option value="">{t("rn.preset.delete")}</option>
                  <For each={renamePresets()}>
                    {(p) => <option value={p.name}>{p.name}</option>}
                  </For>
                </select>
              </Show>
            </div>
            <div class="rn-body">
              <div class="rn-ops">
                <Index each={renameOptions().ops}>
                  {(op, i) => (
                    <div class="rn-op">
                      <div class="rn-op-head">
                        <label>{t("rn.op.label")}</label>
                        <select
                          title={t("rn.op.title")}
                          value={op().kind}
                          onChange={(e) => changeOpKind(i, e.currentTarget.value as Op["kind"])}
                        >
                          <option value="replace">{t("rn.op.replace")}</option>
                          <option value="add">{t("rn.op.add")}</option>
                          <option value="case">{t("rn.op.case")}</option>
                          <option value="setExt">{t("rn.op.ext")}</option>
                        </select>
                        <div class="rn-op-actions">
                          <Show when={renameOptions().ops.length > 1}>
                            <button
                              class="rn-iconbtn"
                              title={t("rn.op.remove")}
                              onClick={() => removeOp(i)}
                            >−</button>
                          </Show>
                          <Show when={i === renameOptions().ops.length - 1}>
                            <button
                              class="rn-iconbtn"
                              title={t("rn.op.add2")}
                              onClick={() => addOp("replace")}
                            >+</button>
                          </Show>
                        </div>
                      </div>

                      <Show when={op().kind !== "setExt"}>
                        <div class="rn-row">
                          <label>{t("rn.applyFor")}</label>
                          <select
                            title={t("rn.applyFor")}
                            value={(op() as OpReplace | OpAdd | OpCase).scope}
                            onChange={(e) => updateOp(i, { scope: e.currentTarget.value as Scope })}
                          >
                            <option value="full">{scopeLabel("full")}</option>
                            <option value="base">{scopeLabel("base")}</option>
                            <option value="ext">{scopeLabel("ext")}</option>
                          </select>
                        </div>
                      </Show>

                      <Show when={op().kind === "replace"}>
                        {(() => {
                          const r = () => op() as OpReplace;
                          return <>
                            <div class="rn-row">
                              <label>{t("rn.mode")}</label>
                              <select
                                title={t("rn.mode")}
                                value={r().mode}
                                onChange={(e) => updateOp(i, { mode: e.currentTarget.value as ReplaceMode })}
                              >
                                <option value="all">{modeLabel("all")}</option>
                                <option value="first">{modeLabel("first")}</option>
                                <option value="last">{modeLabel("last")}</option>
                                <option value="start">{modeLabel("start")}</option>
                                <option value="end">{modeLabel("end")}</option>
                              </select>
                            </div>
                            <div class="rn-row">
                              <label>{t("rn.replaceLabel")}</label>
                              <input
                                type="text"
                                title={t("rn.search")}
                                value={r().find}
                                onInput={(e) => updateOp(i, { find: e.currentTarget.value })}
                              />
                            </div>
                            <div class="rn-row">
                              <label>{t("rn.withLabel")}</label>
                              <input
                                type="text"
                                title={t("rn.replace")}
                                value={r().replace}
                                placeholder={t("rn.counterHint", { n: "{n}" })}
                                onInput={(e) => updateOp(i, { replace: e.currentTarget.value })}
                              />
                            </div>
                            <div class="rn-row rn-flags">
                              <label></label>
                              <div>
                                <label class="chk">
                                  <input
                                    type="checkbox"
                                    checked={r().caseSensitive}
                                    onChange={(e) => updateOp(i, { caseSensitive: e.currentTarget.checked })}
                                  /> {t("rn.caseSensitive")}
                                </label>
                                <label class="chk">
                                  <input
                                    type="checkbox"
                                    checked={r().regex}
                                    onChange={(e) => updateOp(i, { regex: e.currentTarget.checked })}
                                  /> {t("rn.regex")}
                                </label>
                              </div>
                            </div>
                          </>;
                        })()}
                      </Show>

                      <Show when={op().kind === "add"}>
                        {(() => {
                          const a = () => op() as OpAdd;
                          return <>
                            <div class="rn-row">
                              <label>{t("rn.prefix")}</label>
                              <input
                                type="text"
                                title={t("rn.prefix")}
                                value={a().prefix}
                                onInput={(e) => updateOp(i, { prefix: e.currentTarget.value })}
                              />
                            </div>
                            <div class="rn-row">
                              <label>{t("rn.suffix")}</label>
                              <input
                                type="text"
                                title={t("rn.suffix")}
                                value={a().suffix}
                                onInput={(e) => updateOp(i, { suffix: e.currentTarget.value })}
                              />
                            </div>
                            <div class="rn-row">
                              <label>{t("rn.number")}</label>
                              <select
                                title={t("rn.numberPos")}
                                value={a().numberPos}
                                onChange={(e) => updateOp(i, { numberPos: e.currentTarget.value as NumberPos })}
                              >
                                <option value="none">{numPosLabel("none")}</option>
                                <option value="before">{numPosLabel("before")}</option>
                                <option value="after">{numPosLabel("after")}</option>
                              </select>
                            </div>
                            <Show when={a().numberPos !== "none"}>
                              <div class="rn-row rn-flags">
                                <label>{t("rn.startPad")}</label>
                                <div>
                                  <input
                                    class="num"
                                    type="number"
                                    title={t("rn.startVal")}
                                    value={a().numberStart}
                                    onInput={(e) => updateOp(i, { numberStart: Number(e.currentTarget.value) || 0 })}
                                  />
                                  <input
                                    class="num"
                                    type="number"
                                    title={t("rn.padding")}
                                    min="0"
                                    max="8"
                                    value={a().numberPad}
                                    onInput={(e) => updateOp(i, { numberPad: Number(e.currentTarget.value) || 0 })}
                                  />
                                </div>
                              </div>
                            </Show>
                          </>;
                        })()}
                      </Show>

                      <Show when={op().kind === "case"}>
                        {(() => {
                          const c = () => op() as OpCase;
                          return <div class="rn-row">
                            <label>{t("rn.case")}</label>
                            <select
                              title={t("rn.case")}
                              value={c().caseMode}
                              onChange={(e) => updateOp(i, { caseMode: e.currentTarget.value as CaseMode })}
                            >
                              <option value="keep">{caseLabel("keep")}</option>
                              <option value="lower">{caseLabel("lower")}</option>
                              <option value="upper">{caseLabel("upper")}</option>
                              <option value="title">{caseLabel("title")}</option>
                            </select>
                          </div>;
                        })()}
                      </Show>

                      <Show when={op().kind === "setExt"}>
                        {(() => {
                          const x = () => op() as OpSetExt;
                          return <div class="rn-row">
                            <label>{t("rn.newExt")}</label>
                            <input
                              type="text"
                              title={t("rn.newExtTitle")}
                              placeholder={t("rn.newExtPh")}
                              value={x().ext}
                              onInput={(e) => updateOp(i, { ext: e.currentTarget.value })}
                            />
                          </div>;
                        })()}
                      </Show>
                    </div>
                  )}
                </Index>
              </div>

              <div class="rn-preview">
                <For each={previews()}>
                  {(p) => (
                    <div class={`rn-pv-row ${p.error ? "err" : p.changed ? "ok" : "noop"}`}>
                      <span class="rn-pv-old">
                        <span class="rn-icon">{p.src.isDir ? "📁" : "📄"}</span>
                        {p.src.name}
                      </span>
                      <span class="rn-pv-arrow">{p.error ? "⛔" : "➜"}</span>
                      <span class="rn-pv-new">
                        {p.error ? p.error : p.newName}
                      </span>
                    </div>
                  )}
                </For>
              </div>
            </div>

            <Show when={error()}>
              <div class="rename-error">{error()}</div>
            </Show>

            <div class="rn-footer">
              <span class="rn-status">
                {hasError()
                  ? t("rn.summaryConflicts", { changed: changedCount(), total: totalCount() })
                  : t("rn.summary", { changed: changedCount(), total: totalCount() })}
              </span>
              <div class="rn-buttons">
                <button class="secondary" onClick={() => closeRenameDialog()} disabled={busy()}>
                  {t("common.cancel")}
                </button>
                <button
                  class="primary"
                  onClick={() => void onApply()}
                  disabled={busy() || hasError() || changedCount() === 0}
                >
                  {t("rn.do")}
                </button>
              </div>
            </div>
            <Show when={false}><span>{s().items.length}</span></Show>
          </div>
        </div>
      )}
    </Show>
  );
}
