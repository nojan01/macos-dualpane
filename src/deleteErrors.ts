// Gemeinsame Auswertung der Kennungen, die das Backend beim Löschen liefert.
// Solche Fehler haben eine eigene Kennung statt eines fertigen Satzes, damit
// die Oberfläche sie übersetzen kann – ein rohes "Resource busy (os error 16)"
// sagt niemandem, was zu tun ist.
import { t } from "./i18n";
import { notify } from "./components/Dialogs";

/** Trennzeichen zwischen Kennung und Nutzlast (ASCII Unit Separator). */
const SEP = "\u001f";

function payload(raw: string): string {
  return raw.split(SEP)[1] ?? "";
}

function lastSegment(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * Zeigt zu einer bekannten Lösch-Kennung eine erklärende Meldung an.
 * Gibt `true` zurück, wenn der Fehler behandelt wurde; sonst `false`, damit der
 * Aufrufer die allgemeine Fehlermeldung ausgibt.
 */
export async function reportKnownDeleteError(raw: string): Promise<boolean> {
  if (raw.includes("TIMEMACHINE_PROTECTED")) {
    await notify({
      title: t("jobs.trash.timeMachine.title"),
      message: t("jobs.trash.timeMachine.message"),
    });
    return true;
  }
  if (raw.includes("NETWORK_BUSY")) {
    await notify({
      title: t("jobs.trash.networkBusy.title"),
      message: t("jobs.trash.networkBusy.message", {
        name: lastSegment(payload(raw)),
      }),
    });
    return true;
  }
  return false;
}
