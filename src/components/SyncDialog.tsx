import { Show, For, createMemo } from "solid-js";
import {
  syncDialog,
  syncEntries,
  syncDeleteExtra,
  syncLoading,
  syncIgnorePatterns,
  activeSyncProfileId,
  syncMode,
  syncVerifyChecksums,
  syncConflictChoices,
  setSyncDelete,
  setSyncIgnoreText,
  setSyncModeAndRefresh,
  setSyncVerifyChecksumsAndRefresh,
  setSyncConflictChoice,
  refreshSyncPreview,
  applySyncProfile,
  saveCurrentSyncProfile,
  deleteCurrentSyncProfile,
  cancelSync,
  confirmSync,
} from "../sync";
import { syncProfiles } from "../syncProfiles";
import type { SyncAction, SyncEntry } from "../ipc";
import { t } from "../i18n";

const ACTION_LABEL: Record<SyncAction, string> = {
  copy: "sync.actionNew",
  update: "sync.actionChanged",
  delete: "sync.actionDelete",
  left_to_right: "sync.actionLeftToRight",
  right_to_left: "sync.actionRightToLeft",
  conflict: "sync.actionConflict",
};

export function SyncDialog() {
  const validEntries = () =>
    syncEntries().filter(
      (entry): entry is SyncEntry =>
        !!entry &&
        typeof entry.rel === "string" &&
        typeof entry.action === "string",
    );
  const counts = createMemo(() => {
    let copy = 0,
      update = 0,
      del = 0,
      leftToRight = 0,
      rightToLeft = 0,
      conflicts = 0;
    for (const e of validEntries()) {
      if (e.action === "copy") copy++;
      else if (e.action === "update") update++;
      else if (e.action === "delete") del++;
      else if (e.action === "left_to_right") leftToRight++;
      else if (e.action === "right_to_left") rightToLeft++;
      else if (e.action === "conflict") conflicts++;
    }
    return { copy, update, del, leftToRight, rightToLeft, conflicts };
  });
  // Was tatsächlich ausgeführt wird: Kopien/Updates immer, Löschungen nur wenn bestätigt.
  const effectiveTotal = () =>
    syncMode() === "twoWay"
      ? counts().leftToRight +
        counts().rightToLeft +
        validEntries().filter(
          (entry) =>
            entry.action === "conflict" &&
            syncConflictChoices()[entry.rel] !== "skip",
        ).length
      : counts().copy +
        counts().update +
        (syncDeleteExtra() ? counts().del : 0);

  return (
    <Show when={syncDialog()}>
      {(s) => (
        <div class="modal-backdrop" onClick={cancelSync}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h2>{t("sync.title")}</h2>
            <p>{t("sync.summary", { name: s().srcName })}</p>
            <Show
              when={syncLoading()}
              fallback={
                <>
                  <div class="sync-profiles">
                    <select
                      value={activeSyncProfileId() ?? ""}
                      onChange={(e) => {
                        const id = e.currentTarget.value;
                        if (id) void applySyncProfile(id);
                      }}
                      aria-label={t("sync.profileSelect")}
                    >
                      <option value="">{t("sync.profileSelect")}</option>
                      <For each={syncProfiles()}>
                        {(profile) => (
                          <option value={profile.id}>{profile.name}</option>
                        )}
                      </For>
                    </select>
                    <button
                      class="secondary"
                      onClick={() => void saveCurrentSyncProfile()}
                    >
                      {t("sync.profileSave")}
                    </button>
                    <Show when={activeSyncProfileId()}>
                      <button
                        class="secondary"
                        onClick={() => void deleteCurrentSyncProfile()}
                      >
                        {t("common.delete")}
                      </button>
                    </Show>
                  </div>
                  <label class="sync-option">
                    <input
                      type="checkbox"
                      checked={syncMode() === "twoWay"}
                      onChange={(e) =>
                        void setSyncModeAndRefresh(
                          e.currentTarget.checked ? "twoWay" : "oneWay",
                        )
                      }
                    />
                    {t("sync.twoWay")}
                  </label>
                  <label class="sync-option">
                    <input
                      type="checkbox"
                      checked={syncVerifyChecksums()}
                      onChange={(e) =>
                        void setSyncVerifyChecksumsAndRefresh(
                          e.currentTarget.checked,
                        )
                      }
                    />
                    {t("sync.verifyChecksums")}
                  </label>
                  <ul class="modal-list">
                    <Show
                      when={syncMode() === "twoWay"}
                      fallback={
                        <>
                          <li>
                            {t("sync.copyCount", { count: counts().copy })}
                          </li>
                          <li>
                            {t("sync.updateCount", { count: counts().update })}
                          </li>
                        </>
                      }
                    >
                      <li>
                        {t("sync.leftToRightCount", {
                          count: counts().leftToRight,
                        })}
                      </li>
                      <li>
                        {t("sync.rightToLeftCount", {
                          count: counts().rightToLeft,
                        })}
                      </li>
                      <Show when={counts().conflicts > 0}>
                        <li class="danger">
                          {t("sync.conflictCount", {
                            count: counts().conflicts,
                          })}
                        </li>
                      </Show>
                    </Show>
                    <Show when={syncMode() === "oneWay" && counts().del > 0}>
                      <li class="danger">
                        {t("sync.deleteCount", { count: counts().del })}
                      </li>
                    </Show>
                  </ul>
                  <Show when={validEntries().length === 0}>
                    <p>{t("sync.upToDate")}</p>
                  </Show>
                  <Show when={validEntries().length > 0}>
                    <details class="sync-details">
                      <summary>{t("sync.details")}</summary>
                      <ul class="sync-details-list">
                        <For each={validEntries()}>
                          {(e) => (
                            <li
                              classList={{
                                danger:
                                  e.action === "delete" ||
                                  e.action === "conflict",
                              }}
                            >
                              <span class="sync-details-badge">
                                {t(
                                  ACTION_LABEL[e.action] ??
                                    "sync.actionConflict",
                                )}
                              </span>
                              <span class="sync-details-path">{e.rel}</span>
                              <Show when={e.action === "conflict"}>
                                <select
                                  class="sync-conflict-choice"
                                  value={syncConflictChoices()[e.rel] ?? "skip"}
                                  onChange={(event) =>
                                    setSyncConflictChoice(
                                      e.rel,
                                      event.currentTarget.value as
                                        "left" | "right" | "skip",
                                    )
                                  }
                                >
                                  <option value="skip">
                                    {t("sync.conflictSkip")}
                                  </option>
                                  <option value="left">
                                    {t("sync.conflictLeft")}
                                  </option>
                                  <option value="right">
                                    {t("sync.conflictRight")}
                                  </option>
                                </select>
                              </Show>
                            </li>
                          )}
                        </For>
                      </ul>
                    </details>
                  </Show>
                  <Show when={syncMode() === "oneWay" && counts().del > 0}>
                    <p class="danger">
                      {t("sync.extrasPrompt", { count: counts().del })}
                    </p>
                    <label class="sync-option">
                      <input
                        type="checkbox"
                        checked={syncDeleteExtra()}
                        onChange={(e) =>
                          setSyncDelete(
                            (e.currentTarget as HTMLInputElement).checked,
                          )
                        }
                      />
                      {t("sync.deleteExtra")}
                    </label>
                  </Show>
                  <details class="sync-ignore">
                    <summary>{t("sync.ignoreTitle")}</summary>
                    <p>{t("sync.ignoreHelp")}</p>
                    <textarea
                      value={syncIgnorePatterns()}
                      placeholder={t("sync.ignorePlaceholder")}
                      onInput={(e) => setSyncIgnoreText(e.currentTarget.value)}
                    />
                    <button
                      class="secondary"
                      onClick={() => void refreshSyncPreview()}
                    >
                      {t("sync.ignoreApply")}
                    </button>
                  </details>
                </>
              }
            >
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
              <button class="secondary" onClick={cancelSync}>
                {t("common.cancel")}
              </button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
