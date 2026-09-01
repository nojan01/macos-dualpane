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
  /** Dateisystem nutzt das eingebundene Ziel direkt. rsync überträgt per SSH. */
  transport: "filesystem" | "rsync";
  /**
   * Obergrenze je Datei in MB. 0 bedeutet „keine Grenze“. Sehr große Dateien
   * (Datenträgerabbilder, Videoarchive) blockieren auf langsamen Zielen sonst
   * den gesamten Abgleich.
   */
  maxFileSizeMb: number;
  /** Zugangsdaten ohne Passwort; dieses liegt ausschließlich im Schlüsselbund. */
  rsync?: {
    host: string;
    username: string;
    remotePath: string;
  };
  /** Stabile Zuordnung für rclone-Laufwerke. Der lokale Mount-Pfad ist nur
   * eine Momentaufnahme und kann nach einem Neustart anders heißen. */
  remotePaths?: {
    src?: { descriptor: string; relativePath: string };
    dst?: { descriptor: string; relativePath: string };
  };
  /** Stabile Zuordnung für macOS-Netzlaufwerke (WebDAV, SMB usw.). */
  networkPaths?: {
    src?: { url: string; relativePath: string };
    dst?: { url: string; relativePath: string };
  };
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
        transport: profile.transport === "rsync" ? "rsync" : "filesystem",
        // Bestandsprofile kennen das Feld nicht; ebenso wenig sind negative,
        // gebrochene oder unendliche Werte sinnvoll. Alles Ungültige bedeutet
        // „keine Grenze“, damit ein beschädigter Eintrag nie stillschweigend
        // Dateien vom Abgleich ausschließt.
        maxFileSizeMb:
          typeof profile.maxFileSizeMb === "number" &&
          Number.isFinite(profile.maxFileSizeMb) &&
          profile.maxFileSizeMb > 0
            ? Math.floor(profile.maxFileSizeMb)
            : 0,
        rsync:
          profile.rsync &&
          typeof profile.rsync.host === "string" &&
          typeof profile.rsync.username === "string" &&
          typeof profile.rsync.remotePath === "string"
            ? {
                host: profile.rsync.host,
                username: profile.rsync.username,
                remotePath: profile.rsync.remotePath,
              }
            : undefined,
        remotePaths:
          profile.remotePaths && typeof profile.remotePaths === "object"
            ? {
                src:
                  profile.remotePaths.src &&
                  typeof profile.remotePaths.src.descriptor === "string" &&
                  typeof profile.remotePaths.src.relativePath === "string"
                    ? profile.remotePaths.src
                    : undefined,
                dst:
                  profile.remotePaths.dst &&
                  typeof profile.remotePaths.dst.descriptor === "string" &&
                  typeof profile.remotePaths.dst.relativePath === "string"
                    ? profile.remotePaths.dst
                    : undefined,
              }
            : undefined,
        networkPaths:
          profile.networkPaths && typeof profile.networkPaths === "object"
            ? {
                src:
                  profile.networkPaths.src &&
                  typeof profile.networkPaths.src.url === "string" &&
                  typeof profile.networkPaths.src.relativePath === "string"
                    ? profile.networkPaths.src
                    : undefined,
                dst:
                  profile.networkPaths.dst &&
                  typeof profile.networkPaths.dst.url === "string" &&
                  typeof profile.networkPaths.dst.relativePath === "string"
                    ? profile.networkPaths.dst
                    : undefined,
              }
            : undefined,
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
