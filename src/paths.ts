// Gemeinsame Pfad-Hilfsfunktionen für Datei-Operationen.
import { pathExists } from "./ipc";

export function joinPath(dir: string, name: string): string {
  if (dir.endsWith("/")) return dir + name;
  return dir + "/" + name;
}

export function splitName(name: string): { base: string; ext: string } {
  const i = name.lastIndexOf(".");
  if (i <= 0 || i === name.length - 1) return { base: name, ext: "" };
  return { base: name.slice(0, i), ext: name.slice(i) };
}

export async function uniqueName(dir: string, name: string): Promise<string> {
  const { base, ext } = splitName(name);
  let candidate = `${base} copy${ext}`;
  let n = 2;
  while (await pathExists(joinPath(dir, candidate))) {
    candidate = `${base} copy ${n}${ext}`;
    n++;
  }
  return candidate;
}
