import { state, loadPane, bumpVolumes } from "./state";
import { mountNetworkUrl } from "./ipc";
import { askPrompt, askConfirm } from "./components/Dialogs";
import { t, errMsg } from "./i18n";

const ALLOWED_SCHEMES = [
  "smb://",
  "afp://",
  "nfs://",
  "ftp://",
  "ftps://",
  "https://",
  "http://",
  "cifs://",
];

/** Öffnet einen Dialog „Mit Server verbinden …“ (⌘K, wie im Finder),
 *  hängt das angegebene Netzlaufwerk ein und wechselt in den Mount-Punkt. */
export async function connectToServer(): Promise<void> {
  const url = await askPrompt({
    title: t("network.connectTitle"),
    label: t("network.connectLabel"),
    placeholder: "smb://server/share",
    okLabel: t("network.connect"),
  });
  if (url == null) return;
  const trimmed = url.trim();
  if (!trimmed) return;

  const lower = trimmed.toLowerCase();
  if (!ALLOWED_SCHEMES.some((s) => lower.startsWith(s))) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: t("err.network.scheme"),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
    return;
  }

  try {
    const mountPath = await mountNetworkUrl(trimmed);
    bumpVolumes();
    if (mountPath) await loadPane(state.active, mountPath);
  } catch (err) {
    await askConfirm({
      title: t("network.connectFailed"),
      message: errMsg(err),
      okLabel: t("common.ok"),
      cancelLabel: t("common.close"),
    });
  }
}
