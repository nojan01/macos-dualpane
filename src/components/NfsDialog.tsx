import { For, Show, createSignal } from "solid-js";
import { bumpVolumes, loadPane, state } from "../state";
import {
  mountNfs,
  type NfsSecurity,
  type NfsSpec,
  type NfsTransport,
  type NfsVersion,
} from "../ipc";
import { errMsg, t } from "../i18n";
import { saveNfsProfile } from "../nfsProfiles";

/** Reihenfolge im Auswahlfeld: die ausgehandelte Voreinstellung zuerst. */
const VERSIONS: NfsVersion[] = ["auto", "v3", "v4", "v41", "v2"];

/** Das sicherste Verfahren steht nach „automatisch“ an erster Stelle. */
const SECURITIES: NfsSecurity[] = ["auto", "krb5p", "krb5i", "krb5", "sys"];

const TRANSPORTS: NfsTransport[] = ["auto", "tcp", "udp"];

type NfsDialogState = {
  host: string;
  path: string;
  version: NfsVersion;
  security: NfsSecurity;
  realm: string;
  transport: NfsTransport;
  noLocks: boolean;
  label: string;
  allowInsecure: boolean;
  busy: boolean;
  error: string | null;
};

const [dialog, setDialog] = createSignal<NfsDialogState | null>(null);

/** Nur `krb5p` verschlüsselt die übertragenen Dateiinhalte. */
const encrypts = (security: NfsSecurity) => security === "krb5p";

const usesKerberos = (security: NfsSecurity) =>
  security === "krb5" || security === "krb5i" || security === "krb5p";

export function openNfsDialog(preset: Partial<NfsDialogState> = {}) {
  setDialog({
    host: "",
    path: "",
    version: "auto",
    security: "auto",
    realm: "",
    transport: "auto",
    noLocks: false,
    label: "",
    allowInsecure: false,
    busy: false,
    error: null,
    ...preset,
  });
}

export function NfsDialog() {
  const close = () => setDialog(null);
  const update = (patch: Partial<NfsDialogState>) =>
    setDialog((current) => (current ? { ...current, ...patch } : current));

  const submit = async () => {
    const current = dialog();
    if (!current || current.busy) return;
    const host = current.host.trim();
    const path = current.path.trim();
    if (!host || !path.startsWith("/")) {
      update({ error: t("nfs.needHostAndPath") });
      return;
    }
    const spec: NfsSpec = {
      host,
      path,
      version: current.version,
      security: current.security,
      realm: current.realm.trim(),
      transport: current.transport,
      noLocks: current.noLocks,
      label: current.label.trim(),
      // Verschlüsselte Verbindungen brauchen keine gesonderte Zustimmung.
      allowInsecure: current.allowInsecure || encrypts(current.security),
    };
    update({ busy: true, error: null });
    try {
      // Kein Netzwerk-Lesezeichen: Dieses Laufwerk hängt DualBeam selbst ein,
      // seine Mount-Quelle lautet "host:/export" und taugt nicht als URL zum
      // erneuten Verbinden. Es wird über register_plain_mount als eigene Art
      // "remote" geführt und erscheint darüber in der Seitenleiste.
      const mountPath = await mountNfs(spec);
      // Erst nach dem geglückten Einhängen merken: Ein Lesezeichen, das gar
      // nicht verbindet, wäre in der Seitenleiste nur eine Sackgasse.
      saveNfsProfile(spec);
      bumpVolumes();
      close();
      if (mountPath) await loadPane(state.active, mountPath);
    } catch (error) {
      update({ busy: false, error: errMsg(error) });
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
            aria-label={t("nfs.title")}
            onMouseDown={(event) => event.stopPropagation()}
          >
            <h2>{t("nfs.title")}</h2>
            <label>
              {t("nfs.host")}
              <input
                type="text"
                autofocus
                placeholder="192.168.1.10"
                value={current().host}
                disabled={current().busy}
                onInput={(event) => update({ host: event.currentTarget.value })}
              />
            </label>
            <label>
              {t("nfs.path")}
              <input
                type="text"
                placeholder="/export/daten"
                value={current().path}
                disabled={current().busy}
                onInput={(event) => update({ path: event.currentTarget.value })}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void submit();
                }}
              />
            </label>
            <label>
              {t("nfs.version")}
              <select
                value={current().version}
                disabled={current().busy}
                onChange={(event) =>
                  update({ version: event.currentTarget.value as NfsVersion })
                }
              >
                <For each={VERSIONS}>
                  {(version) => (
                    <option value={version}>{t(`nfs.version.${version}`)}</option>
                  )}
                </For>
              </select>
            </label>
            <label>
              {t("nfs.security")}
              <select
                value={current().security}
                disabled={current().busy}
                onChange={(event) =>
                  update({
                    security: event.currentTarget.value as NfsSecurity,
                    // Ein Wechsel weg von Kerberos macht den Bereich gegenstandslos.
                    realm: usesKerberos(event.currentTarget.value as NfsSecurity)
                      ? current().realm
                      : "",
                  })
                }
              >
                <For each={SECURITIES}>
                  {(security) => (
                    <option value={security}>
                      {t(`nfs.security.${security}`)}
                    </option>
                  )}
                </For>
              </select>
            </label>
            <Show when={usesKerberos(current().security)}>
              <label>
                {t("nfs.realm")}
                <input
                  type="text"
                  placeholder="BEISPIEL.DE"
                  value={current().realm}
                  disabled={current().busy}
                  onInput={(event) =>
                    update({ realm: event.currentTarget.value })
                  }
                />
              </label>
              <p class="hint">{t("nfs.realmHint")}</p>
            </Show>
            <label>
              {t("nfs.transport")}
              <select
                value={current().transport}
                disabled={current().busy}
                onChange={(event) =>
                  update({
                    transport: event.currentTarget.value as NfsTransport,
                  })
                }
              >
                <For each={TRANSPORTS}>
                  {(transport) => (
                    <option value={transport}>
                      {t(`nfs.transport.${transport}`)}
                    </option>
                  )}
                </For>
              </select>
            </label>
            <label>
              {t("nfs.label")}
              <input
                type="text"
                placeholder={current().host || "nfs-freigabe"}
                value={current().label}
                disabled={current().busy}
                onInput={(event) => update({ label: event.currentTarget.value })}
              />
            </label>
            <label class="check-line">
              <input
                type="checkbox"
                checked={current().noLocks}
                disabled={current().busy}
                onChange={(event) =>
                  update({ noLocks: event.currentTarget.checked })
                }
              />
              {t("nfs.noLocks")}
            </label>
            <p class="hint">{t("nfs.noLocksHint")}</p>
            <Show when={!encrypts(current().security)}>
              <p class="warning">
                {usesKerberos(current().security)
                  ? t("nfs.kerberosPlainNote")
                  : t("nfs.insecureNote")}
              </p>
              <label class="check-line">
                <input
                  type="checkbox"
                  checked={current().allowInsecure}
                  disabled={current().busy}
                  onChange={(event) =>
                    update({ allowInsecure: event.currentTarget.checked })
                  }
                />
                {t("nfs.acceptInsecure")}
              </label>
            </Show>
            <p class="hint">{t("nfs.privilegedPortHint")}</p>
            <Show when={current().error}>
              <p class="error pre-wrap">{current().error}</p>
            </Show>
            <div class="modal-actions">
              <button disabled={current().busy} onClick={() => void submit()}>
                {current().busy ? t("nfs.connecting") : t("nfs.connect")}
              </button>
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
