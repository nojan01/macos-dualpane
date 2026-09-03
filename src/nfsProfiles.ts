import { createSignal } from "solid-js";
import type { NfsSecurity, NfsSpec, NfsTransport, NfsVersion } from "./ipc";

/** Dauerhafte NFS-Lesezeichen.
 *
 * NFS kennt kein Kennwort — bei AUTH_SYS meldet der Mac schlicht seine eigene
 * Benutzernummer, bei Kerberos übernimmt das Ticket die Anmeldung. Es liegt
 * hier also nichts Schutzwürdiges, anders als bei SFTP oder SMB, wo das
 * Kennwort ausschließlich im Schlüsselbund steht. */
export type NfsProfile = NfsSpec & { id: string };

const KEY = "dualbeam:nfs-profiles:v1";

/** Muss mit `NfsSpec::descriptor` im Rust-Modul übereinstimmen. Weicht die
 * Kennung ab, findet die Oberfläche ein bereits eingehängtes Laufwerk nicht
 * wieder: Das Lesezeichen erschiene als nicht verbunden, und ein zweiter
 * Verbindungsversuch legte einen doppelten Eintrag an. */
export function nfsDescriptor(spec: NfsSpec): string {
  return `nfs://${spec.host}${spec.path}`;
}

function idFor(spec: NfsSpec) {
  return nfsDescriptor(spec);
}

const VERSIONS: NfsVersion[] = ["auto", "v2", "v3", "v4", "v41"];
const SECURITIES: NfsSecurity[] = ["auto", "sys", "krb5", "krb5i", "krb5p"];
const TRANSPORTS: NfsTransport[] = ["auto", "tcp", "udp"];

/** Nimmt nur Werte an, die der Dialog auch anbieten kann. Ein von Hand
 * verfälschter oder aus einer älteren Fassung stammender Eintrag fällt damit
 * auf die Voreinstellung zurück, statt einen unbrauchbaren Einhängebefehl zu
 * erzeugen. */
function oneOf<T extends string>(allowed: T[], value: unknown, fallback: T): T {
  return typeof value === "string" && (allowed as string[]).includes(value) ? (value as T) : fallback;
}

function load(): NfsProfile[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    if (!Array.isArray(value)) return [];
    return value.flatMap((item): NfsProfile[] => {
      if (!item || typeof item !== "object") return [];
      const p = item as Partial<NfsProfile>;
      // Ohne Server und Freigabepfad lässt sich nichts einhängen. Der Pfad muss
      // mit „/“ beginnen, sonst ergäbe die Kennung eine unsinnige Zeichenkette.
      if (typeof p.host !== "string" || p.host.trim() === "") return [];
      if (typeof p.path !== "string" || !p.path.startsWith("/")) return [];
      const spec: NfsSpec = {
        host: p.host,
        path: p.path,
        version: oneOf(VERSIONS, p.version, "auto"),
        security: oneOf(SECURITIES, p.security, "auto"),
        realm: typeof p.realm === "string" ? p.realm : "",
        transport: oneOf(TRANSPORTS, p.transport, "auto"),
        noLocks: p.noLocks === true,
        label: typeof p.label === "string" ? p.label : "",
        allowInsecure: p.allowInsecure === true,
      };
      return [{ ...spec, id: idFor(spec) }];
    });
  } catch {
    return [];
  }
}

export const [nfsProfiles, setNfsProfiles] = createSignal<NfsProfile[]>(load());

export function saveNfsProfile(spec: NfsSpec) {
  const profile = { ...spec, id: idFor(spec) };
  // Dieselbe Freigabe erneut zu verbinden — etwa mit geänderter Fassung oder
  // anderem Sicherheitsverfahren — ersetzt das bisherige Lesezeichen, statt
  // einen zweiten, in der Seitenleiste nicht unterscheidbaren Eintrag anzulegen.
  const next = nfsProfiles().filter((item) => item.id !== profile.id);
  next.push(profile);
  setNfsProfiles(next);
  localStorage.setItem(KEY, JSON.stringify(next));
}

export function removeNfsProfile(id: string) {
  const next = nfsProfiles().filter((item) => item.id !== id);
  setNfsProfiles(next);
  localStorage.setItem(KEY, JSON.stringify(next));
}
