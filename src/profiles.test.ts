import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NfsSpec, RemoteSpec } from "./ipc";

/** Beide Module lesen ihren Bestand einmalig beim Laden aus dem lokalen
 * Speicher. Ein Neustart des Programms lässt sich deshalb nur nachbilden,
 * indem die Modulverwaltung zurückgesetzt und danach erneut geladen wird. */
beforeEach(() => {
  localStorage.clear();
  vi.resetModules();
});

const remote = () => import("./remoteProfiles");
const nfsMod = () => import("./nfsProfiles");
const objMod = () => import("./objectStorageProfiles");

const smb: RemoteSpec = {
  protocol: "smb",
  host: "server.example.com",
  port: null,
  username: "norbert",
  path: "/freigabe",
  label: "Arbeit",
  domain: "WERKSTATT",
};

describe("remoteDescriptor", () => {
  it("bildet SMB auf Schema und Standardport des Rust-Moduls ab", async () => {
    const { remoteDescriptor } = await remote();
    // Muss `RemoteProtocol::scheme` und `default_port` entsprechen. Wich die
    // Kennung ab, fände die Oberfläche ein eingehängtes SMB-Laufwerk nie wieder.
    expect(remoteDescriptor(smb)).toBe("smb://norbert@server.example.com:445");
  });

  it("lässt die übrigen Protokolle unverändert", async () => {
    const { remoteDescriptor } = await remote();
    const base = { host: "h", port: null, username: "u", path: "", label: "" };
    expect(remoteDescriptor({ ...base, protocol: "sftp" })).toBe("sftp://u@h:22");
    expect(remoteDescriptor({ ...base, protocol: "ftp" })).toBe("ftp://u@h:21");
    expect(remoteDescriptor({ ...base, protocol: "ftpsExplicit" })).toBe("ftps://u@h:21");
    expect(remoteDescriptor({ ...base, protocol: "ftpsImplicit" })).toBe("ftps://u@h:990");
  });

  it("bevorzugt einen ausdrücklich gesetzten Port", async () => {
    const { remoteDescriptor } = await remote();
    expect(remoteDescriptor({ ...smb, port: 4450 })).toBe("smb://norbert@server.example.com:4450");
  });
});

/** Die vollständige Zuordnung zwingt TypeScript, jedes Protokoll aufzuführen.
 * Käme eines hinzu, ohne hier ergänzt zu werden, schlüge bereits die
 * Übersetzung fehl — eine bloße Aufzählung ließe die Lücke durchgehen. */
const PROTOKOLL_TAFEL: Record<RemoteSpec["protocol"], true> = {
  sftp: true,
  ftp: true,
  ftpsExplicit: true,
  ftpsImplicit: true,
  smb: true,
  webdav: true,
};
const ALLE_PROTOKOLLE = Object.keys(PROTOKOLL_TAFEL) as RemoteSpec["protocol"][];

describe("WebDAV-Profile", () => {
  it("führt Anbieter und Adresspfad über einen Neustart hinweg mit", async () => {
    const { saveRemoteProfile } = await remote();
    saveRemoteProfile({
      protocol: "webdav", host: "wolke.example.net", port: null,
      username: "norbert", path: "", label: "Wolke",
      basePath: "/remote.php/dav/files/norbert", vendor: "nextcloud",
    });
    // Neustart nachbilden: Modul neu laden, damit erneut aus dem Speicher
    // gelesen wird. Fehlte eines der beiden Felder beim Laden, ginge die
    // Adresse verloren und die Verbindung liefe auf die nackte Wurzel.
    vi.resetModules();
    const { remoteProfiles } = await remote();
    expect(remoteProfiles().find((p) => p.host === "wolke.example.net")).toMatchObject({
      vendor: "nextcloud", basePath: "/remote.php/dav/files/norbert",
    });
  });

  it("ersetzt dasselbe Konto, statt einen zweiten Eintrag anzulegen", async () => {
    const { remoteProfiles, saveRemoteProfile } = await remote();
    const base: RemoteSpec = {
      protocol: "webdav", host: "wolke.example.net", port: null,
      username: "norbert", path: "", label: "",
    };
    // Lesezeichen werden über „schema://benutzer@rechner:port“ unterschieden,
    // nicht über den Adresspfad. Ein im Dialog geänderter Adresspfad muss den
    // bisherigen Eintrag ersetzen — sonst stünde dasselbe Konto doppelt da.
    saveRemoteProfile({ ...base, basePath: "/remote.php/dav/files/norbert" });
    saveRemoteProfile({ ...base, basePath: "/remote.php/dav/files/neu" });
    const gefunden = remoteProfiles().filter((p) => p.protocol === "webdav");
    expect(gefunden).toHaveLength(1);
    expect(gefunden[0].basePath).toBe("/remote.php/dav/files/neu");
  });
});

describe("remoteFromDescriptor", () => {
  it("erkennt implizites FTPS am Port statt es für explizites zu halten", async () => {
    const { remoteFromDescriptor } = await remote();
    // Beide teilen sich das Schema „ftps“. Zuvor lieferte die Zerlegung immer
    // „ftpsExplicit“: Das Zahnrad zeigte das falsche Verfahren, und erneutes
    // Verbinden nahm Port 21 statt 990.
    expect(remoteFromDescriptor("ftps://u@h:990")).toEqual({
      protocol: "ftpsImplicit", username: "u", host: "h", port: 990,
    });
    expect(remoteFromDescriptor("ftps://u@h:21")).toEqual({
      protocol: "ftpsExplicit", username: "u", host: "h", port: 21,
    });
  });

  it("kehrt remoteDescriptor für jedes Protokoll um", async () => {
    const { remoteDescriptor, remoteFromDescriptor } = await remote();
    for (const protocol of ALLE_PROTOKOLLE) {
      const spec: RemoteSpec = { protocol, host: "beispiel.de", port: null, username: "norbert", path: "", label: "" };
      expect(remoteFromDescriptor(remoteDescriptor(spec))).toMatchObject({
        protocol, username: "norbert", host: "beispiel.de",
      });
    }
  });

  it("behält einen abweichenden Port bei", async () => {
    const { remoteDescriptor, remoteFromDescriptor } = await remote();
    const spec: RemoteSpec = { protocol: "sftp", host: "h", port: 2222, username: "u", path: "", label: "" };
    expect(remoteFromDescriptor(remoteDescriptor(spec))).toMatchObject({ protocol: "sftp", port: 2222 });
  });

  it("lehnt fremde und unvollständige Kennungen ab", async () => {
    const { remoteFromDescriptor } = await remote();
    expect(remoteFromDescriptor("nfs://server/freigabe")).toBeUndefined();
    expect(remoteFromDescriptor("gopher://u@h:70")).toBeUndefined();
    expect(remoteFromDescriptor("sftp://h:22")).toBeUndefined();
    expect(remoteFromDescriptor("kein-schema")).toBeUndefined();
  });
});

describe("SMB-Lesezeichen", () => {
  it("übersteht einen Neustart", async () => {
    const { saveRemoteProfile } = await remote();
    saveRemoteProfile(smb);
    // Zuvor warf die Ladeprüfung SMB stillschweigend weg: Das Lesezeichen war
    // beim nächsten Start einfach fort.
    vi.resetModules();
    const { remoteProfiles } = await remote();
    const found = remoteProfiles();
    expect(found).toHaveLength(1);
    expect(found[0].protocol).toBe("smb");
    expect(found[0].domain).toBe("WERKSTATT");
  });
});

const nfs: NfsSpec = {
  host: "192.168.8.103",
  path: "/srv/samba/mac-test",
  version: "v41",
  security: "krb5p",
  realm: "BEISPIEL.DE",
  transport: "tcp",
  noLocks: true,
  label: "Nfs-Test",
  allowInsecure: true,
};

describe("nfsDescriptor", () => {
  it("entspricht der Kennung des Rust-Moduls", async () => {
    const { nfsDescriptor } = await nfsMod();
    expect(nfsDescriptor(nfs)).toBe("nfs://192.168.8.103/srv/samba/mac-test");
  });
});

describe("NFS-Lesezeichen", () => {
  it("bewahrt sämtliche Einstellungen über einen Neustart hinweg", async () => {
    const { saveNfsProfile } = await nfsMod();
    saveNfsProfile(nfs);
    vi.resetModules();
    const { nfsProfiles } = await nfsMod();
    const found = nfsProfiles();
    expect(found).toHaveLength(1);
    expect(found[0]).toMatchObject(nfs);
  });

  it("ersetzt dieselbe Freigabe, statt sie zu verdoppeln", async () => {
    const { saveNfsProfile, nfsProfiles } = await nfsMod();
    saveNfsProfile(nfs);
    saveNfsProfile({ ...nfs, version: "v3", label: "Neu" });
    const found = nfsProfiles();
    expect(found).toHaveLength(1);
    expect(found[0].version).toBe("v3");
    expect(found[0].label).toBe("Neu");
  });

  it("verwirft Einträge ohne Server oder mit unbrauchbarem Pfad", async () => {
    localStorage.setItem(
      "dualbeam:nfs-profiles:v1",
      JSON.stringify([
        { ...nfs, host: "" },
        { ...nfs, path: "ohne-schrägstrich" },
      ]),
    );
    const { nfsProfiles } = await nfsMod();
    expect(nfsProfiles()).toHaveLength(0);
  });

  it("setzt unbekannte Werte auf die Voreinstellung zurück", async () => {
    localStorage.setItem(
      "dualbeam:nfs-profiles:v1",
      JSON.stringify([{ ...nfs, version: "v99", security: "irgendwas", transport: "sctp" }]),
    );
    const { nfsProfiles } = await nfsMod();
    const found = nfsProfiles();
    expect(found).toHaveLength(1);
    expect(found[0].version).toBe("auto");
    expect(found[0].security).toBe("auto");
    expect(found[0].transport).toBe("auto");
  });

  it("entfernt ein Lesezeichen dauerhaft", async () => {
    const { saveNfsProfile, removeNfsProfile, nfsDescriptor } = await nfsMod();
    saveNfsProfile(nfs);
    removeNfsProfile(nfsDescriptor(nfs));
    vi.resetModules();
    const { nfsProfiles } = await nfsMod();
    expect(nfsProfiles()).toHaveLength(0);
  });
});

describe("Objekt-Speicher-Lesezeichen", () => {
  it("bewahrt S3 und Swift über einen Neustart hinweg", async () => {
    const { emptyObjectStorageProfile, saveObjectStorageProfile } = await objMod();
    const s3 = { ...emptyObjectStorageProfile(), id: "s3-1", name: "Sicherung", protocol: "s3" as const, endpoint: "https://s3.example.com", region: "eu-central-1", container: "eimer", accessKey: "AKIA", parallelTransfers: 4 as const };
    const swift = { ...emptyObjectStorageProfile(), id: "swift-1", name: "Wolke", protocol: "swift" as const, endpoint: "https://auth.example.com", username: "norbert", swiftProject: "Projekt", swiftAuthVersion: "v2" as const };
    saveObjectStorageProfile(s3);
    saveObjectStorageProfile(swift);
    vi.resetModules();
    const { objectStorageProfiles } = await objMod();
    const found = objectStorageProfiles();
    expect(found).toHaveLength(2);
    expect(found.find((p) => p.id === "s3-1")).toMatchObject(s3);
    expect(found.find((p) => p.id === "swift-1")).toMatchObject(swift);
  });

  it("ersetzt dasselbe Profil, statt es zu verdoppeln", async () => {
    const { emptyObjectStorageProfile, saveObjectStorageProfile, objectStorageProfiles } = await objMod();
    const base = { ...emptyObjectStorageProfile(), id: "s3-1", name: "Alt" };
    saveObjectStorageProfile(base);
    saveObjectStorageProfile({ ...base, name: "Neu" });
    expect(objectStorageProfiles()).toHaveLength(1);
    expect(objectStorageProfiles()[0].name).toBe("Neu");
  });

  it("verwirft Einträge mit unbekanntem Protokoll", async () => {
    localStorage.setItem(
      "dualbeam:object-storage-profiles:v1",
      JSON.stringify([{ id: "x", name: "Kaputt", protocol: "gopher" }]),
    );
    const { objectStorageProfiles } = await objMod();
    expect(objectStorageProfiles()).toHaveLength(0);
  });
});
