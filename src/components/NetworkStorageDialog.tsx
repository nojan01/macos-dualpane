import { Show, createSignal } from "solid-js";
import { t } from "../i18n";
import { emptyObjectStorageProfile } from "../objectStorageProfiles";
import { webdavPresetFromUrl } from "../remoteProfiles";
import { openObjectStorageDialog } from "./ObjectStorageDialog";
import { openNfsDialog } from "./NfsDialog";
import { openRemoteDialog } from "./RemoteDialog";

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
    //
    // WebDAV ging denselben Weg und litt an einem anderen Mangel: Der Finder
    // fragte Benutzer und Kennwort in einem eigenen Fenster ab. DualBeam
    // konnte deshalb weder Anbieter noch Adresspfad anbieten, legte kein
    // Lesezeichen an, und die Freigabe landete unter /Volumes statt im
    // App-Ordner — womit Löschschutz und Übertragungswege nicht griffen.
    if (current.protocol === "webdav") {
      // Eine bereits eingetippte Adresse — oder die eines alten
      // Finder-Lesezeichens — wird übernommen, damit niemand alles neu tippt.
      const preset = webdavPresetFromUrl(current.webdavUrl);
      openRemoteDialog({ protocol: "webdav", ...preset });
      return;
    }
    openRemoteDialog({
      protocol:
        current.protocol === "ftps"
          ? "ftpsExplicit"
          : current.protocol === "smb"
            ? "smb"
            : "sftp",
    });
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
          <input type="url" autofocus disabled={current().busy} placeholder="https://webdav.example.net/" value={current().webdavUrl} onInput={(event) => update({ webdavUrl: event.currentTarget.value })} onKeyDown={(event) => { if (event.key === "Enter") continueToRemote(); }} />
        </label>
        <p class="network-storage-note">{t("network.nextCredentials")}</p>
      </Show>
      <div class="modal-actions"><span /><button disabled={current().busy} onClick={() => current().protocol === "nfs" ? continueToNfs() : (current().protocol === "s3" || current().protocol === "swift") ? continueToObjectProfile() : continueToRemote()}>{t("network.next")}</button><button class="secondary" disabled={current().busy} onClick={close}>{t("common.cancel")}</button></div>
    </div>
  </div>}</Show>;
}
