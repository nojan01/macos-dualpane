import { Show, createEffect } from "solid-js";
import { state } from "../state";
import { cancelJob } from "../ipc";
import { t, intlLocale } from "../i18n";

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toLocaleString(intlLocale(), {
    maximumFractionDigits: v >= 100 ? 0 : 1,
  })} ${units[i]}`;
}

function fmtElapsed(ms: number): string {
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const sec = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

export function JobBar() {
  return (
    <Show when={state.job}>
      {(j) => {
        const pct = () => {
          if (j().transferPercent !== undefined) return j().transferPercent ?? 0;
          const t = j().total;
          if (t <= 0) return 0;
          // Der Bruchteil der laufenden Datei lässt den Balken auch bei einer
          // einzelnen großen Datei sichtbar wachsen.
          const fp = j().fileProgress;
          const frac =
            fp && fp.bytesDone !== undefined && fp.bytesTotal > 0
              ? Math.min(1, fp.bytesDone / fp.bytesTotal)
              : 0;
          return Math.min(100, ((j().done + frac) / t) * 100);
        };
        const fileInfo = () => {
          const fp = j().fileProgress;
          if (!fp) return null;
          if (fp.bytesDone !== undefined && fp.bytesTotal > 0) {
            return t("jobbar.fileBytes", {
              done: fmtBytes(fp.bytesDone),
              total: fmtBytes(fp.bytesTotal),
              percent: Math.floor((fp.bytesDone / fp.bytesTotal) * 100),
            });
          }
          return t("jobbar.elapsed", { time: fmtElapsed(fp.elapsedMs) });
        };
        return (
          <div class="jobbar">
            {/* Dauerhaft animiert: zeigt auch ohne neue Ereignisse, dass der
                Auftrag noch läuft und die Oberfläche nicht hängt. */}
            <span
              class="job-spinner"
              role="status"
              aria-label={t("jobbar.active")}
              title={t("jobbar.active")}
            />
            <span class="kind">{j().kind === "copy" ? t("jobbar.copying") : j().kind === "delete" ? t("jobbar.deleting") : t("jobbar.moving")}</span>
            <div class="bar">
              <div
                class="bar-fill jobbar-fill"
                classList={{
                  indeterminate:
                    !!j().indeterminate ||
                    (j().kind === "delete" && j().total === 0),
                }}
                ref={(el) =>
                  createEffect(() =>
                    // Als Faktor 0…1, weil der Balken über `scaleX` skaliert
                    // wird statt seine Breite zu ändern (kein Layout je Schritt).
                    el.style.setProperty("--progress", `${pct() / 100}`),
                  )
                }
              />
            </div>
            <span class="prog">
              <Show
                when={j().transferPercent !== undefined}
                fallback={
                  <Show
                    when={j().indeterminate}
                    fallback={
                      /* Auf Netzlaufwerken ist die Gesamtzahl nicht bekannt: Sie
                         vorab zu ermitteln würde so lange dauern wie das Löschen
                         selbst. Dann lieber melden, was schon erledigt ist, statt
                         „0 / ?" anzuzeigen. */
                      <Show
                        when={j().kind === "delete" && j().total === 0}
                        fallback={t("jobbar.items", {
                          done: j().done,
                          total: j().total || "?",
                        })}
                      >
                        {t("jobbar.itemsDeleted", { count: j().done })}
                      </Show>
                    }
                  >
                    {t("common.loading")}
                  </Show>
                }
              >
                {j().transferPercent} %
              </Show>
              <Show when={j().kind !== "delete"}>
                {" · "}
                {t("jobbar.filesCopied", { count: j().filesDone })}
              </Show>
              <Show when={fileInfo()}>
                {(info) => (
                  <>
                    {" · "}
                    {info()}
                  </>
                )}
              </Show>
            </span>
            <span class="cur">{j().current.split("/").pop() ?? ""}</span>
            <button onClick={() => void cancelJob(j().id).catch(() => {})}>
              {t("common.cancel")}
            </button>
          </div>
        );
      }}
    </Show>
  );
}
