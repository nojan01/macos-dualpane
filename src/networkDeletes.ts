import type { Entry } from "./types";

function key(path: string): string {
  return path.replace(/^\/System\/Volumes\/Data(?=\/)/, "").replace(/\/+$/, "") || "/";
}

function inside(path: string, deleted: string): boolean {
  return path === deleted || path.startsWith(`${deleted}/`);
}

type ListingSnapshot = { revision: number; paths: Set<string> };

/** Überbrückt ausschließlich vom Backend bestätigte Löschungen, solange der
 * Mount alte Einträge liefert. Kein dauerhaftes Ausblenden: Nach einer frischen
 * Abwesenheitsbestätigung oder spätestens einer Minute gilt wieder das Listing. */
export class NetworkDeletes {
  private revision = 0;
  private pending = new Map<string, number>();

  confirm(paths: string[]) {
    if (paths.length === 0) return;
    this.revision++;
    for (const path of paths) this.pending.set(key(path), Date.now() + 60_000);
  }

  beginListing(): ListingSnapshot {
    for (const [path, expires] of this.pending) {
      if (expires <= Date.now()) this.pending.delete(path);
    }
    return { revision: this.revision, paths: new Set(this.pending.keys()) };
  }

  visible(entries: Entry[], paths: Iterable<string> = this.pending.keys()): Entry[] {
    const deleted = [...paths];
    return entries.filter((entry) => !deleted.some((path) => inside(key(entry.path), path)));
  }

  reconcile(directory: string, entries: Entry[], snapshot: ListingSnapshot, showHidden: boolean): Entry[] | null {
    // Eine vor der Löschbestätigung gestartete Anfrage kann weder Anwesenheit
    // noch Abwesenheit verlässlich bestätigen. Der Aufrufer liest erneut.
    if (snapshot.revision !== this.revision) return null;
    const prefix = `${key(directory).replace(/\/$/, "")}/`;
    const listed = new Set(entries.map((entry) => key(entry.path)));
    for (const path of snapshot.paths) {
      const name = path.slice(prefix.length);
      if (path.startsWith(prefix) && !name.includes("/") &&
          (showHidden || !name.startsWith(".")) && !listed.has(path)) {
        this.pending.delete(path);
      }
    }
    // Auch wenn der andere Pane inzwischen ein frisches Listing erhalten hat,
    // gilt für diese bereits laufende Anfrage weiterhin ihr eigener Snapshot.
    return this.visible(entries, snapshot.paths);
  }
}
