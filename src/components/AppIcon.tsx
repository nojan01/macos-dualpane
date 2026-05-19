import { createSignal, onMount, Show } from "solid-js";
import { readImageThumb } from "../ipc";

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
      fn().then(resolve, reject).finally(() => {
        active--;
        pump();
      });
    });
    pump();
  });
}

async function getIcon(path: string): Promise<string> {
  const c = cache.get(path);
  if (c) return c;
  const f = inflight.get(path);
  if (f) return f;
  const p = schedule(() => readImageThumb(path, 32)).then((url) => {
    cache.set(path, url);
    inflight.delete(path);
    return url;
  }).catch((e) => {
    inflight.delete(path);
    throw e;
  });
  inflight.set(path, p);
  return p;
}

export function AppIcon(props: { path: string; fallback: string }) {
  const [url, setUrl] = createSignal<string | null>(cache.get(props.path) ?? null);
  onMount(() => {
    if (url()) return;
    getIcon(props.path).then(setUrl).catch(() => setUrl(null));
  });
  return (
    <Show when={url()} fallback={<>{props.fallback}</>}>
      <img class="app-icon" src={url()!} alt="" draggable={false} />
    </Show>
  );
}
