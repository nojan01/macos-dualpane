import { Show, createEffect } from "solid-js";
import { state } from "../state";
import { cancelJob } from "../ipc";
import { t } from "../i18n";

export function JobBar() {
  return (
    <Show when={state.job}>
      {(j) => {
        const pct = () => {
          const t = j().total;
          return t > 0 ? Math.min(100, Math.round((j().done / t) * 100)) : 0;
        };
        return (
          <div class="jobbar">
            <span class="kind">{j().kind === "rsync" ? t("jobbar.rsync") : j().kind === "copy" ? t("jobbar.copying") : j().kind === "delete" ? t("jobbar.deleting") : t("jobbar.moving")}</span>
            <div class="bar">
              <div
                class="bar-fill jobbar-fill"
                classList={{ indeterminate: j().kind === "rsync" }}
                ref={(el) =>
                  createEffect(() =>
                    el.style.setProperty("--pct", `${pct()}%`),
                  )
                }
              />
            </div>
            <span class="prog">
              <Show
                when={j().kind !== "rsync"}
                fallback={t("jobbar.filesCopied", { count: j().filesDone })}
              >
                {t("jobbar.items", { done: j().done, total: j().total || "?" })}
              </Show>
              <Show when={j().kind !== "delete" && j().kind !== "rsync"}>
                {" · "}
                {t("jobbar.filesCopied", { count: j().filesDone })}
              </Show>
            </span>
            <span class="cur">{j().current.split("/").pop() ?? ""}</span>
            <button onClick={() => cancelJob(j().id)}>{t("common.cancel")}</button>
          </div>
        );
      }}
    </Show>
  );
}
