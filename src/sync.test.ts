import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NfsSpec, RemoteMount } from "./ipc";
import type { SyncProfile } from "./syncProfiles";

const ipc = vi.hoisted(() => ({
  remoteMounts: vi.fn(),
  mountNfs: vi.fn(),
  listNetworkBookmarks: vi.fn(),
  loadRemotePassword: vi.fn(),
}));
const notifyError = vi.hoisted(() => vi.fn());
vi.mock("./ipc", () => ipc);
vi.mock("./state", () => ({ state: { job: null, active: "left" } }));
vi.mock("./components/Dialogs", () => ({ notifyError }));
vi.mock("./i18n", () => ({ errMsg: String }));

const nfs: NfsSpec = {
  host: "nas.local", path: "/exports/data", label: "NFS",
  version: "v3", security: "sys", transport: "tcp", realm: "",
  noLocks: true, allowInsecure: true,
};
const descriptor = "nfs://nas.local/exports/data";
const mount: RemoteMount = {
  path: "/Users/test/Library/Application Support/DualBeam/Remote/Volumes/NFS 2",
  label: "NFS 2", descriptor,
};

beforeEach(() => {
  vi.resetModules();
  vi.resetAllMocks();
  localStorage.clear();
  localStorage.setItem("dualbeam:nfs-profiles:v1", JSON.stringify([nfs]));
  ipc.listNetworkBookmarks.mockResolvedValue([]);
  ipc.remoteMounts.mockResolvedValue([]);
  ipc.mountNfs.mockImplementation(async () => {
    ipc.remoteMounts.mockResolvedValue([mount]);
    return mount.path;
  });
});

async function apply(side: "src" | "dst") {
  const profile: SyncProfile = {
    id: "nfs-sync", name: "NFS-Sync", src: "/local/source", dst: "/local/target",
    deleteExtra: false, ignorePatterns: "", mode: "oneWay",
    verifyChecksums: false, maxFileSizeMb: 0,
    remotePaths: { [side]: { descriptor, relativePath: "/projects/current" } },
  };
  profile[side] = "/old-mount/projects/current";
  localStorage.setItem("dualbeam:sync-profiles:v1", JSON.stringify([profile]));
  const sync = await import("./sync");
  const result = await sync.applySyncProfile(profile.id);
  const saved = JSON.parse(localStorage.getItem("dualbeam:sync-profiles:v1")!)[0];
  return { result, saved };
}

describe("NFS-Automount für Sync-Profile", () => {
  it.each(["src", "dst"] as const)("verbindet NFS als %s und erhält Einstellungen und Unterordner", async (side) => {
    const { result, saved } = await apply(side);
    expect(result).toBe(true);
    expect(ipc.mountNfs).toHaveBeenCalledExactlyOnceWith({ ...nfs, id: descriptor });
    expect(ipc.loadRemotePassword).not.toHaveBeenCalled();
    expect(saved[side]).toBe(`${mount.path}/projects/current`);
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("verwendet ein bereits verbundenes NFS-Laufwerk ohne erneuten Mount", async () => {
    ipc.remoteMounts.mockResolvedValue([mount]);
    expect((await apply("dst")).result).toBe(true);
    expect(ipc.mountNfs).not.toHaveBeenCalled();
  });

  it("bricht bei einem Mountfehler ab und erhält den gespeicherten Pfad", async () => {
    ipc.mountNfs.mockRejectedValue("NFS nicht erreichbar");
    const { result, saved } = await apply("dst");
    expect(result).toBe(false);
    expect(saved.dst).toBe("/old-mount/projects/current");
    expect(notifyError).toHaveBeenCalledWith(expect.stringContaining("NFS nicht erreichbar"));
  });
});
