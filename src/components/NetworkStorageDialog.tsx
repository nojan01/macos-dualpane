import { Show, createSignal } from "solid-js";
import { mountNetworkUrl, rememberNetworkVolume } from "../ipc";
import { bumpVolumes, loadPane, state } from "../state";
import { errMsg, t } from "../i18n";
import { emptyObjectStorageProfile } from "../objectStorageProfiles";
import { openObjectStorageDialog } from "./ObjectStorageDialog";
import { openNfsDialog } from "./NfsDialog";
import { openRemoteDialog } from "./RemoteDialog";
import { notifyError } from "./Dialogs";

type Protocol = "webdav" | "smb" | "nfs" | "s3" | "swift" | "sftp" | "ftps";
type DialogState = { protocol: Protocol; webdavUrl: string; busy: boolean };

const [dialog, setDialog] = createSignal<DialogState | null>(null);

/** Zentraler Einstieg für alle Netzwerkziele unter „Netzwerk“. */
export function openNetworkStorageDialog(webdavUrl = "https://") {
  setDialog({ protocol: "webdav", webdavUrl, busy: false });
}

export function NetworkStorageDialog() {
  const close = () => setDialog(null);
  const update = (patch: Partial<DialogState>) =>
    setDialog((current) => (current ? { ...current, ...patch } : current));

  const continueToObjectProfile = () => {
    const current = dialog();
    if (!current || current.busy) return;
    const profile = emptyObjectStorageProfile();
    profile.protocol = current.protocol === "swift" ? "swift" : "s3";
    close();
    openObjectStorageDialog(profile);
  };

  const continueToNfs = () => {
    const current = dialog();
    if (!current || current.busy) return;
    close();
    openNfsDialog();
  };

  const continueToRemote = () => {
    const current = dialog();
    if (!current || current.busy) return;
    close();
    // SMB lief früher über den Finder („mount volume"). Der meldet für
    // Windows-Freigaben nur den Sammelfehler -5016 und kommt nie bis zur
    // Anmeldung. Deshalb geht SMB denselben Weg wie SFTP und FTPS.
    openRemoteDialog({
      protocol:
        current.protocol === "ftps"
          ? "ftpsExplicit"
          : current.protocol === "smb"
            ? "smb"
            : "sftp",
    });
  };

  const connectWebDav = async () => {
    const current = dialog();
    if (!current || current.busy) return;
    const url = current.webdavUrl.trim();
    if (!/^https:\/\/.+/i.test(url)) {
      await notifyError(t("network.invalidWebdav"));
      return;
    }
    update({ busy: true });
    try {
      const mountPath = await mountNetworkUrl(url);
      // Neue WebDAV- und SMB-Ziele erhalten sofort ein Lesezeichen. Damit
      // zeigen sie ohne den Umweg über ein erstes Aushängen dieselben Aktionen
      // wie bereits bekannte Netzwerk-Laufwerke.
      if (mountPath) await rememberNetworkVolume(mountPath);
      bumpVolumes();
      close();
      if (mountPath) await loadPane(state.active, mountPath);
    } catch (error) {
      update({ busy: false });
      await notifyError(errMsg(error));
    }
  };

  return <Show when={dialog()}>{(current) => <div class="modal-backdrop" onMouseDown={() => !current().busy && close()}>
    <div class="modal network-storage-modal" role="dialog" aria-modal="true" aria-label={t("network.addDrive")} onMouseDown={(event) => event.stopPropagation()}>
      <h2>{current().webdavUrl === "https://" ? t("network.addDrive") : t(`network.title.${current().protocol}`)}</h2>
      <p>{t("network.chooseProtocol")}</p>
      <label>
        <span>Protokoll</span>
        <select value={current().protocol} disabled={current().busy} onChange={(event) => update({ protocol: event.currentTarget.value as Protocol })}>
          <option value="webdav">WebDAV</option>
          <option value="smb">SMB / Samba</option>
          <option value="nfs">NFS</option>
          <option value="s3">S3 (Objekt-Speicher)</option>
          <option value="swift">OpenStack Swift</option>
          <option value="sftp">SFTP</option>
          <option value="ftps">FTPS</option>
        </select>
      </label>
      <Show when={current().protocol === "webdav"} fallback={<p class="network-storage-note">{t(current().protocol === "nfs" ? "network.nextNfs" : "network.nextCredentials")}</p>}>
        <label>
          <span>{t("network.webdavAddress")}</span>
          <input type="url" autofocus disabled={current().busy} placeholder="https://webdav.example.net/" value={current().webdavUrl} onInput={(event) => update({ webdavUrl: event.currentTarget.value })} onKeyDown={(event) => { if (event.key === "Enter") void connectWebDav(); }} />
        </label>
      </Show>
      <div class="modal-actions"><span /><button disabled={current().busy} onClick={() => (current().protocol === "webdav") ? void connectWebDav() : current().protocol === "nfs" ? continueToNfs() : (current().protocol === "s3" || current().protocol === "swift") ? continueToObjectProfile() : continueToRemote()}>{current().protocol === "webdav" ? current().busy ? t("network.connecting") : t("network.connect") : t("network.next")}</button><button class="secondary" disabled={current().busy} onClick={close}>{t("common.cancel")}</button></div>
    </div>
  </div>}</Show>;
}
