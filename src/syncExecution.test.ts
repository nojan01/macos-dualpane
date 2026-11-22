import { beforeEach, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
  remoteMounts: vi.fn(async () => []), listNetworkBookmarks: vi.fn(async () => []),
  syncPreview: vi.fn(), syncTwoWayPreview: vi.fn(), runJob: vi.fn(),
  pathIsNetwork: vi.fn(async () => false), moveToTrash: vi.fn(async () => {}),
}));
const model = vi.hoisted(() => ({ job: null as unknown, active: "left" }));
const notifyError = vi.hoisted(() => vi.fn());
vi.mock("./ipc", () => ipc);
vi.mock("./state", () => ({
  state: model,
  setState: vi.fn((field: string, value: unknown) => { if (field === "job") model.job = value; }),
  refreshPane: vi.fn(async () => {}),
}));
vi.mock("./components/Dialogs", () => ({ notifyError, askConfirm: vi.fn(), askPrompt: vi.fn() }));
vi.mock("./i18n", () => ({ t: (key: string) => key, errMsg: String }));
vi.mock("./remoteProfiles", () => ({
  remoteProfiles: () => [], remoteDescriptor: vi.fn(),
  remoteStorageTransferTarget: vi.fn(async () => undefined), remoteStorageDeleteTarget: vi.fn(),
}));
vi.mock("./objectStorageProfiles", () => ({
  objectStorageProfiles: () => [], objectStorageTransferTarget: vi.fn(async () => undefined),
  objectStorageDeleteTarget: vi.fn(),
}));

beforeEach(() => {
  vi.resetModules(); vi.clearAllMocks(); localStorage.clear(); model.job = null;
});

async function run(mode: "oneWay" | "twoWay") {
  localStorage.setItem("dualbeam:sync-profiles:v1", JSON.stringify([{
    id: "test", name: "test", src: "/source", dst: "/target", deleteExtra: true,
    ignorePatterns: "", mode, verifyChecksums: false, maxFileSizeMb: 0,
  }]));
  const sync = await import("./sync");
  await sync.runSyncProfile("test");
}

it("never starts deletion after cancelling the copy phase", async () => {
  ipc.syncPreview.mockResolvedValue([
    { rel: "new.txt", action: "copy", isDir: false, size: 1 },
    { rel: "old.txt", action: "delete", isDir: false, size: 1 },
  ]);
  ipc.runJob.mockRejectedValue("JOB_CANCELLED");
  await run("oneWay");
  expect(ipc.runJob).toHaveBeenCalledOnce();
  expect(ipc.moveToTrash).not.toHaveBeenCalled();
  expect(notifyError).not.toHaveBeenCalled(); expect(model.job).toBeNull();
});

it("never starts the reverse transfer after a two-way cancellation", async () => {
  ipc.syncTwoWayPreview.mockResolvedValue([
    { rel: "left.txt", action: "left_to_right", isDir: false, size: 1 },
    { rel: "right.txt", action: "right_to_left", isDir: false, size: 1 },
  ]);
  ipc.runJob.mockRejectedValue("JOB_CANCELLED");
  await run("twoWay");
  expect(ipc.runJob).toHaveBeenCalledOnce();
  expect(notifyError).not.toHaveBeenCalled(); expect(model.job).toBeNull();
});

it("sends directories as shallow operations, not recursive copies", async () => {
  ipc.syncPreview.mockResolvedValue([
    { rel: "folder", action: "copy", isDir: true, size: 0 },
    { rel: "folder/keep.txt", action: "copy", isDir: false, size: 1 },
  ]);
  ipc.runJob.mockResolvedValue(undefined);
  await run("oneWay");
  const items = ipc.runJob.mock.calls[0][2];
  expect(items[0].directoryOnly).toBe(true); expect(items[1].directoryOnly).toBe(false);
});
