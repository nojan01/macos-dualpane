import { state, loadPane, bumpVolumes } from "./state";
import { mountNetworkUrl } from "./ipc";
import { askPrompt, askConfirm } from "./components/Dialogs";
import { t, errMsg } from "./i18n";

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
