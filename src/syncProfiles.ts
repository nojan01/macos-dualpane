import { createSignal } from "solid-js";

export type SyncProfile = {
  id: string;
  name: string;
  src: string;
  dst: string;
  deleteExtra: boolean;
  ignorePatterns: string;
  mode: "oneWay" | "twoWay";
  verifyChecksums: boolean;
};

const KEY = "dualbeam:sync-profiles:v1";

function load(): SyncProfile[] {
  try {
    const value = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    if (!Array.isArray(value)) return [];
    return value
      .filter(
        (profile): profile is SyncProfile =>
          profile &&
          typeof profile.id === "string" &&
          typeof profile.name === "string" &&
          typeof profile.src === "string" &&
          typeof profile.dst === "string",
      )
      .map((profile) => ({
        ...profile,
        deleteExtra: !!profile.deleteExtra,
        ignorePatterns:
          typeof profile.ignorePatterns === "string"
            ? profile.ignorePatterns
            : "",
        mode: profile.mode === "twoWay" ? "twoWay" : "oneWay",
        verifyChecksums: !!profile.verifyChecksums,
      }));
  } catch {
    return [];
  }
}

export const [syncProfiles, setSyncProfiles] =
  createSignal<SyncProfile[]>(load());

function persist(profiles: SyncProfile[]) {
  setSyncProfiles(profiles);
  try {
    localStorage.setItem(KEY, JSON.stringify(profiles));
  } catch {
    // Private mode or exhausted storage: keep the current session usable.
  }
}

export function saveSyncProfile(profile: SyncProfile) {
  const profiles = syncProfiles().slice();
  const index = profiles.findIndex((item) => item.id === profile.id);
  if (index >= 0) profiles[index] = profile;
  else profiles.push(profile);
  persist(profiles);
}

export function removeSyncProfile(id: string) {
  persist(syncProfiles().filter((profile) => profile.id !== id));
}

export function newSyncProfileId(): string {
  return `sync-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}
