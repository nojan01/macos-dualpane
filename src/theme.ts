// Theme-Verwaltung: Auto (folgt macOS), Hell, Dunkel
// Persistiert in localStorage. Wendet [data-theme] auf <html> an.

export type ThemeMode = "auto" | "light" | "dark";
const KEY = "dualbeam:theme:v1";

let currentMode: ThemeMode = "auto";
const listeners = new Set<(m: ThemeMode, resolved: "light" | "dark") => void>();

function systemPrefersLight(): boolean {
  return typeof window !== "undefined"
    && window.matchMedia
    && window.matchMedia("(prefers-color-scheme: light)").matches;
}

function resolve(mode: ThemeMode): "light" | "dark" {
  if (mode === "auto") return systemPrefersLight() ? "light" : "dark";
  return mode;
}

function apply(mode: ThemeMode) {
  const resolved = resolve(mode);
  document.documentElement.dataset.theme = resolved;
  for (const l of listeners) l(mode, resolved);
}

export function initTheme() {
  try {
    const saved = localStorage.getItem(KEY) as ThemeMode | null;
    if (saved === "auto" || saved === "light" || saved === "dark") {
      currentMode = saved;
    }
  } catch {}
  apply(currentMode);

  if (window.matchMedia) {
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => { if (currentMode === "auto") apply(currentMode); };
    if (mq.addEventListener) mq.addEventListener("change", onChange);
    else if ((mq as any).addListener) (mq as any).addListener(onChange);
  }
}

export function getThemeMode(): ThemeMode { return currentMode; }

export function setThemeMode(mode: ThemeMode) {
  currentMode = mode;
  try { localStorage.setItem(KEY, mode); } catch {}
  apply(mode);
}

export function cycleThemeMode(): ThemeMode {
  const next: ThemeMode = currentMode === "auto" ? "light" : currentMode === "light" ? "dark" : "auto";
  setThemeMode(next);
  return next;
}

export function onThemeChange(cb: (mode: ThemeMode, resolved: "light" | "dark") => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function themeLabel(mode: ThemeMode): string {
  return mode === "auto" ? "Auto" : mode === "light" ? "Hell" : "Dunkel";
}

export function themeIcon(mode: ThemeMode): string {
  // Sun / Moon / Auto (half)
  return mode === "light" ? "☀︎" : mode === "dark" ? "☾" : "◐";
}
