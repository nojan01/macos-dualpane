/**
 * Software-Aktualisierung über den Tauri-Updater.
 *
 * Der Ablauf ist bewusst zweigeteilt: Beim Programmstart wird still geprüft und
 * nur bei einer verfügbaren Version nachgefragt. Über das Menü wird dieselbe
 * Prüfung ausdrücklich angestoßen und meldet sich auch dann, wenn schon alles
 * aktuell ist – sonst bliebe der Menübefehl scheinbar wirkungslos.
 *
 * Heruntergeladen und ausgetauscht wird im Rust-Backend. Die strenge CSP der
 * Oberfläche spielt deshalb keine Rolle: Aus dem Webview geht nur ein
 * IPC-Aufruf hinaus, nicht die Netzwerkverbindung selbst.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check } from "@tauri-apps/plugin-updater";
import { askConfirm, notify, notifyError } from "./components/Dialogs";
import { t } from "./i18n";
import { appVersion, restartApplication } from "./ipc";

/** Verhindert zwei gleichzeitige Prüfungen, etwa Start und Menübefehl. */
let running = false;

/**
 * Setzt den Fenstertitel und liefert eine Funktion, die ihn zurücksetzt.
 * Der Fortschritt gehört in den Titel, weil die Rückfragedialoge modal sind
 * und sich nachträglich nicht mehr aktualisieren lassen.
 */
async function titleReporter(): Promise<{
  show: (text: string) => void;
  reset: () => void;
}> {
  const win = getCurrentWindow();
  let original = "DualBeam";
  try {
    original = await win.title();
  } catch {
    // Titel nicht lesbar: Rückfallwert aus tauri.conf.json.
  }
  return {
    show: (text) => void win.setTitle(text).catch(() => {}),
    reset: () => void win.setTitle(original).catch(() => {}),
  };
}

/**
 * Prüft auf eine neue Version und installiert sie nach Rückfrage.
 *
 * @param interactive Bei `true` wird auch gemeldet, dass keine neue Version
 *   vorliegt, und Fehler werden angezeigt statt nur protokolliert.
 */
export async function checkForUpdates(interactive = false): Promise<void> {
  if (running) return;
  running = true;

  const title = t("update.title");
  const reporter = await titleReporter();

  try {
    const update = await check();

    if (!update) {
      if (interactive) {
        await notify({
          title,
          message: t("update.upToDate", { version: await appVersion() }),
        });
      }
      return;
    }

    const install = await askConfirm({
      title,
      message: `${t("update.available", {
        version: update.version,
        current: update.currentVersion,
      })}\n\n${t("update.question")}`,
      okLabel: t("update.install"),
      cancelLabel: t("update.later"),
    });
    if (!install) return;

    let total = 0;
    let loaded = 0;
    reporter.show(t("update.preparing"));

    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? 0;
          break;
        case "Progress":
          loaded += event.data.chunkLength;
          reporter.show(
            total > 0
              ? t("update.downloading", {
                  percent: Math.round((loaded / total) * 100),
                })
              : t("update.preparing"),
          );
          break;
        case "Finished":
          reporter.show(t("update.installing"));
          break;
      }
    });

    reporter.reset();
    await notify({
      title,
      message: t("update.done", { version: update.version }),
      okLabel: t("update.restart"),
    });
    await restartApplication();
  } catch (err) {
    reporter.reset();
    const detail = err instanceof Error ? err.message : String(err);
    console.error("Update-Prüfung fehlgeschlagen:", detail);
    if (interactive) {
      await notifyError(`${t("err.update.failed")}\n\n${detail}`);
    }
  } finally {
    reporter.reset();
    running = false;
  }
}
