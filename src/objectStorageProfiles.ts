import { createSignal } from "solid-js";
import {
  remoteMounts,
  type ObjectStorageDeleteTarget,
  type ObjectStorageTransferTarget,
  type JobItem,
} from "./ipc";

export type ObjectStorageProtocol = "s3" | "swift";
export type SwiftAuthVersion = "v3" | "v2";

/**
 * Verbindungsdaten ohne Geheimnis. Access-/Secret-Key bzw. Swift-Kennwort
 * bleiben im macOS-Schlüsselbund. Das Format entspricht absichtlich dem
 * bisherigen RemoteDeskRDP-Profil, damit vorhandene Zugänge ohne Neueingabe
 * übernommen werden können.
 */
export type ObjectStorageProfile = {
  id: string;
  name: string;
  protocol: ObjectStorageProtocol;
  endpoint: string;
  region: string;
  /** Optional: bei Leerwert werden alle Buckets bzw. Container eingehängt. */
  container: string;
  pathStyle: boolean;
  accessKey: string;
  username: string;
  swiftProject: string;
  swiftUserDomain: string;
  swiftProjectDomain: string;
  swiftIdentityPath: string;
  swiftAuthVersion: SwiftAuthVersion;
  /** Gleichzeitige Dateiübertragungen innerhalb eines rclone-Auftrags.
   * 1 ist besonders schonend, 4 nutzt bei Ordnerkopien die Leitung besser. */
  parallelTransfers: 1 | 4;
};

const KEY = "dualbeam:object-storage-profiles:v1";

export function emptyObjectStorageProfile(): ObjectStorageProfile {
  return {
    id: `object-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`,
    name: "",
    protocol: "s3",
    endpoint: "",
    region: "us-east-1",
    container: "",
    pathStyle: true,
    accessKey: "",
    username: "",
    swiftProject: "",
    swiftUserDomain: "Default",
    swiftProjectDomain: "Default",
    swiftIdentityPath: "/identity/v3",
    swiftAuthVersion: "v3",
    parallelTransfers: 1,
  };
}

function load(): ObjectStorageProfile[] {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    if (!Array.isArray(raw)) return [];
    return raw.flatMap((value): ObjectStorageProfile[] => {
      if (!value || typeof value !== "object") return [];
      const profile = value as Partial<ObjectStorageProfile>;
      if (
        typeof profile.id !== "string" ||
        typeof profile.name !== "string" ||
        (profile.protocol !== "s3" && profile.protocol !== "swift")
      ) return [];
      return [{
        ...emptyObjectStorageProfile(),
        ...profile,
        protocol: profile.protocol,
        pathStyle: profile.pathStyle !== false,
        swiftAuthVersion: profile.swiftAuthVersion === "v2" ? "v2" : "v3",
        parallelTransfers: profile.parallelTransfers === 4 ? 4 : 1,
      }];
    });
  } catch {
    return [];
  }
}

export const [objectStorageProfiles, setObjectStorageProfiles] =
  createSignal<ObjectStorageProfile[]>(load());

function persist(profiles: ObjectStorageProfile[]) {
  setObjectStorageProfiles(profiles);
  localStorage.setItem(KEY, JSON.stringify(profiles));
}

export function saveObjectStorageProfile(profile: ObjectStorageProfile) {
  const next = objectStorageProfiles().slice();
  const index = next.findIndex((item) => item.id === profile.id);
  if (index < 0) next.push(profile);
  else next[index] = profile;
  persist(next);
}

export function removeObjectStorageProfile(id: string) {
  persist(objectStorageProfiles().filter((profile) => profile.id !== id));
}

/** Fügt beim ersten Start die bestehenden RemoteDeskRDP-Profile hinzu. */
export function mergeObjectStorageProfiles(incoming: ObjectStorageProfile[]) {
  const existing = objectStorageProfiles();
  const known = new Set(existing.map((profile) => profile.id));
  const next = [...existing, ...incoming.filter((profile) => !known.has(profile.id))];
  if (next.length !== existing.length) persist(next);
}

/** Liefert die aktive Objekt-Speicher-Verbindung nur dann, wenn alle Pfade
 * derselben Einhängung unterhalb ihrer Wurzel liegen. Der Backend-Schnellpfad
 * kann so keinen Bucket/Container versehentlich an seiner Wurzel löschen. */
export async function objectStorageDeleteTarget(
  paths: string[],
  directoryPaths: string[] = [],
): Promise<ObjectStorageDeleteTarget | undefined> {
  if (paths.length === 0) return undefined;
  const mounts = await remoteMounts();
  for (const profile of objectStorageProfiles()) {
    const mount = mounts.find(
      (item) => item.descriptor === `${profile.protocol}://${profile.id}`,
    );
    if (!mount) continue;
    const prefix = `${mount.path}/`;
    if (paths.every((path) => path.startsWith(prefix))) {
      // NFS bildet Swift-/S3-Verzeichnisse als virtuelle Präfixe ab. Ihre
      // Dateisystem-Metadaten sind deshalb nicht immer verfügbar, während die
      // Pane sie beim Listing eindeutig als Ordner kennt.
      return { profile, mountPath: mount.path, directoryPaths };
    }
  }
  return undefined;
}

/** Erkennt eine Kopie von oder zu genau einem aktiven Objekt-Speicher-Mount.
 * Der Backend-Auftrag kann dann direkt rclone verwenden, statt die Daten über
 * den macOS-NFS-Adapter des Mounts zu schieben. */
export async function objectStorageTransferTarget(
  items: JobItem[],
): Promise<ObjectStorageTransferTarget | undefined> {
  if (items.length === 0) return undefined;
  const mounts = await remoteMounts();
  for (const profile of objectStorageProfiles()) {
    const mount = mounts.find(
      (item) => item.descriptor === `${profile.protocol}://${profile.id}`,
    );
    if (!mount) continue;
    const prefix = `${mount.path}/`;
    if (items.every((item) => item.src.startsWith(prefix))) {
      return { profile, mountPath: mount.path, sourceIsObjectStorage: true };
    }
    if (items.every((item) => item.dst.startsWith(prefix))) {
      return { profile, mountPath: mount.path, sourceIsObjectStorage: false };
    }
  }
  return undefined;
}
