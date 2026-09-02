import { Show, createEffect } from "solid-js";
import { state } from "../state";
import { cancelJob } from "../ipc";
import { t } from "../i18n";

export function JobBar() {
  return (
    <Show when={state.job}>
      {(j) => {
        const pct = () => {
          if (j().transferPercent !== undefined) return j().transferPercent ?? 0;
          const t = j().total;
          return t > 0 ? Math.min(100, Math.round((j().done / t) * 100)) : 0;
        };
        return (
          <div class="jobbar">
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
