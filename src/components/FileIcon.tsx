import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import { readFileIcon } from "../ipc";

const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();
const queue: Array<() => void> = [];
let active = 0;
const MAX_CONCURRENT = 3;

function pump() {
  while (active < MAX_CONCURRENT && queue.length > 0) {
    const job = queue.shift()!;
    active++;
    job();
  }
}

function schedule<T>(fn: () => Promise<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    queue.push(() => {
      fn()
        .then(resolve, reject)
        .finally(() => {
          active--;
          pump();
        });
    });
    pump();
  });
}

function getIcon(path: string): Promise<string> {
  const c = cache.get(path);
  if (c) return Promise.resolve(c);
  const f = inflight.get(path);
  if (f) return f;
  const p = schedule(() => readFileIcon(path, 32))
    .then((url) => {
      cache.set(path, url);
      inflight.delete(path);
      return url;
    })
    .catch((e) => {
      inflight.delete(path);
      throw e;
    });
  inflight.set(path, p);
  return p;
}

export function FileIcon(props: { path: string; fallback: string }) {
  const [url, setUrl] = createSignal<string | null>(cache.get(props.path) ?? null);
  createEffect(() => {
    const path = props.path;
    const cached = cache.get(path);
    if (cached) {
      setUrl(cached);
      return;
    }
    setUrl(null);
    let cancelled = false;
    getIcon(path)
      .then((u) => {
        if (!cancelled) setUrl(u);
      })
      .catch(() => {
        if (!cancelled) setUrl(null);
      });
    onCleanup(() => {
      cancelled = true;
    });
  });
  return (
    <Show when={url()} fallback={<span class="icon">{props.fallback}</span>}>
      <img class="file-icon" src={url()!} alt="" draggable={false} />
    </Show>
  );
}
