import { state, loadPane, bumpVolumes } from "./state";
import { mountNetworkUrl } from "./ipc";
import { askPrompt, askConfirm } from "./components/Dialogs";
import { t, errMsg } from "./i18n";
import { openRsyncDialog } from "./components/RsyncDialog";

const SECURE_SCHEMES = new Set(["https:", "smb:"]);
const INSECURE_SCHEMES = new Set([
  "http:",
  "ftp:",
  "ftps:",
  "afp:",
  "nfs:",
  "cifs:",
]);
let connecting = false;

/** Akzeptiert zusätzlich den vollständigen, von IONOS dokumentierten
 * Terminal-Befehl. Damit kann z. B. `rsync -rltDv -e ssh .
 * jano150@rsync.hidrive.ionos.com:/users/jano150/` direkt in ⌘K eingefügt
 * werden; für DualBeam werden daraus Host, Benutzer und Zielpfad. */
function parseIonosRsyncCommand(input: string): {
  host: string;
  username: string;
  remotePath: string;
} | null {
  const match = input.trim().match(
    /(?:^|\s)([A-Za-z0-9._-]+)@([A-Za-z0-9.-]+):(\/[^\s]*)\s*$/,
  );
  if (!match) return null;
  const [, username, host, remotePath] = match;
  if (!host.toLowerCase().endsWith(".hidrive.ionos.com")) return null;
  return { username, host, remotePath };
}

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

  const ionosCommand = parseIonosRsyncCommand(trimmed);
  if (ionosCommand) {
    openRsyncDialog(
      ionosCommand.host,
      ionosCommand.remotePath,
      ionosCommand.username,
    );
    return;
  }

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
  if (parsed.username || parsed.password) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.credentials"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }
  if (parsed.protocol === "rsync:") {
    if (!parsed.hostname) {
      await askConfirm({
        title: t("network.connectFailed"),
        message: t("err.network.invalidUrl"),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
      return;
    }
    openRsyncDialog(parsed.hostname, decodeURIComponent(parsed.pathname || "/"));
    return;
  }
  if (
    !SECURE_SCHEMES.has(parsed.protocol) &&
    !INSECURE_SCHEMES.has(parsed.protocol)
  ) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.scheme"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }

  const insecure = INSECURE_SCHEMES.has(parsed.protocol);
  if (insecure && !isDirectLocalIp(parsed.hostname)) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.localIpOnly"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }
  if (
    insecure &&
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
