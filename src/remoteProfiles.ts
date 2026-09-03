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
 * nach einem Neustart ändern kann.
 *
 * Schema und Standardport müssen mit `RemoteProtocol::scheme` und
 * `RemoteProtocol::default_port` im Rust-Modul übereinstimmen. Weicht auch nur
 * eines davon ab, findet die Oberfläche ein bereits eingehängtes Laufwerk nicht
 * wieder: Das Zahnrad bliebe wirkungslos und erneutes Verbinden legte einen
 * zweiten Eintrag an, statt den bestehenden zu ersetzen. Die vollständige
 * Zuordnung erzwingt, dass ein neues Protokoll hier nicht vergessen wird. */
const SCHEME: Record<RemoteSpec["protocol"], string> = {
  sftp: "sftp",
  ftp: "ftp",
  ftpsExplicit: "ftps",
  ftpsImplicit: "ftps",
  smb: "smb",
};

const DEFAULT_PORT: Record<RemoteSpec["protocol"], number> = {
  sftp: 22,
  ftp: 21,
  ftpsExplicit: 21,
  ftpsImplicit: 990,
  smb: 445,
};

export function remoteDescriptor(spec: RemoteSpec): string {
  const port = spec.port ?? DEFAULT_PORT[spec.protocol];
  return `${SCHEME[spec.protocol]}://${spec.username}@${spec.host}:${port}`;
}

/** Alle Protokolle, abgeleitet aus `SCHEME`. Weil TypeScript diese Zuordnung
 * vollständig erzwingt, kann hier kein Protokoll fehlen. */
const PROTOCOLS = Object.keys(SCHEME) as RemoteSpec["protocol"][];

/** Kehrt `remoteDescriptor` um: gewinnt die Verbindungsdaten aus der Kennung
 * eines eingehängten Laufwerks zurück, zu dem es kein Lesezeichen gibt.
 *
 * Bewusst aus `SCHEME` und `DEFAULT_PORT` abgeleitet statt mit einer eigenen
 * Protokollliste geschrieben: Eine zweite Liste geriete beim Hinzufügen eines
 * Protokolls unweigerlich aus dem Tritt, und der Einstellungsdialog bliebe für
 * das neue Protokoll wirkungslos — ohne dass irgendetwas sich beschwert. */
export function remoteFromDescriptor(descriptor: string):
  | { protocol: RemoteSpec["protocol"]; username: string; host: string; port: number }
  | undefined {
  const found = /^([a-z]+):\/\/([^@]+)@(.+):(\d+)$/.exec(descriptor);
  if (!found) return undefined;
  const [, scheme, username, host, portText] = found;
  const port = Number(portText);
  const candidates = PROTOCOLS.filter((item) => SCHEME[item] === scheme);
  if (candidates.length === 0) return undefined;
  // „ftps“ steht für zwei Protokolle. Erst der Port trennt sie: 990 bedeutet
  // implizites TLS, 21 explizites. Ohne diese Unterscheidung öffnete das
  // Zahnrad an einem implizit gesicherten Laufwerk den Dialog mit dem falschen
  // Verfahren, und erneutes Verbinden liefe auf den falschen Port.
  const protocol = candidates.find((item) => DEFAULT_PORT[item] === port) ?? candidates[0];
  return { protocol, username, host, port };
}

function load(): RemoteProfile[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    if (!Array.isArray(value)) return [];
    return value.flatMap((item): RemoteProfile[] => {
      if (!item || typeof item !== "object") return [];
      const p = item as Partial<RemoteProfile>;
      if (typeof p.host !== "string" || typeof p.username !== "string" || typeof p.protocol !== "string") return [];
      // Bewusst aus `PROTOCOLS` geprüft statt gegen eine eigene Aufzählung:
      // Fehlte in einer solchen Liste ein Protokoll, würde sein Lesezeichen
      // zwar gespeichert, beim nächsten Start aber stillschweigend verworfen —
      // das Laufwerk wäre einfach verschwunden. Genau so ging SMB verloren.
      if (!(PROTOCOLS as string[]).includes(p.protocol)) return [];
      const spec: RemoteSpec = { protocol: p.protocol as RemoteSpec["protocol"], host: p.host, username: p.username, port: p.port ?? null, path: p.path ?? "", label: p.label ?? "", domain: p.domain ?? "" };
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
