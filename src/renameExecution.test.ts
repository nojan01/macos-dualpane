import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./state", () => ({
  state: { job: null, left: { entries: [] } },
  setState: vi.fn(), refreshPane: vi.fn(async () => {}),
}));
vi.mock("./jobs", () => ({ selectedEntries: vi.fn() }));
vi.mock("./ipc", () => ({ renamePath: vi.fn(), pathExists: vi.fn(), samePathEntry: vi.fn(async () => false) }));
import { renamePath, pathExists, samePathEntry } from "./ipc";
import { applyRename, setRenameSession, setRenameOptions, defaultOpReplace } from "./rename";
import type { Entry } from "./types";

function entry(name: string): Entry {
  return { name, path: "/test/" + name, isDir: false, isSymlink: false,
    size: 0, mtime: 0, ext: "", hidden: false };
}

describe("rename rollback", () => {
  beforeEach(() => vi.clearAllMocks());

  it.each([1, 2, 3, 4, 5, 6])("restores all original names and contents when move %i fails", async (failAt) => {
    const items = [entry("2.txt"), entry("3.txt"), entry("1.txt")];
    const original = new Map(items.map(e => [e.path, "content of " + e.name]));
    const files = new Map(original);
    let calls = 0;
    vi.mocked(pathExists).mockImplementation(async path => files.has(path));
    vi.mocked(renamePath).mockImplementation(async (from, to) => {
      if (++calls === failAt) throw new Error("simulated failure");
      if (!files.has(from)) throw new Error("missing source");
      if (files.has(to)) throw new Error("destination exists");
      files.set(to, files.get(from)!);
      files.delete(from);
    });
    setRenameSession({ pane: "left", dir: "/test", items });
    setRenameOptions({ ops: [{ ...defaultOpReplace(), find: "^.+$", replace: "{n}.txt", regex: true }] });
    expect((await applyRename()).ok).toBe(false);
    expect(files).toEqual(original);
  });
});

it("renames case-only aliases through an exclusive temporary name", async () => {
  vi.mocked(pathExists).mockResolvedValue(true);
  vi.mocked(samePathEntry).mockResolvedValue(true);
  vi.mocked(renamePath).mockResolvedValue(undefined);
  vi.mocked(renamePath).mockClear();
  setRenameSession({ pane: "left", dir: "/test", items: [entry("file.txt")] });
  setRenameOptions({ ops: [{ kind: "case", scope: "base", caseMode: "upper" }] });
  expect((await applyRename()).ok).toBe(true);
  expect(renamePath).toHaveBeenCalledTimes(2);
  expect(vi.mocked(renamePath).mock.calls[1][1]).toBe("/test/FILE.txt");
});
