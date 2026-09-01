import { For, Show, createSignal } from "solid-js";
import { state, loadPane, bumpVolumes } from "../state";
import {
  loadRemotePassword,
  mountRemote,
  remoteMounts,
  remoteHostKeys,
  remoteTrustHost,
  saveRemotePassword,
  unmountRemote,
  type RemoteProtocol,
  type RemoteSpec,
} from "../ipc";
import { t, errMsg } from "../i18n";
import { remoteDescriptor, saveRemoteProfile } from "../remoteProfiles";

/** Reihenfolge im Auswahlfeld: die sichere Voreinstellung zuerst. */
const PROTOCOLS: RemoteProtocol[] = [
  "sftp",
  "ftpsExplicit",
  "ftpsImplicit",
  "ftp",
];

const DEFAULT_PORTS: Record<RemoteProtocol, number> = {
  sftp: 22,
  ftp: 21,
  ftpsExplicit: 21,
  ftpsImplicit: 990,
};

type RemoteDialogState = {
  protocol: RemoteProtocol;
  host: string;
  /** Als Text geführt, damit das Feld auch leer sein darf (= Standardport). */
  port: string;
  username: string;
  password: string;
  path: string;
  label: string;
  savePassword: boolean;
  busy: boolean;
  error: string | null;
  /** Gesetzt, solange der Fingerabdruck des Servers bestätigt werden muss. */
  fingerprints: string[] | null;
};

const [dialog, setDialog] = createSignal<RemoteDialogState | null>(null);

/** Öffnet „Netzlaufwerk verbinden“ für SFTP und FTPS. Die Felder können aus
 * einer eingegebenen Adresse vorbelegt werden. */
export function openRemoteDialog(preset: Partial<RemoteDialogState> = {}) {
  const initial: RemoteDialogState = {
    protocol: "sftp",
    host: "",
    port: "",
    username: "",
    password: "",
    path: "",
    label: "",
    savePassword: true,
    busy: false,
    error: null,
    fingerprints: null,
    ...preset,
  };
  setDialog(initial);
  // Ein gespeichertes Profil soll beim erneuten Öffnen kein neues Passwort
  // verlangen. Fehlt der Schlüsselbund-Eintrag, bleibt das Feld einfach leer.
  if (initial.host.trim() && initial.username.trim()) {
    void loadRemotePassword(toSpec(initial)).then(
      (password) => {
        if (!password) return;
        setDialog((current) =>
          current &&
          current.host === initial.host &&
          current.username === initial.username &&
          !current.password
            ? { ...current, password }
            : current,
        );
      },
      () => {},
    );
  }
}

/** Unverschlüsseltes FTP zeigt denselben Warnhinweis wie im ⌘K-Dialog. */
function isInsecure(protocol: RemoteProtocol): boolean {
  return protocol === "ftp";
}

function toSpec(current: RemoteDialogState): RemoteSpec {
  const port = current.port.trim();
  return {
    protocol: current.protocol,
    host: current.host.trim(),
    port: port ? Number(port) : null,
    username: current.username.trim(),
    path: current.path.trim(),
    label: current.label.trim(),
  };
}

export function RemoteDialog() {
  const update = (patch: Partial<RemoteDialogState>) =>
    setDialog((current) => (current ? { ...current, ...patch } : current));

  const close = () => setDialog(null);

  const effectivePort = (current: RemoteDialogState): number => {
    const typed = Number(current.port.trim());
    return current.port.trim() && Number.isFinite(typed)
      ? typed
      : DEFAULT_PORTS[current.protocol];
  };

  /** Schritt 1: Bei SFTP zuerst den Hostschlüssel abgleichen. Ist er schon
   * bekannt, wird durchgereicht; sonst muss der Benutzer bestätigen. */
  const submit = async () => {
    const current = dialog();
    if (!current || current.busy) return;
    if (!current.host.trim() || !current.username.trim() || !current.password) {
      update({ error: t("remote.required") });
      return;
    }
    if (current.port.trim() && !/^\d{1,5}$/.test(current.port.trim())) {
      update({ error: t("err.remote.port") });
      return;
    }
    update({ busy: true, error: null });
    try {
      if (current.protocol === "sftp") {
        const report = await remoteHostKeys(
          current.host.trim(),
          effectivePort(current),
        );
        if (!report.trusted) {
          update({ busy: false, fingerprints: report.fingerprints });
          return;
        }
      }
      await connect();
    } catch (error) {
      update({ busy: false, error: errMsg(error) });
    }
  };

  /** Schritt 2: Der Benutzer hat den Fingerabdruck bestätigt. */
  const trustAndConnect = async () => {
    const current = dialog();
    if (!current || current.busy) return;
    update({ busy: true, error: null, fingerprints: null });
    try {
      await remoteTrustHost(current.host.trim(), effectivePort(current));
      await connect();
    } catch (error) {
      update({ busy: false, error: errMsg(error) });
    }
  };

  /** Schritt 3: Einhängen und den aktiven Bereich dorthin führen. */
  const connect = async () => {
    const current = dialog();
    if (!current) return;
    const spec = toSpec(current);
    if (current.savePassword) {
      // Ein fehlgeschlagenes Sichern darf das Verbinden nicht verhindern.
      try {
        await saveRemotePassword(spec, current.password);
      } catch {
        /* Schlüsselbund nicht verfügbar: dann eben ohne. */
      }
    }
    // Ein rclone-Mount hält sein Ziel beim Start fest. Wenn im Dialog der
    // Remote-Pfad geändert wurde, darf der vorhandene Mount daher nicht
    // weiterverwendet werden: Er würde weiterhin die alte Server-Wurzel
    // zeigen. Ersetze ausschließlich den Mount desselben Lesezeichens, bevor
    // der neue Verbindungsweg aufgebaut wird.
    const previous = (await remoteMounts()).find(
      (mount) => mount.descriptor === remoteDescriptor(spec),
    );
    if (previous) await unmountRemote(previous.path);
    const mountPath = await mountRemote(
      spec,
      current.password,
      isInsecure(current.protocol),
    );
    saveRemoteProfile(spec);
    bumpVolumes();
    close();
    if (mountPath) {
      await loadPane(
        state.active,
        mountPath,
        spec.protocol === "sftp" ? { navigationRoot: mountPath } : {},
      );
    }
  };

  const loadPassword = async () => {
    const current = dialog();
    if (!current || current.busy) return;
    if (!current.host.trim() || !current.username.trim()) {
      update({ error: t("remote.hostUserRequired") });
      return;
    }
    try {
      const password = await loadRemotePassword(toSpec(current));
      if (!password) {
        update({ error: t("remote.passwordMissing") });
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
          onMouseDown={() => !current().busy && close()}
        >
          <div
            class="modal remote-dialog"
            role="dialog"
            aria-modal="true"
            aria-label={t("remote.title")}
            onMouseDown={(event) => event.stopPropagation()}
            tabIndex={-1}
            ref={(el) => queueMicrotask(() => el?.focus())}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.key === "Escape" && !current().busy) {
                event.preventDefault();
                close();
              }
            }}
          >
            <h2>{t("remote.title")}</h2>
            <Show
              when={current().fingerprints}
              fallback={
                <>
                  <p>{t("remote.description")}</p>
                  <label>
                    {t("remote.protocol")}
                    <select
                      value={current().protocol}
                      disabled={current().busy}
                      onChange={(event) =>
                        update({
                          protocol: event.currentTarget.value as RemoteProtocol,
                        })
                      }
                    >
                      <For each={PROTOCOLS}>
                        {(protocol) => (
                          <option value={protocol}>
                            {t(`remote.protocol.${protocol}`)}
                          </option>
                        )}
                      </For>
                    </select>
                  </label>
                  <Show when={isInsecure(current().protocol)}>
                    <p class="warning">{t("remote.insecureNote")}</p>
                  </Show>
                  <label>
                    {t("remote.host")}
                    <input
                      type="text"
                      placeholder="sftp.example.com"
                      value={current().host}
                      disabled={current().busy}
                      onInput={(event) =>
                        update({ host: event.currentTarget.value })
                      }
                    />
                  </label>
                  <label>
                    {t("remote.port", {
                      port: String(DEFAULT_PORTS[current().protocol]),
                    })}
                    <input
                      type="text"
                      inputmode="numeric"
                      value={current().port}
                      disabled={current().busy}
                      onInput={(event) =>
                        update({ port: event.currentTarget.value })
                      }
                    />
                  </label>
                  <label>
                    {t("remote.username")}
                    <input
                      type="text"
                      autocomplete="username"
                      value={current().username}
                      disabled={current().busy}
                      onInput={(event) =>
                        update({ username: event.currentTarget.value })
                      }
                    />
                  </label>
                  <label>
                    {t("remote.password")}
                    <input
                      type="password"
                      autocomplete="current-password"
                      value={current().password}
                      disabled={current().busy}
                      onInput={(event) =>
                        update({ password: event.currentTarget.value })
                      }
                      onKeyDown={(event) => {
                        if (event.key === "Enter") void submit();
                      }}
                    />
                  </label>
                  <label>
                    {t("remote.path")}
                    <input
                      type="text"
                      placeholder="/"
                      value={current().path}
                      disabled={current().busy}
                      onInput={(event) =>
                        update({ path: event.currentTarget.value })
                      }
                    />
                  </label>
                  <label>
                    {t("remote.label")}
                    <input
                      type="text"
                      placeholder={current().host || "sftp.example.com"}
                      value={current().label}
                      disabled={current().busy}
                      onInput={(event) =>
                        update({ label: event.currentTarget.value })
                      }
                    />
                  </label>
                  <label class="check-line">
                    <input
                      type="checkbox"
                      checked={current().savePassword}
                      disabled={current().busy}
                      onChange={(event) =>
                        update({ savePassword: event.currentTarget.checked })
                      }
                    />
                    {t("remote.savePassword")}
                  </label>
                </>
              }
            >
              {(fingerprints) => (
                <>
                  <p>
                    {t("remote.hostKeyPrompt", {
                      host: current().host.trim(),
                    })}
                  </p>
                  <ul class="fingerprint-list">
                    <For each={fingerprints()}>
                      {(line) => <li>{line}</li>}
                    </For>
                  </ul>
                  <p class="hint">{t("remote.hostKeyHint")}</p>
                </>
              )}
            </Show>
            <Show when={current().error}>
              <p class="error pre-wrap">{current().error}</p>
            </Show>
            <div class="modal-actions">
              <Show
                when={current().fingerprints}
                fallback={
                  <button
                    disabled={current().busy}
                    onClick={() => void submit()}
                  >
                    {current().busy ? t("remote.connecting") : t("remote.connect")}
                  </button>
                }
              >
                <button
                  disabled={current().busy}
                  onClick={() => void trustAndConnect()}
                >
                  {current().busy
                    ? t("remote.connecting")
                    : t("remote.hostKeyTrust")}
                </button>
              </Show>
              <Show when={!current().fingerprints}>
                <button
                  class="secondary"
                  disabled={current().busy}
                  onClick={() => void loadPassword()}
                >
                  {t("remote.loadPassword")}
                </button>
              </Show>
              <button
                class="secondary"
                disabled={current().busy}
                onClick={close}
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
