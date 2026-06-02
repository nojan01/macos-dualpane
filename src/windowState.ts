import { getCurrentWindow, LogicalSize, LogicalPosition } from "@tauri-apps/api/window";

const KEY = "dualbeam:window:v1";

type WinState = { width: number; height: number; x: number | null; y: number | null };

function load(): WinState | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const p = JSON.parse(raw);
    if (typeof p?.width !== "number" || typeof p?.height !== "number") return null;
    if (p.width < 400 || p.height < 300 || p.width > 10000 || p.height > 10000) return null;
    return {
      width: p.width,
      height: p.height,
      x: typeof p.x === "number" ? p.x : null,
      y: typeof p.y === "number" ? p.y : null,
    };
  } catch {
    return null;
  }
}

let saveTimer: number | null = null;
function scheduleSave(s: WinState) {
  if (saveTimer != null) clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    try { localStorage.setItem(KEY, JSON.stringify(s)); } catch {}
  }, 300);
}

export async function attachWindowState() {
  const w = getCurrentWindow();
  const persisted = load();
  if (persisted) {
    try {
      await w.setSize(new LogicalSize(persisted.width, persisted.height));
      if (persisted.x != null && persisted.y != null) {
        await w.setPosition(new LogicalPosition(persisted.x, persisted.y));
      }
    } catch {
      // Fenstergeometrie konnte nicht wiederhergestellt werden – unkritisch.
    }
  }

  const dpr = () => window.devicePixelRatio || 1;
  const capture = async () => {
    try {
      const sz = await w.outerSize();
      const pos = await w.outerPosition();
      const d = dpr();
      scheduleSave({
        width: Math.round(sz.width / d),
        height: Math.round(sz.height / d),
        x: Math.round(pos.x / d),
        y: Math.round(pos.y / d),
      });
    } catch {
      // Fenstergröße nicht abrufbar – Speichern wird übersprungen.
    }
  };

  await w.onResized(() => { void capture(); });
  await w.onMoved(() => { void capture(); });
}
