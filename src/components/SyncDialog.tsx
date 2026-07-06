import { Show, createMemo } from "solid-js";
import {
  syncDialog, syncEntries, syncDeleteExtra, syncLoading,
  setSyncDelete, cancelSync, confirmSync,
} from "../sync";
import { t } from "../i18n";

export function SyncDialog() {
  const counts = createMemo(() => {
    let copy = 0, update = 0, del = 0;
    for (const e of syncEntries()) {
      if (e.action === "copy") copy++;
      else if (e.action === "update") update++;
      else if (e.action === "delete") del++;
    }
    return { copy, update, del };
  });
  // Was tatsächlich ausgeführt wird: Kopien/Updates immer, Löschungen nur wenn bestätigt.
  const effectiveTotal = () =>
    counts().copy + counts().update + (syncDeleteExtra() ? counts().del : 0);

  return (
    <Show when={syncDialog()}>
      {(s) => (
        <div class="modal-backdrop" onClick={cancelSync}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h2>{t("sync.title")}</h2>
            <p>{t("sync.summary", { name: s().srcName })}</p>
            <Show when={syncLoading()} fallback={
              <>
                <ul class="modal-list">
                  <li>{t("sync.copyCount", { count: counts().copy })}</li>
                  <li>{t("sync.updateCount", { count: counts().update })}</li>
                  <Show when={counts().del > 0}>
                    <li class="danger">{t("sync.deleteCount", { count: counts().del })}</li>
                  </Show>
                </ul>
                <Show when={counts().copy + counts().update + counts().del === 0}>
                  <p>{t("sync.upToDate")}</p>
                </Show>
                <Show when={counts().del > 0}>
                  <p class="danger">{t("sync.extrasPrompt", { count: counts().del })}</p>
                  <label class="sync-option">
                    <input
                      type="checkbox"
                      checked={syncDeleteExtra()}
                      onChange={(e) => setSyncDelete((e.currentTarget as HTMLInputElement).checked)}
                    />
                    {t("sync.deleteExtra")}
                  </label>
                </Show>
              </>
            }>
              <p class="sync-preparing">
                <span class="spinner" aria-hidden="true" />
                {t("sync.preparing")}
              </p>
            </Show>
            <div class="modal-actions">
              <button
                onClick={() => void confirmSync()}
                disabled={syncLoading() || effectiveTotal() === 0}
              >
                {t("sync.start")}
              </button>
              <button class="secondary" onClick={cancelSync}>{t("common.cancel")}</button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
