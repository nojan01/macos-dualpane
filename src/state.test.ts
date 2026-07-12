import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Entry } from "./types";

const ipc = vi.hoisted(() => ({
  listDir: vi.fn(),
  pathExists: vi.fn(),
  pathIsNetwork: vi.fn(),
  homeDir: vi.fn(),
  watchPath: vi.fn(),
  unwatchPane: vi.fn(),
}));

vi.mock("./ipc", () => ipc);

import { _set, loadPane, state } from "./state";

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
});
