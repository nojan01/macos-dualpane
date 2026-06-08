import { createSignal } from "solid-js";
import { de } from "./locale/de";
import { en } from "./locale/en";

export type LangMode = "auto" | "de" | "en";
export type ResolvedLang = "de" | "en";

const STORAGE_KEY = "dualbeam:lang:v1";

const dicts: Record<ResolvedLang, Record<string, string>> = { de, en };

function systemLang(): ResolvedLang {
  try {
    const l = (navigator.language || "en").toLowerCase();
    return l.startsWith("de") ? "de" : "en";
  } catch {
    return "en";
  }
}

function resolve(m: LangMode): ResolvedLang {
  return m === "auto" ? systemLang() : m;
}

function load(): LangMode {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "auto" || v === "de" || v === "en") return v;
  } catch {
    // localStorage nicht verfügbar – Standard "auto".
  }
  return "auto";
}

const [mode, setMode] = createSignal<LangMode>("auto");
const [resolved, setResolved] = createSignal<ResolvedLang>("en");
type Listener = (m: LangMode, r: ResolvedLang) => void;
const listeners = new Set<Listener>();

function apply(m: LangMode) {
  const r = resolve(m);
  setMode(m);
  setResolved(r);
  try {
    document.documentElement.lang = r;
  } catch {
    // DOM evtl. nicht bereit – unkritisch.
  }
  for (const l of listeners) l(m, r);
}

export function initI18n() {
  apply(load());
  try {
    // Wenn im Auto-Modus, Systemwechsel beobachten (selten, aber wir tun's)
    window.addEventListener("languagechange", () => {
      if (mode() === "auto") apply("auto");
    });
  } catch {
    // Kein window/Event-Support – Systemwechsel wird dann nicht beobachtet.
  }
}

export function getLangMode(): LangMode {
  return mode();
}

export function getResolvedLang(): ResolvedLang {
  return resolved();
}

export function setLangMode(m: LangMode) {
  try {
    localStorage.setItem(STORAGE_KEY, m);
  } catch {
    // Persistenz fehlgeschlagen – nicht kritisch.
  }
  apply(m);
}

export function cycleLangMode() {
  const order: LangMode[] = ["auto", "de", "en"];
  const next = order[(order.indexOf(mode()) + 1) % order.length];
  setLangMode(next);
}

export function onLangChange(cb: Listener): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function langLabel(m: LangMode): string {
  return m === "auto" ? "Auto" : m === "de" ? "DE" : "EN";
}

export function langIcon(m: LangMode): string {
  return m === "auto" ? "🌐" : m === "de" ? "DE" : "EN";
}

/** Reactive translation. Lies resolved() — Solid trackt das. */
export function t(key: string, params?: Record<string, string | number>): string {
  const r = resolved();
  const dict = dicts[r];
  let s = dict[key];
  if (s === undefined) s = en[key];
  if (s === undefined) s = key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.replace(new RegExp(`\\{${k}\\}`, "g"), String(v));
    }
  }
  return s;
}

/** Locale für Intl-APIs (Datumsformat etc.) */
export function intlLocale(): string {
  return resolved() === "de" ? "de-DE" : "en-US";
}

/** Wandelt einen unbekannten Fehlerwert (catch) in eine lesbare Meldung um.
 *  Backend-Fehler können als Code (`err.*`) kommen und werden dann lokalisiert. */
export function errMsg(e: unknown): string {
  let raw: string;
  if (e instanceof Error) raw = e.message;
  else if (typeof e === "string") raw = e;
  else if (e == null) return String(e);
  else {
    try {
      raw = typeof e === "object" ? JSON.stringify(e) : String(e);
    } catch {
      return String(e);
    }
  }
  return translateErr(raw);
}

/** Übersetzt einen Backend-Fehler-Code (`err.*`). Codes können Parameter
 *  tragen, getrennt durch das Unit-Separator-Zeichen (\x1f): `err.foo\x1farg0`.
 *  Im Übersetzungstext werden {0}, {1} … ersetzt. Unbekannte oder freie
 *  Texte werden unverändert zurückgegeben. */
export function translateErr(raw: string): string {
  const trimmed = raw.trim();
  const parts = trimmed.split("\x1f");
  const code = parts[0];
  if (code.startsWith("err.")) {
    const r = resolved();
    const dict = dicts[r];
    let msg = dict[code] ?? en[code];
    if (msg !== undefined) {
      for (let i = 1; i < parts.length; i++) {
        msg = msg.replace(new RegExp(`\\{${i - 1}\\}`, "g"), parts[i]);
      }
      return msg;
    }
  }
  return raw;
}
