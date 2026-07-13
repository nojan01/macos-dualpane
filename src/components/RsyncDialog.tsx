import { Show, createSignal } from "solid-js";
import { state } from "../state";
import { loadRsyncPassword, runRsync, saveRsyncPassword } from "../ipc";
import { t, errMsg } from "../i18n";

type RsyncDialogState = {
  localPath: string;
  host: string;
  remotePath: string;
  username: string;
  password: string;
  savePassword: boolean;
  deleteExtra: boolean;
  running: boolean;
  error: string | null;
};

const [dialog, setDialog] = createSignal<RsyncDialogState | null>(null);

/** Öffnet die direkte rsync-over-SSH-Synchronisation. Der aktive Pane-Ordner
 * ist bewusst vorausgefüllt, kann aber vor dem Start angepasst werden. */
export function openRsyncDialog(
  host: string,
  remotePath: string,
  username = "",
) {
  const localPath = state[state.active].cwd;
  if (!localPath) return;
  setDialog({
    localPath,
    host,
    remotePath: remotePath || "/",
    username,
    password: "",
    savePassword: true,
    deleteExtra: false,
    running: false,
    error: null,
  });
}

export function RsyncDialog() {
  const update = (patch: Partial<RsyncDialogState>) =>
    setDialog((current) => (current ? { ...current, ...patch } : current));

  const start = async () => {
    const current = dialog();
    if (!current || current.running) return;
    if (!current.localPath || !current.username || !current.password) {
      update({ error: t("rsync.required") });
      return;
    }
    update({ running: true, error: null });
    try {
      if (current.savePassword) {
        await saveRsyncPassword(current.host, current.username, current.password);
      }
      await runRsync({
        jobId: `rsync-${Date.now()}-${Math.floor(Math.random() * 1e6)}`,
        localPath: current.localPath,
        host: current.host,
        remotePath: current.remotePath,
        username: current.username,
        password: current.password,
        deleteExtra: current.deleteExtra,
        excludePatterns: [],
      });
      setDialog(null);
    } catch (error) {
      update({ running: false, error: errMsg(error) });
    }
  };

  const loadPassword = async () => {
    const current = dialog();
    if (!current || current.running || !current.username) {
      update({ error: t("rsync.usernameRequired") });
      return;
    }
    try {
      const password = await loadRsyncPassword(current.host, current.username);
      if (!password) {
        update({ error: t("rsync.passwordMissing") });
        return;
      }
      update({ password, error: null });
    } catch (error) {
      update({ error: errMsg(error) });
    }
  };

  return (
    <Show when={dialog()}>
      {(current) => (
        <div
          class="modal-backdrop"
          onMouseDown={() => !current().running && setDialog(null)}
        >
          <div
            class="modal rsync-dialog"
            role="dialog"
            aria-modal="true"
            aria-label={t("rsync.title")}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <h2>{t("rsync.title")}</h2>
            <p>{t("rsync.description")}</p>
            <label>
              {t("rsync.localPath")}
              <input
                type="text"
                value={current().localPath}
                disabled={current().running}
                onInput={(event) => update({ localPath: event.currentTarget.value })}
              />
            </label>
            <label>
              {t("rsync.username")}
              <input
                type="text"
                autocomplete="username"
                value={current().username}
                disabled={current().running}
                onInput={(event) => update({ username: event.currentTarget.value })}
              />
            </label>
            <label>
              {t("rsync.password")}
              <input
                type="password"
                autocomplete="current-password"
                value={current().password}
                disabled={current().running}
                onInput={(event) => update({ password: event.currentTarget.value })}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void start();
                }}
              />
            </label>
            <label>
              {t("rsync.host")}
              <input
                type="text"
                value={current().host}
                disabled={current().running}
                onInput={(event) => update({ host: event.currentTarget.value })}
              />
            </label>
            <label>
              {t("rsync.remotePath")}
              <input
                type="text"
                value={current().remotePath}
                disabled={current().running}
                onInput={(event) => update({ remotePath: event.currentTarget.value })}
              />
            </label>
            <label class="check-line">
              <input
                type="checkbox"
                checked={current().deleteExtra}
                disabled={current().running}
                onChange={(event) => update({ deleteExtra: event.currentTarget.checked })}
              />
              {t("rsync.deleteExtra")}
            </label>
            <label class="check-line">
              <input
                type="checkbox"
                checked={current().savePassword}
                disabled={current().running}
                onChange={(event) => update({ savePassword: event.currentTarget.checked })}
              />
              {t("rsync.savePassword")}
            </label>
            <Show when={current().error}>
              <p class="error pre-wrap">{current().error}</p>
            </Show>
            <div class="modal-actions">
              <button disabled={current().running} onClick={() => void start()}>
                {current().running ? t("rsync.running") : t("rsync.start")}
              </button>
              <button
                class="secondary"
                disabled={current().running}
                onClick={() => void loadPassword()}
              >
                {t("rsync.loadPassword")}
              </button>
              <button
                class="secondary"
                disabled={current().running}
                onClick={() => setDialog(null)}
              >
                {t("common.cancel")}
              </button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
