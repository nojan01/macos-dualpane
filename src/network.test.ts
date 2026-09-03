import { describe, expect, it } from "vitest";
import { remoteFromUrl } from "./network";

describe("remoteFromUrl", () => {
  it("erkennt sftp und benutzt den Standardport, wenn keiner angegeben ist", () => {
    const result = remoteFromUrl(new URL("sftp://sftp.example.com/daten"));
    expect(result).toEqual({
      protocol: "sftp",
      host: "sftp.example.com",
      port: "",
      path: "/daten",
    });
  });

  it("übernimmt einen ausdrücklich angegebenen Port", () => {
    const result = remoteFromUrl(new URL("sftp://example.com:2222/"));
    expect(result?.port).toBe("2222");
  });

  it("behandelt ssh wie sftp, weil dahinter derselbe Dienst steckt", () => {
    expect(remoteFromUrl(new URL("ssh://example.com/"))?.protocol).toBe("sftp");
  });

  it("unterscheidet die beiden FTPS-Spielarten", () => {
    expect(remoteFromUrl(new URL("ftps://example.com/"))?.protocol).toBe(
      "ftpsImplicit",
    );
    expect(remoteFromUrl(new URL("ftpes://example.com/"))?.protocol).toBe(
      "ftpsExplicit",
    );
  });

  it("löst Prozentzeichen im Pfad auf", () => {
    expect(remoteFromUrl(new URL("sftp://example.com/mein%20ordner"))?.path).toBe(
      "/mein ordner",
    );
  });

  it("entfernt die Klammern einer IPv6-Adresse", () => {
    expect(remoteFromUrl(new URL("sftp://[fe80::1]/"))?.host).toBe("fe80::1");
  });

  it("erkennt SMB samt Freigabe – auch unter dem alten Namen cifs", () => {
    expect(remoteFromUrl(new URL("smb://nas.fritz.box/Daten"))).toEqual({
      protocol: "smb",
      host: "nas.fritz.box",
      port: "",
      path: "/Daten",
    });
    expect(remoteFromUrl(new URL("cifs://10.211.55.3/Daten"))?.protocol).toBe(
      "smb",
    );
  });

  it("führt WebDAV in den eigenen Dialog statt zum Finder", () => {
    // Früher landete jede https-Adresse beim Finder. Der fragte Benutzer und
    // Kennwort selbst ab; DualBeam konnte weder Anbieter noch Adresspfad
    // anbieten und legte kein Lesezeichen an.
    expect(remoteFromUrl(new URL("https://ewebdav.pcloud.com/"))).toEqual({
      protocol: "webdav", host: "ewebdav.pcloud.com", port: "", path: "", basePath: "",
    });
    // Der Pfad gehört bei Nextcloud zur Adresse, nicht zum Startordner.
    expect(
      remoteFromUrl(new URL("https://wolke.example.net:8443/remote.php/dav/files/norbert")),
    ).toEqual({
      protocol: "webdav", host: "wolke.example.net", port: "8443", path: "",
      basePath: "/remote.php/dav/files/norbert",
    });
    expect(remoteFromUrl(new URL("davs://dav.example.com/"))?.protocol).toBe("webdav");
  });

  it("lässt Protokolle unberührt, die macOS selbst einhängt", () => {
    for (const url of ["ftp://10.0.0.5/", "afp://10.0.0.5/"]) {
      expect(remoteFromUrl(new URL(url))).toBeNull();
    }
  });
});
