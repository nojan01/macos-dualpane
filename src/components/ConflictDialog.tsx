import { Show } from "solid-js";
import { conflictPrompt, resolveConflict } from "../jobs";
import { t } from "../i18n";

export function ConflictDialog() {
  return (
    <Show when={conflictPrompt()}>
      {(p) => (
        <div class="modal-backdrop" onClick={() => resolveConflict("cancel")}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h2>{t("conflict.title")}</h2>
            <p>
              {p().count === 1
                ? t("conflict.oneExists")
                : t("conflict.manyExists", { count: p().count })}
            </p>
            <ul class="modal-list">
              {p().sample.map((n) => <li>{n}</li>)}
              {p().count > p().sample.length && <li>{t("conflict.more", { count: p().count - p().sample.length })}</li>}
            </ul>
            <div class="modal-actions">
              <button onClick={() => resolveConflict("overwrite")}>{t("conflict.overwrite")}</button>
              <button onClick={() => resolveConflict("rename")}>{t("conflict.keepBoth")}</button>
              <button onClick={() => resolveConflict("skip")}>{t("conflict.skip")}</button>
              <button class="secondary" onClick={() => resolveConflict("cancel")}>{t("common.cancel")}</button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
