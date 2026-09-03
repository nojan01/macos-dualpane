import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Entry } from "./types";

const ipc = vi.hoisted(() => ({
  listDir: vi.fn(),
  pathExists: vi.fn(),
  pathIsNetwork: vi.fn(),
  navigationRoot: vi.fn(),
  homeDir: vi.fn(),
  watchPath: vi.fn(),
  unwatchPane: vi.fn(),
}));

vi.mock("./ipc", () => ipc);

import {
  _set,
  canNavigateUp,
  confirmDeletedNetworkPaths,
  followFrom,
  loadPane,
  selectOnly,
  state,
  toggleFollowMode,
} from "./state";

function entry(path: string): Entry {
  const name = path.split("/").pop() || path;
  return {
    name,
    path,
    isDir: false,
    isSymlink: false,
    size: 1,
    mtime: 0,
    ext: "",
    hidden: false,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

describe("loadPane", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipc.pathExists.mockResolvedValue(true);
    ipc.pathIsNetwork.mockResolvedValue(false);
    ipc.navigationRoot.mockResolvedValue(null);
    ipc.homeDir.mockResolvedValue("/Users/test");
    ipc.watchPath.mockResolvedValue(undefined);
    _set("showHidden", false);
    _set("left", {
      cwd: "",
      history: [],
      historyIndex: -1,
      entriesRaw: [],
      entries: [],
      cursor: 0,
      selected: new Set(),
      anchor: null,
      loading: false,
      error: null,
      sortKey: "name",
      sortDir: "asc",
      filter: "",
    });
  });

  it("verwirft das Ergebnis einer älteren, langsameren Navigation", async () => {
    const slow = deferred<Entry[]>();
    ipc.listDir.mockImplementation((path: string) =>
      path === "/slow" ? slow.promise : Promise.resolve([entry("/fast/file.txt")]),
    );

    const first = loadPane("left", "/slow");
    await Promise.resolve();
    await Promise.resolve();
    const second = loadPane("left", "/fast");
    await second;

    slow.resolve([entry("/slow/old.txt")]);
    await first;

    expect(state.left.cwd).toBe("/fast");
    expect(state.left.entries[0]?.path).toBe("/fast/file.txt");
  });

  it("entfernt bestätigte WebDAV-Löschungen sofort aus beiden Panes und ihrer Auswahl", async () => {
    const deleted = entry("/pcloud-immediate/delete.txt");
    const kept = entry("/pcloud-immediate/keep.txt");
    ipc.listDir.mockResolvedValue([deleted, kept]);
    await loadPane("left", "/pcloud-immediate");
    await loadPane("right", "/pcloud-immediate");
    for (const pane of ["left", "right"] as const) {
      _set(pane, "selected", new Set([deleted.path, kept.path]));
      _set(pane, "cursor", 1);
      _set(pane, "anchor", 1);
    }
    confirmDeletedNetworkPaths([deleted.path]);
    for (const pane of ["left", "right"] as const) {
      expect(state[pane].entriesRaw).toEqual([kept]);
      expect(state[pane].entries).toEqual([kept]);
      expect(state[pane].selected).toEqual(new Set([kept.path]));
      expect(state[pane].cursor).toBe(0);
      expect(state[pane].anchor).toBe(0);
    }
    await loadPane("left", "/pcloud-immediate");
    expect(state.left.entries).toEqual([kept]);
  });

  it("verwirft ein vor der WebDAV-Löschung gestartetes Listing und liest erneut", async () => {
    const deleted = entry("/pcloud-race/delete.txt");
    const kept = entry("/pcloud-race/keep.txt");
    ipc.pathIsNetwork.mockResolvedValue(true);
    const slow = deferred<Entry[]>();
    ipc.listDir.mockReturnValueOnce(slow.promise).mockResolvedValue([deleted, kept]);
    const loading = loadPane("left", "/pcloud-race");
    await vi.waitFor(() => expect(ipc.listDir).toHaveBeenCalledTimes(1));
    confirmDeletedNetworkPaths([deleted.path]);
    slow.resolve([deleted, kept]);
    await loading;
    expect(ipc.listDir).toHaveBeenCalledTimes(2);
    expect(state.left.entries).toEqual([kept]);
    expect(state.left.loading).toBe(false);
  });

  it("begrenzt die Aufwärtsnavigation an der sichtbaren Mountwurzel", () => {
    _set("left", "cwd", "/private/remote/Datensicherung");
    _set("left", "navigationRoot", "/private/remote/Datensicherung");
    expect(canNavigateUp("left")).toBe(false);

    _set("left", "cwd", "/private/remote/Datensicherung/Dokumente");
    expect(canNavigateUp("left")).toBe(true);
  });

  it("erkennt die S3-Mountwurzel auch nach der macOS-Pfadnormalisierung", () => {
    _set("left", "cwd", "/System/Volumes/Data/Users/nojan/Library/Application Support/DualBeam/Remote/Volumes/S3 DS");
    _set("left", "navigationRoot", "/Users/nojan/Library/Application Support/DualBeam/Remote/Volumes/S3 DS");
    expect(canNavigateUp("left")).toBe(false);
  });
});

describe("Folgemodus (followMode)", () => {
  function dirEntry(path: string): Entry {
    return { ...entry(path), isDir: true };
  }

  function resetPane(pane: "left" | "right", entries: Entry[]) {
    _set(pane, {
      cwd: pane === "left" ? "/left" : "",
      history: [],
      historyIndex: -1,
      entriesRaw: entries,
      entries,
      cursor: 0,
      selected: new Set(),
      anchor: null,
      loading: false,
      error: null,
      sortKey: "name",
      sortDir: "asc",
      filter: "",
    });
  }

  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
    ipc.pathExists.mockResolvedValue(true);
    ipc.pathIsNetwork.mockResolvedValue(false);
    ipc.homeDir.mockResolvedValue("/Users/test");
    ipc.watchPath.mockResolvedValue(undefined);
    ipc.listDir.mockResolvedValue([]);
    _set("active", "left");
    _set("followMode", false);
    resetPane("left", [dirEntry("/left/sub")]);
    resetPane("right", []);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("spiegelt einen Ordner in den anderen Pane", async () => {
    _set("followMode", true);
    followFrom("left");
    await vi.advanceTimersByTimeAsync(80);
    expect(ipc.listDir).toHaveBeenCalledWith("/left/sub", false);
    expect(state.right.cwd).toBe("/left/sub");
  });

  it("tut nichts, wenn der Folgemodus aus ist", async () => {
    followFrom("left");
    await vi.advanceTimersByTimeAsync(80);
    expect(ipc.listDir).not.toHaveBeenCalled();
    expect(state.right.cwd).toBe("");
  });

  it("folgt keiner Datei", async () => {
    resetPane("left", [entry("/left/file.txt")]);
    _set("followMode", true);
    followFrom("left");
    await vi.advanceTimersByTimeAsync(80);
    expect(ipc.listDir).not.toHaveBeenCalled();
  });

  it("wird über selectOnly ausgelöst (Klick/Cursor)", async () => {
    _set("followMode", true);
    selectOnly("left", 0);
    await vi.advanceTimersByTimeAsync(80);
    expect(state.right.cwd).toBe("/left/sub");
  });

  it("spiegelt beim Einschalten sofort den markierten Ordner", async () => {
    toggleFollowMode();
    await vi.advanceTimersByTimeAsync(80);
    expect(state.right.cwd).toBe("/left/sub");
  });
});
