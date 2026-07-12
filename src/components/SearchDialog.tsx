import { createSignal, For, Show, onCleanup } from "solid-js";
import { state, loadPane, selectOnly, setActive } from "../state";
import { searchInDir } from "../ipc";
import type { Entry, PaneId } from "../types";
import { t, errMsg } from "../i18n";

export const [searchOpen, setSearchOpen] = createSignal(false);

/** Maximale Anzahl Suchtreffer. */
const SEARCH_RESULT_LIMIT = 1000;
/** Verzögerung (ms) bis die Suche nach der Eingabe startet. */
const SEARCH_DEBOUNCE_MS = 250;

export function openSearch() {
  setSearchOpen(true);
}

function dirname(p: string): string {
  const i = p.lastIndexOf("/");
  if (i <= 0) return "/";
  return p.slice(0, i);
}

export function SearchDialog() {
  const [query, setQuery] = createSignal("");
  const [results, setResults] = createSignal<Entry[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [info, setInfo] = createSignal("");
  let inputEl: HTMLInputElement | undefined;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let request = 0;
  let resultPane: PaneId | null = null;

  const close = () => {
    request += 1;
    resultPane = null;
    setBusy(false);
    setSearchOpen(false);
    setQuery("");
    setResults([]);
    setInfo("");
  };

  const run = async () => {
    const requestId = ++request;
    const q = query().trim();
    if (q.length < 1) {
      setResults([]);
      setInfo("");
      return;
    }
    const pane = state.active;
    const root = state[pane].cwd;
    if (!root) return;
    setBusy(true);
    setInfo(t("search.searchingIn", { path: root }));
    try {
      const list = await searchInDir(
        root,
        q,
        state.showHidden,
        SEARCH_RESULT_LIMIT,
      );
      if (requestId !== request || !searchOpen()) return;
      resultPane = pane;
      setResults(list);
      setInfo(
        list.length >= SEARCH_RESULT_LIMIT
          ? t("search.hitsMax", { count: list.length })
          : t("search.hits", { count: list.length }),
      );
    } catch (e) {
      if (requestId !== request || !searchOpen()) return;
      setInfo(t("common.error", { msg: errMsg(e) }));
    } finally {
      if (requestId === request) setBusy(false);
    }
  };

  const onInput = (ev: InputEvent) => {
    setQuery((ev.currentTarget as HTMLInputElement).value);
    request += 1;
    resultPane = null;
    setBusy(false);
    setResults([]);
    setInfo("");
    if (timer) clearTimeout(timer);
    timer = setTimeout(run, SEARCH_DEBOUNCE_MS);
  };

  const reveal = async (e: Entry) => {
    const pane = resultPane ?? state.active;
    setActive(pane);
    const parent = dirname(e.path);
    await loadPane(pane, parent);
    const idx = state[pane].entries.findIndex((x) => x.path === e.path);
    if (idx >= 0) selectOnly(pane, idx);
    close();
  };

  const onKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      close();
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      const first = results()[0];
      if (first) void reveal(first);
    }
  };

  // Auto-focus beim Mount via ref
  setTimeout(() => inputEl?.focus(), 0);

  onCleanup(() => {
    request += 1;
    if (timer) clearTimeout(timer);
  });

  return (
    <Show when={searchOpen()}>
      <div class="search-overlay" onClick={close}>
        <div class="search-dialog" onClick={(e) => e.stopPropagation()}>
          <div class="search-header">
            <span>
              {t("search.title", { path: state[state.active].cwd || "—" })}
            </span>
            <button class="search-close" onClick={close}>
              ×
            </button>
          </div>
          <input
            ref={inputEl}
            class="search-input"
            type="text"
            placeholder={t("search.placeholder")}
            value={query()}
            onInput={onInput}
            onKeyDown={onKey}
          />
          <div class="search-info">
            {busy() ? t("search.searching") : info()}
          </div>
          <div class="search-results">
            <For each={results()}>
              {(e) => (
                <div class="search-row" onClick={() => void reveal(e)}>
                  <span class="search-name">
                    {e.isDir ? "📁 " : "📄 "}
                    {e.name}
                  </span>
                  <span class="search-path">{dirname(e.path)}</span>
                </div>
              )}
            </For>
          </div>
        </div>
      </div>
    </Show>
  );
}
