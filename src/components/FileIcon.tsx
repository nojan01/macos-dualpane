import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import { readFileIcon } from "../ipc";

// Icons sind Data-URLs und damit vergleichsweise gross. Ohne Obergrenze
// wuechse der Cache beim Durchblaettern grosser Ordner unbegrenzt.
const ICON_CACHE_MAX = 2000;
const cache = new Map<string, string>();

/** Zuletzt genutzten Eintrag ans Ende schieben und den aeltesten verwerfen. */
function cacheGet(path: string): string | undefined {
  const hit = cache.get(path);
  if (hit === undefined) return undefined;
  cache.delete(path);
  cache.set(path, hit);
  return hit;
}

function cacheSet(path: string, url: string) {
  cache.set(path, url);
  while (cache.size > ICON_CACHE_MAX) {
    const oldest = cache.keys().next();
    if (oldest.done) break;
    cache.delete(oldest.value);
  }
}

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
  const c = cacheGet(path);
  if (c) return Promise.resolve(c);
  const f = inflight.get(path);
  if (f) return f;
  const p = schedule(() => readFileIcon(path, 32))
    .then((url) => {
      cacheSet(path, url);
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

export function FileIcon(props: {
  path: string;
  fallback: string;
  /** Virtuelle Einträge besitzen keine macOS-Datei und damit nur das generische weiße Finder-Symbol. */
  nativeIcon?: boolean;
}) {
  const [url, setUrl] = createSignal<string | null>(cacheGet(props.path) ?? null);
  createEffect(() => {
    const path = props.path;
    if (props.nativeIcon === false) {
      setUrl(null);
      return;
    }
    const cached = cacheGet(path);
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
