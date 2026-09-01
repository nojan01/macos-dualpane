import { Show, createSignal } from "solid-js";
import {
  emptyObjectStorageProfile,
  removeObjectStorageProfile,
  saveObjectStorageProfile,
  type ObjectStorageProfile,
} from "../objectStorageProfiles";
import {
  forgetObjectStorageSecret,
  hasObjectStorageSecret,
  saveObjectStorageSecret,
} from "../ipc";
import { t } from "../i18n";
import { askConfirm, notifyError } from "./Dialogs";

const [editing, setEditing] = createSignal<ObjectStorageProfile | null>(null);
const [secret, setSecret] = createSignal("");
const [hasStoredSecret, setHasStoredSecret] = createSignal(false);

export function openObjectStorageDialog(profile?: ObjectStorageProfile) {
  const next = profile ? { ...profile } : emptyObjectStorageProfile();
  setEditing(next);
  setSecret("");
  setHasStoredSecret(false);
  void hasObjectStorageSecret(next.id).then(
    setHasStoredSecret,
    () => setHasStoredSecret(false),
  );
}

export function ObjectStorageDialog() {
  const update = <K extends keyof ObjectStorageProfile>(
    key: K,
    value: ObjectStorageProfile[K],
  ) => {
    const current = editing();
    if (current) setEditing({ ...current, [key]: value });
  };
  const close = () => setEditing(null);

  const save = async () => {
    const profile = editing();
    if (!profile) return;
    const valid = profile.name.trim() && profile.endpoint.trim() &&
      (profile.protocol === "s3"
        ? profile.accessKey.trim() && profile.region.trim()
        : profile.username.trim() && profile.swiftProject.trim() &&
          profile.swiftIdentityPath.startsWith("/"));
    if (!valid || (!hasStoredSecret() && !secret())) {
      await notifyError(t("object.required"));
      return;
    }
    try {
      if (secret()) await saveObjectStorageSecret(profile.id, secret());
      saveObjectStorageProfile({
        ...profile,
        name: profile.name.trim(),
        endpoint: profile.endpoint.trim(),
        container: profile.container.trim(),
      });
      setHasStoredSecret(true);
      close();
    } catch (error) {
      await notifyError(String(error));
    }
  };

  const remove = async () => {
    const profile = editing();
    if (!profile) return;
    const confirmed = await askConfirm({
      title: t("object.removeTitle"),
      message: t("object.removeMessage", { name: profile.name || t("object.profile") }),
      okLabel: t("common.delete"),
      danger: true,
    });
    if (!confirmed) return;
    try {
      await forgetObjectStorageSecret(profile.id);
      removeObjectStorageProfile(profile.id);
      close();
    } catch (error) {
      await notifyError(String(error));
    }
  };

  return <Show when={editing()}>{(profile) => <div class="modal-backdrop" onMouseDown={close}>
    <div class="modal object-profile-modal" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()}>
      <h2>{profile().name ? t("object.profile") : t("object.add")}</h2>
      <p>{t("object.intro")}</p>
      <div class="object-profile-grid">
        <label><span>{t("object.name")}</span><input value={profile().name} onInput={(e) => update("name", e.currentTarget.value)} /></label>
        <label><span>{t("object.protocol")}</span><select value={profile().protocol} onChange={(e) => update("protocol", e.currentTarget.value as "s3" | "swift")}><option value="s3">S3</option><option value="swift">OpenStack Swift</option></select></label>
        <label class="wide"><span>{profile().protocol === "s3" ? t("object.s3Endpoint") : t("object.swiftEndpoint")}</span><input placeholder={profile().protocol === "s3" ? "https://s3.example.net" : "https://swift.example.net"} value={profile().endpoint} onInput={(e) => update("endpoint", e.currentTarget.value)} /></label>
        <Show when={profile().protocol === "s3"} fallback={<>
          <label><span>{t("object.keystoneVersion")}</span><select value={profile().swiftAuthVersion} onChange={(e) => { const version = e.currentTarget.value as "v2" | "v3"; update("swiftAuthVersion", version); update("swiftIdentityPath", version === "v3" ? "/identity/v3" : "/v2.0"); }}><option value="v3">Keystone v3</option><option value="v2">Keystone v2 (Legacy)</option></select></label>
          <label><span>{t("object.username")}</span><input value={profile().username} onInput={(e) => update("username", e.currentTarget.value)} /></label>
          <label><span>{profile().swiftAuthVersion === "v2" ? t("object.tenant") : t("object.project")}</span><input value={profile().swiftProject} onInput={(e) => update("swiftProject", e.currentTarget.value)} /></label>
          <label><span>{t("object.region")}</span><input value={profile().region} onInput={(e) => update("region", e.currentTarget.value)} /></label>
          <label><span>{t("object.keystonePath")}</span><input value={profile().swiftIdentityPath} onInput={(e) => update("swiftIdentityPath", e.currentTarget.value)} /></label>
          <Show when={profile().swiftAuthVersion === "v3"}><label><span>{t("object.userDomain")}</span><input value={profile().swiftUserDomain} onInput={(e) => update("swiftUserDomain", e.currentTarget.value)} /></label><label><span>{t("object.projectDomain")}</span><input value={profile().swiftProjectDomain} onInput={(e) => update("swiftProjectDomain", e.currentTarget.value)} /></label></Show>
        </>}>
          <label><span>Access Key</span><input value={profile().accessKey} onInput={(e) => update("accessKey", e.currentTarget.value)} /></label>
          <label><span>{t("object.region")}</span><input value={profile().region} onInput={(e) => update("region", e.currentTarget.value)} /></label>
        </Show>
        <label><span>{t("object.containerBucket")}</span><input placeholder={t("object.allContainers")} value={profile().container} onInput={(e) => update("container", e.currentTarget.value)} /></label>
        <label><span>{t("object.parallelTransfers")}</span><select value={profile().parallelTransfers} onChange={(e) => update("parallelTransfers", e.currentTarget.value === "4" ? 4 : 1)}><option value="1">{t("object.parallelTransfersSafe")}</option><option value="4">{t("object.parallelTransfersFast")}</option></select><small>{t("object.parallelTransfersHint")}</small></label>
        <label><span>{profile().protocol === "s3" ? t("object.s3Secret") : t("object.swiftPassword")}</span><input type="password" placeholder={hasStoredSecret() ? t("object.secretStoredKeep") : t("object.secretWillStore")} value={secret()} onInput={(e) => setSecret(e.currentTarget.value)} /><Show when={hasStoredSecret()} fallback={<small class="object-secret-status missing">{t("object.secretMissing")}</small>}><small class="object-secret-status">{t("object.secretStored")}</small></Show></label>
        <Show when={profile().protocol === "s3"}><label class="object-profile-toggle wide"><input type="checkbox" checked={profile().pathStyle} onChange={(e) => update("pathStyle", e.currentTarget.checked)} /> {t("object.pathStyle")}</label></Show>
      </div>
      <div class="modal-actions"><Show when={profile().name}><button class="danger" onClick={() => void remove()}>{t("common.delete")}</button></Show><span /><button onClick={() => void save()}>{t("common.save")}</button><button class="secondary" onClick={close}>{t("common.cancel")}</button></div>
    </div>
  </div>}</Show>;
}
