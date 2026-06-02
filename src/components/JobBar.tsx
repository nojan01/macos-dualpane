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
            <span class="kind">{j().kind === "copy" ? t("jobbar.copying") : t("jobbar.moving")}</span>
            <div class="bar"><div class="bar-fill jobbar-fill" ref={(el) => createEffect(() => el.style.setProperty("--pct", `${pct()}%`))} /></div>
            <span class="prog">{j().done} / {j().total || "?"}</span>
            <span class="cur">{j().current.split("/").pop() ?? ""}</span>
            <button onClick={() => cancelJob(j().id)}>{t("common.cancel")}</button>
          </div>
        );
      }}
    </Show>
  );
}
