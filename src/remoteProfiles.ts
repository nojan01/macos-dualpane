import { createSignal } from "solid-js";
import {
  remoteMounts,
  type JobItem,
  type RemoteSpec,
  type RemoteStorageDeleteTarget,
  type RemoteStorageTransferTarget,
} from "./ipc";

/** Dauerhafte SFTP/FTP/FTPS-Lesezeichen – das Kennwort liegt ausschließlich
 * im Schlüsselbund und wird hier bewusst nie gespeichert. */
export type RemoteProfile = RemoteSpec & { id: string };
const KEY = "dualbeam:remote-profiles:v1";

function idFor(spec: RemoteSpec) {
  return [spec.protocol, spec.host.trim(), spec.port ?? "", spec.username.trim(), spec.path.trim()].join("|");
}

/** Entspricht der stabilen Kennung, die das Rust-Backend für einen aktiven
 * rclone-Mount liefert. Sie ist unabhängig vom lokalen Mount-Namen, der sich
 * nach einem Neustart ändern kann. */
export function remoteDescriptor(spec: RemoteSpec): string {
  const port = spec.port ?? (spec.protocol === "sftp" ? 22 : spec.protocol === "ftpsImplicit" ? 990 : 21);
  const scheme = spec.protocol === "sftp" ? "sftp" : spec.protocol === "ftp" ? "ftp" : "ftps";
  return `${scheme}://${spec.username}@${spec.host}:${port}`;
}

function load(): RemoteProfile[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    if (!Array.isArray(value)) return [];
    return value.flatMap((item): RemoteProfile[] => {
      if (!item || typeof item !== "object") return [];
      const p = item as Partial<RemoteProfile>;
      if (typeof p.host !== "string" || typeof p.username !== "string" || typeof p.protocol !== "string") return [];
      if (!(["sftp", "ftp", "ftpsExplicit", "ftpsImplicit"] as string[]).includes(p.protocol)) return [];
      const spec: RemoteSpec = { protocol: p.protocol as RemoteSpec["protocol"], host: p.host, username: p.username, port: p.port ?? null, path: p.path ?? "", label: p.label ?? "" };
      return [{ ...spec, id: idFor(spec) }];
    });
  } catch { return []; }
}

export const [remoteProfiles, setRemoteProfiles] = createSignal<RemoteProfile[]>(load());

export function saveRemoteProfile(spec: RemoteSpec) {
  const profile = { ...spec, id: idFor(spec) };
  // Der technische rclone-Mount ist pro Konto/Host eindeutig; sein
  // Descriptor enthält den Pfad nicht. Ein im Dialog geänderter Startordner
  // muss daher das bisherige Lesezeichen dieses Kontos ersetzen, statt einen
  // zweiten, nicht unterscheidbaren Eintrag in der Seitenleiste anzulegen.
  const next = remoteProfiles().filter(
    (item) => remoteDescriptor(item) !== remoteDescriptor(profile),
  );
  next.push(profile);
  setRemoteProfiles(next);
  localStorage.setItem(KEY, JSON.stringify(next));
}

export function removeRemoteProfile(id: string) {
  const next = remoteProfiles().filter((item) => item.id !== id);
  setRemoteProfiles(next);
  localStorage.setItem(KEY, JSON.stringify(next));
}

/** Erkennt den aktiven eigenen rclone-Mount zu allen Löschpfaden. Nur Pfade
 * unterhalb der Mount-Wurzel sind zulässig; das Backend erhält damit niemals
 * den Server-Stammordner als Purge-Ziel. */
export async function remoteStorageDeleteTarget(
  paths: string[],
): Promise<RemoteStorageDeleteTarget | undefined> {
  if (paths.length === 0) return undefined;
  const mounts = await remoteMounts();
  for (const profile of remoteProfiles()) {
    // SFTP ist ein echtes SSHFS-Dateisystem. Löschen läuft daher direkt über
    // dessen POSIX-Operationen und nicht über einen zweiten rclone-Weg.
    if (profile.protocol === "sftp") continue;
    const mount = mounts.find(
      (item) => item.descriptor === remoteDescriptor(profile),
    );
    if (!mount) continue;
    const prefix = `${mount.path}/`;
    if (paths.every((path) => path.startsWith(prefix))) {
      return { spec: profile, mountPath: mount.path };
    }
  }
  return undefined;
}

/** SFTP ist über SSHFS bereits ein normales Dateisystem. Es gibt deshalb
 * keinen zweiten Transferpfad mehr, der dieselbe Verbindung mit abweichenden
 * Caches oder Zugangsdaten ansprechen könnte. */
export async function remoteStorageTransferTarget(
  _items: JobItem[],
): Promise<RemoteStorageTransferTarget | undefined> {
  return undefined;
}
