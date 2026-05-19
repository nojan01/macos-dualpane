import { Show, createSignal, createResource, createEffect } from "solid-js";
import { getProperties, setPermissions } from "../ipc";
import { t, intlLocale } from "../i18n";

const [openPath, setOpenPath] = createSignal<string | null>(null);
let resolver: (() => void) | null = null;

export function openProperties(path: string): Promise<void> {
  setOpenPath(path);
  return new Promise((res) => { resolver = res; });
}

function close() {
  setOpenPath(null);
  const r = resolver; resolver = null;
  r?.();
}

function fmtDate(t: number): string {
  if (!t) return "—";
  const d = new Date(t * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v >= 100 ? 0 : v >= 10 ? 1 : 2)} ${units[i]} (${n.toLocaleString(intlLocale())} B)`;
}

function modeToOctal(mode: number): string {
  return (mode & 0o7777).toString(8).padStart(4, "0");
}

export function PropertiesDialog() {
  const [props, { refetch }] = createResource(openPath, async (p) => {
    if (!p) return null;
    return await getProperties(p);
  });

  const [octal, setOctal] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);

  createEffect(() => {
    const p = props();
    if (p) setOctal(modeToOctal(p.mode));
    else { setOctal(""); setErr(null); }
  });

  const apply = async () => {
    const p = props();
    if (!p) return;
    const m = parseInt(octal(), 8);
    if (isNaN(m) || m < 0 || m > 0o7777) {
      setErr(t("props.octalErr"));
      return;
    }
    setSaving(true);
    setErr(null);
    try {
      await setPermissions(p.path, m);
      await refetch();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Show when={openPath()}>
      <div
        class="modal-backdrop"
        onMouseDown={(ev) => { if (ev.target === ev.currentTarget) close(); }}
        onKeyDown={(ev) => { if (ev.key === "Escape") close(); }}
      >
        <div class="modal modal-props" onMouseDown={(ev) => ev.stopPropagation()}>
          <Show
            when={props()}
            fallback={<div class="props-loading">{t("common.loading")}</div>}
          >
            {(pAcc) => {
              const p = pAcc();
              return (
                <>
                  <h2>{t("props.title")}</h2>
                  <table class="props-table">
                    <tbody>
                      <tr><th>{t("props.name")}</th><td>{p.name}</td></tr>
                      <tr><th>{t("props.path")}</th><td class="mono">{p.path}</td></tr>
                      <tr><th>{t("props.kind")}</th><td>{p.kind}</td></tr>
                      <Show when={p.isSymlink && p.symlinkTarget}>
                        <tr><th>{t("props.symlinkTarget")}</th><td class="mono">{p.symlinkTarget}</td></tr>
                      </Show>
                      <tr><th>{t("props.size")}</th><td>{fmtSize(p.size)}</td></tr>
                      <Show when={p.isDir}>
                        <tr><th>{t("props.content")}</th><td>{t("props.contentCounts", { files: p.fileCount, dirs: p.dirCount })}</td></tr>
                      </Show>
                      <tr><th>{t("props.created")}</th><td class="mono">{fmtDate(p.btime)}</td></tr>
                      <tr><th>{t("props.modified")}</th><td class="mono">{fmtDate(p.mtime)}</td></tr>
                      <tr><th>{t("props.accessed")}</th><td class="mono">{fmtDate(p.atime)}</td></tr>
                      <tr><th>{t("props.owner")}</th><td class="mono">{p.owner} ({p.uid})</td></tr>
                      <tr><th>{t("props.group")}</th><td class="mono">{p.group} ({p.gid})</td></tr>
                      <tr>
                        <th>{t("props.perms")}</th>
                        <td class="mono">
                          {p.modeStr}{" "}
                          <input
                            class="props-octal"
                            type="text"
                            value={octal()}
                            maxLength={4}
                            onInput={(ev) => setOctal((ev.currentTarget as HTMLInputElement).value)}
                            onKeyDown={(ev) => { ev.stopPropagation(); if (ev.key === "Enter") void apply(); }}
                            title={t("props.octalHint")}
                          />
                          <button
                            class="secondary"
                            disabled={saving()}
                            onClick={() => void apply()}
                          >{t("common.apply")}</button>
                        </td>
                      </tr>
                      <Show when={err()}>
                        <tr><th></th><td class="props-error">{err()}</td></tr>
                      </Show>
                    </tbody>
                  </table>
                  <div class="modal-actions">
                    <button onClick={close}>{t("common.close")}</button>
                  </div>
                </>
              );
            }}
          </Show>
        </div>
      </div>
    </Show>
  );
}
