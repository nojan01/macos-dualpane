import { Show, createEffect, createSignal, on, onCleanup } from "solid-js";
import { state, selTick } from "../state";
import { previewInfo, readTextPreview, readImageThumb, type PreviewInfo } from "../ipc";
import { t, errMsg } from "../i18n";

function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${u[i]}`;
}

function formatTime(secs: number): string {
  if (!secs) return "—";
  const d = new Date(secs * 1000);
  return d.toLocaleString();
}

export function PreviewPane() {
  const [info, setInfo] = createSignal<PreviewInfo | null>(null);
  const [text, setText] = createSignal<string | null>(null);
  const [thumb, setThumb] = createSignal<string | null>(null);
  const [err, setErr] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);

  function selectedPath(): string | null {
    const p = state[state.active];
    if (!p.entries.length) return null;
    const e = p.entries[p.cursor];
    return e ? e.path : null;
  }

  let token = 0;

  createEffect(
    on(
      () => {
        selTick();
        return [state.active, state[state.active].cursor, selectedPath()] as const;
      },
      async () => {
        if (!state.previewVisible) return;
        const path = selectedPath();
        setText(null);
        setThumb(null);
        setErr(null);
        if (!path) {
          setInfo(null);
          return;
        }
        const my = ++token;
        setLoading(true);
        try {
          const i = await previewInfo(path);
          if (my !== token) return;
          setInfo(i);
          if (i.kind === "text") {
            const t = await readTextPreview(path, 65536);
            if (my !== token) return;
            setText(t);
          } else if (i.kind === "image") {
            try {
              const url = await readImageThumb(path, 256);
              if (my !== token) return;
              setThumb(url);
            } catch (e) {
              if (my !== token) return;
              setErr(errMsg(e));
            }
          }
        } catch (e) {
          if (my !== token) return;
          setErr(errMsg(e));
          setInfo(null);
        } finally {
          if (my === token) setLoading(false);
        }
      },
    ),
  );

  onCleanup(() => {
    token++;
  });

  return (
    <Show when={state.previewVisible}>
      <aside class="preview-pane">
        <div class="pv-header">{t("preview.title")}</div>
        <Show
          when={info()}
          fallback={<div class="pv-empty">{t("preview.none")}</div>}
        >
          {(i) => (
            <>
              <div class="pv-title" title={i().path}>{i().name}</div>
              <div class="pv-meta">
                <div>{i().isDir ? t("preview.folder") : i().kind}</div>
                <Show when={!i().isDir}><div>{formatSize(i().size)}</div></Show>
                <div>{formatTime(i().mtime)}</div>
              </div>
              <div class="pv-body">
                <Show when={loading()}><div class="pv-empty">{t("common.loading")}</div></Show>
                <Show when={err()}>
                  <div class="pv-error">{err()}</div>
                </Show>
                <Show when={i().kind === "image" && thumb()}>
                  <img class="pv-image" src={thumb()!} alt={i().name} />
                </Show>
                <Show when={i().kind === "text" && text() !== null}>
                  <pre class="pv-text">{text()}</pre>
                </Show>
                <Show when={i().kind === "binary" && !loading()}>
                  <div class="pv-empty">{t("preview.binary")}</div>
                </Show>
                <Show when={i().kind === "dir"}>
                  <div class="pv-empty">{t("preview.folder")}</div>
                </Show>
                <Show when={i().kind === "other" && !loading()}>
                  <div class="pv-empty">{t("preview.noPreview")}</div>
                </Show>
              </div>
            </>
          )}
        </Show>
      </aside>
    </Show>
  );
}
