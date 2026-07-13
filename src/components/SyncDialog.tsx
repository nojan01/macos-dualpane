import { Show, For, createMemo } from "solid-js";
import {
  syncDialog,
  syncEntries,
  syncDeleteExtra,
  syncLoading,
  syncPreviewReady,
  syncIgnorePatterns,
  activeSyncProfileId,
  syncMode,
  syncVerifyChecksums,
  syncTransport,
  syncRsyncHost,
  syncRsyncUsername,
  syncRsyncRemotePath,
  syncRsyncPassword,
  syncRsyncSavePassword,
  syncConflictChoices,
  setSyncDelete,
  setSyncIgnoreText,
  setSyncModeAndRefresh,
  setSyncVerifyChecksumsAndRefresh,
  setSyncTransportAndRefresh,
  setSyncRsyncHostValue,
  setSyncRsyncUsernameValue,
  setSyncRsyncRemotePathValue,
  setSyncRsyncPasswordValue,
  setSyncRsyncSavePasswordValue,
  loadSyncRsyncPasswordFromKeychain,
  setSyncConflictChoice,
  refreshSyncPreview,
  applySyncProfile,
  saveCurrentSyncProfile,
  deleteCurrentSyncProfile,
  cancelSync,
  confirmSync,
} from "../sync";
import { notifyError } from "./Dialogs";
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
    <>
      <Show when={syncLoading()}>
        <div class="sync-background-status" role="status">
          <span class="spinner" aria-hidden="true" />
          <span>{t("sync.preparingBackground")}</span>
          <button class="secondary" onClick={cancelSync}>
            {t("common.cancel")}
          </button>
        </div>
      </Show>
      <Show when={syncDialog()}>
        {(s) => (
          <Show when={!syncLoading()}>
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
                  <label class="sync-transport">
                    {t("sync.transport")}
                    <select
                      value={syncTransport()}
                      onChange={(event) =>
                        void setSyncTransportAndRefresh(
                          event.currentTarget.value as "filesystem" | "rsync",
                        )
                      }
                    >
                      <option value="filesystem">
                        {t("sync.transportFilesystem")}
                      </option>
                      <option value="rsync">{t("sync.transportRsync")}</option>
                    </select>
                  </label>
                  <Show when={syncTransport() === "rsync"}>
                    <div class="sync-rsync">
                      <p>{t("sync.rsyncMountedNote")}</p>
                      <label>
                        {t("rsync.host")}
                        <input
                          type="text"
                          value={syncRsyncHost()}
                          onInput={(event) =>
                            setSyncRsyncHostValue(event.currentTarget.value)
                          }
                        />
                      </label>
                      <label>
                        {t("rsync.username")}
                        <input
                          type="text"
                          autocomplete="username"
                          value={syncRsyncUsername()}
                          onInput={(event) =>
                            setSyncRsyncUsernameValue(event.currentTarget.value)
                          }
                        />
                      </label>
                      <label>
                        {t("rsync.password")}
                        <input
                          type="password"
                          autocomplete="current-password"
                          value={syncRsyncPassword()}
                          onInput={(event) =>
                            setSyncRsyncPasswordValue(event.currentTarget.value)
                          }
                        />
                      </label>
                      <label>
                        {t("rsync.remotePath")}
                        <input
                          type="text"
                          value={syncRsyncRemotePath()}
                          onInput={(event) =>
                            setSyncRsyncRemotePathValue(event.currentTarget.value)
                          }
                        />
                      </label>
                      <label class="sync-option">
                        <input
                          type="checkbox"
                          checked={syncRsyncSavePassword()}
                          onChange={(event) =>
                            setSyncRsyncSavePasswordValue(
                              event.currentTarget.checked,
                            )
                          }
                        />
                        {t("rsync.savePassword")}
                      </label>
                      <button
                        class="secondary"
                        onClick={() =>
                          void loadSyncRsyncPasswordFromKeychain().then(
                            (loaded) => {
                              if (!loaded) void notifyError(t("rsync.passwordMissing"));
                            },
                            (error) => void notifyError(String(error)),
                          )
                        }
                      >
                        {t("rsync.loadPassword")}
                      </button>
                    </div>
                  </Show>
                  <Show when={syncTransport() === "filesystem"}>
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
                  </Show>
                  <Show when={syncTransport() === "filesystem"}>
                    <div class="sync-preview-action">
                      <p>{t("sync.previewHint")}</p>
                      <button
                        class="secondary"
                        onClick={() => void refreshSyncPreview()}
                      >
                        {t("sync.preview")}
                      </button>
                    </div>
                  </Show>
                  <ul class="modal-list">
                    <Show when={syncTransport() === "rsync"}>
                      <li>{t("sync.rsyncDirect")}</li>
                    </Show>
                    <Show
                      when={
                        syncTransport() === "filesystem" && syncPreviewReady()
                      }
                    >
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
                    </Show>
                  </ul>
                  <Show
                    when={
                      syncTransport() === "filesystem" &&
                      syncPreviewReady() &&
                      validEntries().length === 0
                    }
                  >
                    <p>{t("sync.upToDate")}</p>
                  </Show>
                  <Show
                    when={
                      syncTransport() === "filesystem" &&
                      syncPreviewReady() &&
                      validEntries().length > 0
                    }
                  >
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
                  <Show
                    when={
                      syncTransport() === "rsync" || syncMode() === "oneWay"
                    }
                  >
                    <p class="danger">
                      {syncTransport() === "rsync"
                        ? t("sync.rsyncDeleteNote")
                        : syncPreviewReady() && counts().del > 0
                          ? t("sync.extrasPrompt", { count: counts().del })
                          : t("sync.deleteExtraHint")}
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
                disabled={
                  syncLoading() ||
                  (syncTransport() === "filesystem" && effectiveTotal() === 0)
                  || (syncTransport() === "filesystem" && !syncPreviewReady())
                }
              >
                {t("sync.start")}
              </button>
              <button class="secondary" onClick={cancelSync}>
                {t("common.cancel")}
              </button>
            </div>
          </div>
        </div>
          </Show>
        )}
      </Show>
    </>
  );
}
