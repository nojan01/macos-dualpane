import { state, loadPane, bumpVolumes } from "./state";
import { mountNetworkUrl } from "./ipc";
import { askPrompt, askConfirm } from "./components/Dialogs";
import { t, errMsg } from "./i18n";
import { openRemoteDialog } from "./components/RemoteDialog";
import { webdavPresetFromUrl } from "./remoteProfiles";

// `smb:`, `cifs:`, `https:` und `davs:` stehen hier bewusst nicht: Sie gehen
// über rclone und werden weiter unten vor dieser Prüfung abgefangen. Übrig
// bleiben allein die Protokolle, die macOS selbst einhängt — und die sind
// sämtlich unverschlüsselt.
const INSECURE_SCHEMES = new Set(["http:", "ftp:", "afp:", "nfs:"]);
/** Protokolle, die DualBeam über das mitgelieferte rclone einhängt statt über
 * macOS. Sie führen in den eigenen Verbindungsdialog, weil dafür Benutzername
 * und Kennwort einzeln gebraucht werden. */
const RCLONE_SCHEMES: Record<
  string,
  "sftp" | "ftps" | "ftpes" | "smb" | "webdav"
> = {
  "sftp:": "sftp",
  "ssh:": "sftp",
  "ftps:": "ftps",
  "ftpes:": "ftpes",
  // SMB lief früher über den Finder. Der meldete für Windows-Freigaben nur
  // „Server antwortet nicht" (-5016) und kam nie bis zur Anmeldung.
  "smb:": "smb",
  "cifs:": "smb",
  // WebDAV lief ebenfalls über den Finder. Der fragte Benutzer und Kennwort
  // selbst ab, weshalb DualBeam weder Anbieter noch Adresspfad anbieten konnte
  // und kein Lesezeichen entstand.
  "https:": "webdav",
  "davs:": "webdav",
};
let connecting = false;

/** Nur direkte lokale IP-Adressen sind für die ausdrücklich unsicheren
 * Protokolle zugelassen. Die finale, maßgebliche Prüfung erfolgt nochmals im
 * Rust-Backend; diese Prüfung steuert lediglich den Warnhinweis im UI. */
function isDirectLocalIp(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, "").toLowerCase();
  const octets = host.split(".");
  if (octets.length === 4 && octets.every((part) => /^\d+$/.test(part))) {
    const [a, b, c, d] = octets.map(Number);
    if ([a, b, c, d].some((part) => part > 255)) return false;
    return (
      a === 10 ||
      a === 127 ||
      (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168)
    );
  }
  if (!host.includes(":")) return false;
  if (host === "::1") return true;
  const first = Number.parseInt(host.split(":")[0] || "0", 16);
  return (
    Number.isFinite(first) &&
    ((first & 0xffc0) === 0xfe80 || (first & 0xfe00) === 0xfc00)
  );
}

/** Zerlegt eine sftp-/ftps-Adresse in die Felder des Verbindungsdialogs.
 * Liefert `null`, wenn das Schema nicht zu rclone gehört. Exportiert, damit es
 * sich einzeln prüfen lässt. */
export function remoteFromUrl(parsed: URL): {
  protocol: "sftp" | "ftpsExplicit" | "ftpsImplicit" | "smb" | "webdav";
  host: string;
  port: string;
  path: string;
  /** Nur bei WebDAV gefüllt: der Teil der Adresse hinter dem Rechnernamen.
   * Er gehört zur Adresse, nicht zum Startordner. */
  basePath?: string;
} | null {
  const kind = RCLONE_SCHEMES[parsed.protocol];
  if (!kind) return null;
  if (kind === "webdav") {
    const preset = webdavPresetFromUrl(parsed.href);
    return { protocol: "webdav", path: "", ...preset };
  }
  // `ftps:` meint hier die implizite Variante (Port 990), `ftpes:` die
  // explizite. Wer es anders braucht, stellt es im Dialog um.
  const protocol =
    kind === "sftp"
      ? "sftp"
      : kind === "smb"
        ? "smb"
        : kind === "ftps"
          ? "ftpsImplicit"
          : "ftpsExplicit";
  return {
    protocol,
    host: parsed.hostname.replace(/^\[|\]$/g, ""),
    port: parsed.port,
    path: decodeURIComponent(parsed.pathname || ""),
  };
}

/** Öffnet den Finder-ähnlichen Dialog „Mit Server verbinden …“ (⌘K). */
export async function connectToServer(): Promise<void> {
  if (connecting) return;
  const input = await askPrompt({
    title: t("network.connectTitle"),
    label: t("network.connectLabel"),
    placeholder: "smb://server/share",
    okLabel: t("network.connect"),
  });
  if (input == null) return;
  const trimmed = input.trim();
  if (!trimmed) return;

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.invalidUrl"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }
  // Bei den eigenen Protokollen darf ein Benutzername in der Adresse stehen; er
  // landet nur als Vorbelegung im Dialog. Ein Kennwort bleibt auch dort
  // ausgeschlossen, damit es nicht in der Eingabezeile auftaucht.
  const allowsUser = RCLONE_SCHEMES[parsed.protocol] != null;
  if ((parsed.username && !allowsUser) || parsed.password) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.credentials"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }
  const remote = remoteFromUrl(parsed);
  if (remote) {
    if (!remote.host) {
      await askConfirm({
        title: t("network.connectFailed"),
        message: t("err.network.invalidUrl"),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
      return;
    }
    openRemoteDialog({
      protocol: remote.protocol,
      host: remote.host,
      port: remote.port,
      path: remote.path,
      basePath: remote.basePath ?? "",
      username: decodeURIComponent(parsed.username || ""),
    });
    return;
  }
  if (!INSECURE_SCHEMES.has(parsed.protocol)) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.scheme"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }

  // Ab hier bleiben nur noch die Protokolle übrig, die macOS selbst einhängt.
  // Sie sind ausnahmslos unverschlüsselt — die Prüfung oben hat alles andere
  // bereits an den eigenen Verbindungsdialog abgegeben.
  const insecure = true;
  if (!isDirectLocalIp(parsed.hostname)) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.localIpOnly"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }
  if (
    !(await askConfirm({
      title: t("network.insecureTitle"),
      message: t("network.insecureWarning", { url: trimmed }),
      okLabel: t("network.insecureConnect"),
      cancelLabel: t("common.cancel"),
      danger: true,
    }))
  )
    return;

  connecting = true;
  try {
    const mountPath = await mountNetworkUrl(trimmed, insecure);
    bumpVolumes();
    if (mountPath) await loadPane(state.active, mountPath);
  } catch (err) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: errMsg(err),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
  } finally {
    connecting = false;
  }
}
