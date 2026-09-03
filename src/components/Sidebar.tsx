import {
  For,
  Show,
  createEffect,
  createSignal,
  onMount,
  onCleanup,
} from "solid-js";
import { state, loadPane, volumesTick, handleVolumeGone, bumpVolumes } from "../state";
import {
  homeDir,
  listVolumes,
  ejectVolume,
  loadFavorites,
  saveFavorites,
  listNetworkBookmarks,
  removeNetworkBookmark,
  rememberNetworkVolume,
  mountNetworkUrl,
  mountObjectStorage,
  loadRemotePassword,
  mountNfs,
  mountRemote,
  remoteMounts,
  unmountRemote,
  forgetObjectStorageSecret,
  importRemoteDeskObjectStorageProfiles,
  rdpProfiles,
  rdpConnect,
  type Volume,
  type Favorite,
  type NetworkBookmark,
  type RemoteMount,
  type RdpProfile,
} from "../ipc";
import { askConfirm, notify, notifyError } from "./Dialogs";
import { openNetworkStorageDialog } from "./NetworkStorageDialog";
import { t, errMsg } from "../i18n";
import { runSyncProfile } from "../sync";
import { syncProfiles } from "../syncProfiles";
import {
  mergeObjectStorageProfiles,
  objectStorageProfiles,
  removeObjectStorageProfile,
  type ObjectStorageProfile,
} from "../objectStorageProfiles";
import { openObjectStorageDialog } from "./ObjectStorageDialog";
import { openRemoteDialog } from "./RemoteDialog";
import { openNfsDialog } from "./NfsDialog";
import { remoteDescriptor, remoteFromDescriptor, remoteProfiles, removeRemoteProfile, type RemoteProfile } from "../remoteProfiles";
import { nfsDescriptor, nfsProfiles, removeNfsProfile, type NfsProfile } from "../nfsProfiles";

function basename(p: string): string {
  const trimmed = p.endsWith("/") ? p.slice(0, -1) : p;
  const i = trimmed.lastIndexOf("/");
  return i < 0 ? trimmed : trimmed.slice(i + 1) || "/";
}

/** Alles, was kein Netzlaufwerk ist – also die eigentlichen Datenträger.
 * „remote“ steht für die selbst über rclone eingehängten Ziele. */
function isLocalVolume(vol: Volume): boolean {
  return vol.kind !== "network" && vol.kind !== "remote";
}

type Menu = { idx: number; x: number; y: number } | null;
type VolMenu = { vol: Volume; x: number; y: number } | null;

export function Sidebar() {
  const [favs, setFavs] = createSignal<Favorite[]>([]);
  const [vols, setVols] = createSignal<Volume[]>([]);
  const [bookmarks, setBookmarks] = createSignal<NetworkBookmark[]>([]);
  const [remoteMountList, setRemoteMountList] = createSignal<RemoteMount[]>([]);
  const [mounting, setMounting] = createSignal<string | null>(null);
  const [editIdx, setEditIdx] = createSignal<number | null>(null);
  const [editValue, setEditValue] = createSignal("");
  const [menu, setMenu] = createSignal<Menu>(null);
  const [volMenu, setVolMenu] = createSignal<VolMenu>(null);
  const [dragIdx, setDragIdx] = createSignal<number | null>(null);
  const [overIdx, setOverIdx] = createSignal<number | null>(null);
  const [rdp, setRdp] = createSignal<RdpProfile[]>([]);
  /** Läuft gerade ein Verbindungsaufbau? Dann ist die Liste gesperrt. */
  const [connectingRdp, setConnectingRdp] = createSignal<string | null>(null);
  let rdpUnlock: number | undefined;
  onCleanup(() => window.clearTimeout(rdpUnlock));

  /** Liest die in RemoteDeskRDP eingerichteten Verbindungen. Fehlt die App,
   *  kommt eine leere Liste und der Abschnitt bleibt unsichtbar. */
  async function refreshRdp() {
    try {
      setRdp(await rdpProfiles());
    } catch {
      setRdp([]);
    }
  }

  const refreshRdpOnFocus = () => void refreshRdp();

  /** Startet die Sitzung in RemoteDeskRDP. DualBeam selbst spricht kein RDP. */
  async function openRdp(profile: RdpProfile) {
    // Sperre gegen den zweiten Klick. RemoteDeskRDP braucht einige Sekunden,
    // bis das Sitzungsfenster steht; ohne Rückmeldung klickt man erneut. Eine
    // zweite Sitzung zum selben Ziel ist aber schädlich: Der RDP-Server lässt
    // je Benutzer nur eine zu und trennt die ältere ohne jede Meldung – das
    // Fenster verschwindet dann einfach. Im Systemprotokoll nachgewiesen.
    if (connectingRdp()) return;
    setConnectingRdp(profile.id);
    try {
      await rdpConnect(profile.id);
      // Die Sperre läuft bewusst nach: `rdpConnect` kehrt bereits zurück,
      // sobald macOS den Start angenommen hat – lange bevor die Sitzung steht.
      rdpUnlock = window.setTimeout(() => setConnectingRdp(null), 4000);
    } catch (err) {
      setConnectingRdp(null);
      await notifyError(errMsg(err));
    }
  }

  async function refreshVols() {
    try {
      setVols(await listVolumes());
    } catch {
      setVols([]);
    }
    try {
      setBookmarks(await listNetworkBookmarks());
    } catch {
      setBookmarks([]);
    }
    try {
      setRemoteMountList(await remoteMounts());
    } catch {
      setRemoteMountList([]);
    }
  }

  /** Übernimmt beim Start die bisher in RemoteDeskRDP gespeicherten S3- und
   * Swift-Verbindungen. Die Profil-ID bleibt gleich, damit das Geheimnis aus
   * dem Schlüsselbund ohne erneute Eingabe weiterverwendet werden kann. */
  async function importRemoteDeskProfiles() {
    try {
      mergeObjectStorageProfiles(await importRemoteDeskObjectStorageProfiles());
    } catch {
      // RemoteDeskRDP ist optional. Ein fehlendes oder altes Profil stört
      // DualBeam nicht und darf den Start der Seitenleiste nicht blockieren.
    }
  }

  async function mountObjectProfile(profile: ObjectStorageProfile) {
    if (mounting()) return;
    const existing = objectMount(profile);
    if (existing) {
      await openObjectHome(profile, existing);
      return;
    }
    setMounting(profile.id);
    try {
      const mountPath = await mountObjectStorage(profile);
      await refreshVols();
      const mount = objectMount(profile);
      if (mount) await openObjectHome(profile, mount);
      else await loadPane(state.active, mountPath, { navigationRoot: mountPath });
    } catch (err) {
      await askConfirm({
        title: t("sidebar.mountFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  /** Der Descriptor ist eine stabile Profil-ID. Über den Namen zuzuordnen
   * würde bei zwei gleich benannten S3-Buckets wieder Doppelungen erzeugen. */
  const objectMount = (profile: ObjectStorageProfile) =>
    remoteMountList().find(
      (mount) => mount.descriptor === `${profile.protocol}://${profile.id}`,
    );

  /** Der Backend-Mount meldet den tatsächlichen sichtbaren Einstieg: einen
   * explizit konfigurierten Container oder bei einem Konto-Profil den
   * vorhandenen Standardcontainer `default`. */
  function objectHomePath(_profile: ObjectStorageProfile, mount: RemoteMount): string {
    return mount.homePath ?? mount.path;
  }

  async function openObjectHome(profile: ObjectStorageProfile, mount: RemoteMount) {
    const path = objectHomePath(profile, mount);
    await loadPane(state.active, path, { navigationRoot: path });
  }

  /** Macht die nicht-sensitive rclone-Kennung wieder als Vorbelegung für den
   * SFTP/FTPS-Dialog nutzbar. Das Kennwort bleibt im Schlüsselbund. */
  function editRemoteMount(mount: RemoteMount) {
    // Gespeicherte Profile enthalten zusätzlich den gewünschten Remote-
    // Startordner (z. B. `default`). Der technische Mount-Descriptor enthält
    // diesen Pfad absichtlich nicht. Deshalb zuerst immer das vollständige
    // Lesezeichen verwenden, damit der Dialog den tatsächlich gespeicherten
    // Pfad zeigt und beim erneuten Verbinden auch genau dort einhängt.
    const profile = remoteProfiles().find(
      (item) => remoteDescriptor(item) === mount.descriptor,
    );
    if (profile) {
      openRemoteDialog({ ...profile, port: profile.port?.toString() ?? "" });
      return;
    }
    // Gibt es zu diesem Laufwerk ein NFS-Lesezeichen, stammen Fassung,
    // Sicherheitsverfahren und Übertragungsart von dort.
    const savedNfs = nfsProfiles().find((item) => nfsDescriptor(item) === mount.descriptor);
    if (savedNfs) {
      openNfsDialog({ ...savedNfs });
      return;
    }
    // Ohne Lesezeichen bleiben nur Server und Freigabepfad aus der Kennung;
    // die übrigen Einstellungen stehen wieder auf „Automatisch“. NFS kennt
    // weder Benutzer noch Port, deshalb fehlt hier beides.
    const nfs = /^nfs:\/\/([^/]+)(\/.*)$/.exec(mount.descriptor);
    if (nfs) {
      openNfsDialog({ host: nfs[1], path: nfs[2], label: mount.label });
      return;
    }
    // Die Zerlegung leitet sich aus derselben Zuordnung ab, die auch die
    // Kennung bildet. Ein künftig ergänztes Protokoll ist damit von selbst
    // abgedeckt, statt hier vergessen zu werden.
    const found = remoteFromDescriptor(mount.descriptor);
    if (!found) return;
    openRemoteDialog({
      protocol: found.protocol,
      username: found.username,
      host: found.host,
      port: found.port.toString(),
      label: mount.label,
    });
  }

  const savedRemoteMount = (profile: RemoteProfile) =>
    remoteMountList().find((mount) => mount.descriptor === remoteDescriptor(profile));

  async function openSavedRemoteProfile(profile: RemoteProfile) {
    if (mounting()) return;
    // Ein bereits verbundenes SFTP-Lesezeichen öffnet immer dessen sichtbare
    // Wurzel. Nur den technischen Sidebar-Eintrag als „verbunden“ zu zeigen,
    // während die Pane auf dem vorherigen lokalen Ordner bleibt, ist für den
    // Benutzer nicht nachvollziehbar und war die Ursache des falschen
    // Eindrucks eines Home-Verzeichnis-Mounts.
    const activeMount = savedRemoteMount(profile);
    if (activeMount) {
      await loadPane(
        state.active,
        activeMount.path,
        { navigationRoot: activeMount.path },
      );
      return;
    }
    setMounting(profile.id);
    try {
      const password = await loadRemotePassword(profile);
      if (!password) {
        openRemoteDialog({ ...profile, port: profile.port?.toString() ?? "" });
        return;
      }
      // „Wiederverbinden“ muss den alten rclone-Prozess ersetzen. Andernfalls
      // bliebe dessen beim Start festgelegter Remote-Pfad aktiv und ein
      // geändertes `default` würde nie sichtbar.
      const previous = savedRemoteMount(profile);
      if (previous) await unmountRemote(previous.path);
      const mountPath = await mountRemote(
        profile,
        password,
        profile.protocol === "ftp",
      );
      bumpVolumes();
      await refreshVols();
      await loadPane(
        state.active,
        mountPath,
        { navigationRoot: mountPath },
      );
    } catch (error) {
      // Der Dialog bleibt der sichere Ausweichweg, z. B. für einen geänderten
      // Hostschlüssel oder ein abgelaufenes Kennwort.
      openRemoteDialog({ ...profile, port: profile.port?.toString() ?? "" });
      await notifyError(errMsg(error));
    } finally {
      setMounting(null);
    }
  }

  /** Ein bewusstes Wiederverbinden ersetzt den rclone-Mount; ein normaler
   * Klick auf den verbundenen Eintrag navigiert dagegen nur in dessen Wurzel. */
  async function reconnectSavedRemoteProfile(profile: RemoteProfile) {
    if (mounting()) return;
    const activeMount = savedRemoteMount(profile);
    if (activeMount) {
      setMounting(profile.id);
      try {
        await unmountRemote(activeMount.path);
        bumpVolumes();
        await refreshVols();
      } finally {
        setMounting(null);
      }
    }
    await openSavedRemoteProfile(profile);
  }

  async function removeSavedRemote(profile: RemoteProfile) {
    const mount = savedRemoteMount(profile);
    if (mount) await ejectVolume(mount.path);
    removeRemoteProfile(profile.id);
    await refreshVols();
  }

  const savedNfsMount = (profile: NfsProfile) =>
    remoteMountList().find((mount) => mount.descriptor === nfsDescriptor(profile));

  async function openSavedNfsProfile(profile: NfsProfile) {
    if (mounting()) return;
    const activeMount = savedNfsMount(profile);
    if (activeMount) {
      await loadPane(state.active, activeMount.path);
      return;
    }
    setMounting(profile.id);
    try {
      const mountPath = await mountNfs(profile);
      bumpVolumes();
      await refreshVols();
      if (mountPath) await loadPane(state.active, mountPath);
    } catch (error) {
      // NFS kennt kein Kennwort; ein Fehlschlag liegt an Server, Netz oder den
      // Freigaberechten. Der Dialog zeigt den Grund und erlaubt, die
      // Einstellungen zu ändern, ohne alles neu einzutippen.
      openNfsDialog({ ...profile });
      await notifyError(errMsg(error));
    } finally {
      setMounting(null);
    }
  }

  async function reconnectSavedNfsProfile(profile: NfsProfile) {
    if (mounting()) return;
    const activeMount = savedNfsMount(profile);
    if (activeMount) {
      setMounting(profile.id);
      try {
        await unmountRemote(activeMount.path);
        bumpVolumes();
        await refreshVols();
      } finally {
        setMounting(null);
      }
    }
    await openSavedNfsProfile(profile);
  }

  async function removeSavedNfs(profile: NfsProfile) {
    const mount = savedNfsMount(profile);
    if (mount) await ejectVolume(mount.path);
    removeNfsProfile(profile.id);
    await refreshVols();
  }

  /** Stufe 1 wie bei WebDAV: nur aushängen, das Profil bleibt erhalten. */
  async function ejectObjectProfile(profile: ObjectStorageProfile) {
    const mount = objectMount(profile);
    if (!mount || mounting()) return;
    const confirmed = await askConfirm({
      title: t("sidebar.unmount"),
      message: t("sidebar.unmountConfirm", { name: profile.name }),
      okLabel: t("sidebar.unmount"),
      danger: true,
    });
    if (!confirmed) return;
    setMounting(profile.id);
    try {
      await ejectVolume(mount.path);
      await handleVolumeGone(mount.path);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function reconnectObjectProfile(profile: ObjectStorageProfile) {
    const mount = objectMount(profile);
    if (!mount || mounting()) return;
    setMounting(profile.id);
    try {
      await ejectVolume(mount.path);
      await handleVolumeGone(mount.path);
      await refreshVols();
    } catch {
      // Das Wiederverbinden darf auch nach einem bereits verlorenen Mount
      // funktionieren; der anschließende Verbindungsaufbau entscheidet.
    } finally {
      setMounting(null);
    }
    await mountObjectProfile(profile);
  }

  /** Stufe 2 wie bei WebDAV: aushängen und das Profil samt Keychain-Secret
   * dauerhaft entfernen. */
  async function removeObjectProfile(profile: ObjectStorageProfile) {
    if (mounting()) return;
    const confirmed = await askConfirm({
      title: t("object.removeFullTitle"),
      message: t("object.removeFullMessage", { name: profile.name }),
      okLabel: t("sidebar.removeNetwork"),
      danger: true,
    });
    if (!confirmed) return;
    setMounting(profile.id);
    try {
      const mount = objectMount(profile);
      if (mount) {
        await ejectVolume(mount.path);
        await handleVolumeGone(mount.path);
      }
      await forgetObjectStorageSecret(profile.id);
      removeObjectStorageProfile(profile.id);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function mountBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    setMounting(b.url);
    try {
      const mountedPath = await mountNetworkUrl(b.url);
      await refreshVols();
      const fresh = bookmarks().find((x) => x.url === b.url);
      const target = mountedPath || (fresh?.connected ? fresh.mountPath : "");
      if (target) go(target);
    } catch (err) {
      await askConfirm({
        title: t("sidebar.mountFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function ejectBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    const confirmed = await askConfirm({
      title: t("sidebar.unmount"),
      message: t("sidebar.unmountConfirm", { name: b.name }),
      okLabel: t("sidebar.unmount"),
      danger: true,
    });
    if (!confirmed) return;
    setMounting(b.url);
    try {
      await ejectVolume(b.mountPath);
      await handleVolumeGone(b.mountPath);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function removeBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    const confirmed = await askConfirm({
      title: t("sidebar.removeNetworkTitle"),
      message: t("sidebar.removeNetworkConfirm", { name: b.name }),
      okLabel: t("sidebar.removeNetwork"),
      danger: true,
    });
    if (!confirmed) return;
    setMounting(b.url);
    try {
      // Stufe 2 umfasst auch Stufe 1: Ein verbundenes Volume wird zuerst
      // sauber ausgehängt, erst dann verschwindet das Lesezeichen.
      if (b.connected) {
        await ejectVolume(b.mountPath);
        await handleVolumeGone(b.mountPath);
      }
      await removeNetworkBookmark(b.url);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title: t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function reconnectBookmark(b: NetworkBookmark) {
    if (mounting()) return;
    setMounting(b.url);
    try {
      // Erst aushängen (Fehler ignorieren, falls schon weg), dann neu mounten.
      try {
        await ejectVolume(b.mountPath);
        await handleVolumeGone(b.mountPath);
      } catch {
        /* nicht gemountet oder bereits ausgehängt – weiter mit mount */
      }
      const mountedPath = await mountNetworkUrl(b.url);
      await refreshVols();
      const fresh = bookmarks().find((x) => x.url === b.url);
      const target = mountedPath || (fresh?.connected ? fresh.mountPath : "");
      if (target) go(target);
    } catch (err) {
      await askConfirm({
        title: t("sidebar.mountFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    } finally {
      setMounting(null);
    }
  }

  async function persist(next: Favorite[]) {
    setFavs(next);
    try {
      await saveFavorites(next);
    } catch (err) {
      console.error("saveFavorites:", err);
      await notifyError(t("sidebar.addFailed"));
    }
  }

  async function addCurrent() {
    const cwd = state[state.active].cwd;
    if (!cwd) return;
    const name = basename(cwd) || cwd;
    if (favs().some((f) => f.path === cwd)) {
      await notify({
        title: t("sidebar.favorites"),
        message: t("sidebar.alreadyFav", { name }),
      });
      return;
    }
    await persist([...favs(), { name, icon: "📁", path: cwd }]);
  }

  async function removeAt(idx: number) {
    const next = favs().slice();
    next.splice(idx, 1);
    await persist(next);
  }

  function startRename(idx: number) {
    setEditIdx(idx);
    setEditValue(favs()[idx].name);
  }

  async function commitRename() {
    const idx = editIdx();
    if (idx == null) return;
    const v = editValue().trim();
    setEditIdx(null);
    if (!v || v === favs()[idx].name) return;
    const next = favs().slice();
    next[idx] = { ...next[idx], name: v };
    await persist(next);
  }

  function cancelRename() {
    setEditIdx(null);
  }

  function onDragStart(i: number, ev: DragEvent) {
    setDragIdx(i);
    if (ev.dataTransfer) {
      ev.dataTransfer.setData("application/x-fav-idx", String(i));
      ev.dataTransfer.effectAllowed = "move";
    }
  }

  function onDragOver(i: number, ev: DragEvent) {
    if (dragIdx() == null) return;
    ev.preventDefault();
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = "move";
    if (overIdx() !== i) setOverIdx(i);
  }

  function onDragLeave(i: number) {
    if (overIdx() === i) setOverIdx(null);
  }

  async function onDrop(target: number, ev: DragEvent) {
    ev.preventDefault();
    const from = dragIdx();
    setDragIdx(null);
    setOverIdx(null);
    if (from == null || from === target) return;
    const next = favs().slice();
    const [m] = next.splice(from, 1);
    next.splice(target, 0, m);
    await persist(next);
  }

  function onDragEnd() {
    setDragIdx(null);
    setOverIdx(null);
  }

  function openMenu(idx: number, ev: MouseEvent) {
    ev.preventDefault();
    ev.stopPropagation();
    setMenu({ idx, x: ev.clientX, y: ev.clientY });
  }

  function closeMenu() {
    setMenu(null);
    setVolMenu(null);
  }

  function openVolMenu(vol: Volume, ev: MouseEvent) {
    ev.preventDefault();
    ev.stopPropagation();
    setMenu(null);
    setVolMenu({ vol, x: ev.clientX, y: ev.clientY });
  }

  async function doEject(vol: Volume) {
    const isNetwork = vol.kind === "network";
    // Über rclone eingehängte Ziele (SFTP, FTPS) verhalten sich beim Trennen wie
    // Netzlaufwerke, lassen sich aber nicht als Lesezeichen merken.
    const isRemote = vol.kind === "remote";
    const detach = isNetwork || isRemote;
    const ok = await askConfirm({
      title: detach ? t("sidebar.unmount") : t("sidebar.ejectTitle"),
      message: detach
        ? t("sidebar.unmountConfirm", { name: vol.name })
        : t("sidebar.ejectConfirm", { name: vol.name }),
      okLabel: detach ? t("sidebar.unmount") : t("sidebar.eject"),
      danger: true,
    });
    if (!ok) return;
    try {
      // Ein bislang nur flüchtig angezeigtes Netzlaufwerk wird vor dem
      // Aushängen als Lesezeichen gespeichert. Dadurch bleibt es danach in
      // der Sidebar sichtbar und lässt sich wieder verbinden.
      if (isNetwork) await rememberNetworkVolume(vol.path);
      await ejectVolume(vol.path);
      await handleVolumeGone(vol.path);
      await refreshVols();
    } catch (err) {
      await askConfirm({
        title:
          isNetwork
            ? t("sidebar.saveNetworkFailed")
            : t("sidebar.ejectFailed"),
        message: errMsg(err),
        okLabel: t("common.ok"),
        cancelLabel: t("common.close"),
      });
    }
  }

  const onGlobalClick = () => closeMenu();
  const onGlobalKey = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") closeMenu();
  };
  const onOpenFavorite = (ev: Event) => {
    const idx = (ev as CustomEvent<number>).detail;
    const favorite = favs()[idx];
    if (favorite) go(favorite.path);
  };

  onMount(() => {
    // onCleanup synchron registrieren – nach einem await würde Solid die
    // Registrierung nicht mehr dem Komponenten-Owner zuordnen und Listener
    // sowie Intervall würden beim Unmount weiterlaufen.
    let disposed = false;
    let volTimer: number | undefined;
    onCleanup(() => {
      disposed = true;
      if (volTimer !== undefined) window.clearInterval(volTimer);
      window.removeEventListener("click", onGlobalClick);
      window.removeEventListener("keydown", onGlobalKey);
      window.removeEventListener("dualbeam:open-favorite", onOpenFavorite);
      window.removeEventListener("focus", refreshRdpOnFocus);
    });
    window.addEventListener("click", onGlobalClick);
    window.addEventListener("keydown", onGlobalKey);
    window.addEventListener("dualbeam:open-favorite", onOpenFavorite);
    // Legt der Nutzer in RemoteDeskRDP eine Verbindung an und kommt zurueck,
    // soll sie ohne Neustart erscheinen.
    window.addEventListener("focus", refreshRdpOnFocus);
    void (async () => {
      await importRemoteDeskProfiles();
      try {
        setFavs(await loadFavorites());
      } catch {
        const home = await homeDir();
        if (!disposed) setFavs([{ name: "Home", icon: "🏠", path: home }]);
      }
      await refreshVols();
      await refreshRdp();
      if (!disposed) volTimer = window.setInterval(refreshVols, 5000);
    })();
  });

  createEffect(() => {
    volumesTick();
    void refreshVols();
  });

  const go = (path: string) => loadPane(state.active, path);

  return (
    <Show when={state.sidebarVisible}>
      <aside class="sidebar">
        <div class="sb-section">
          <span>{t("sidebar.favorites")}</span>
          <button
            class="sb-add"
            title={t("sidebar.addCurrent")}
            onClick={addCurrent}
          >
            ＋
          </button>
        </div>
        <For each={favs()}>
          {(f, i) => (
            <div
              class={`sb-item ${state[state.active].cwd === f.path ? "active" : ""} ${overIdx() === i() ? "drop-target" : ""}`}
              onClick={() => {
                if (editIdx() === i()) return;
                go(f.path);
              }}
              title={f.path}
              draggable={editIdx() !== i()}
              onDragStart={(ev) => onDragStart(i(), ev)}
              onDragEnter={(ev) => onDragOver(i(), ev)}
              onDragOver={(ev) => onDragOver(i(), ev)}
              onDragLeave={() => onDragLeave(i())}
              onDrop={(ev) => onDrop(i(), ev)}
              onDragEnd={onDragEnd}
              onContextMenu={(ev) => openMenu(i(), ev)}
              onDblClick={(ev) => {
                ev.stopPropagation();
                startRename(i());
              }}
            >
              <span class="sb-icon">{f.icon}</span>
              <Show
                when={editIdx() === i()}
                fallback={<span class="sb-label">{f.name}</span>}
              >
                <input
                  class="sb-edit"
                  title={t("sidebar.renameFav")}
                  value={editValue()}
                  autofocus
                  onInput={(e) => setEditValue(e.currentTarget.value)}
                  onClick={(e) => e.stopPropagation()}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") commitRename();
                    else if (e.key === "Escape") cancelRename();
                  }}
                  onBlur={commitRename}
                />
              </Show>
            </div>
          )}
        </For>
        <div class="sb-section">{t("sidebar.volumes")}</div>
        <Show
          when={vols().filter(isLocalVolume).length > 0}
          fallback={<div class="sb-empty">{t("sidebar.none")}</div>}
        >
          <For each={vols().filter(isLocalVolume)}>
            {(v) => (
              <div
                class={`sb-item ${state[state.active].cwd === v.path ? "active" : ""}`}
                onClick={() => go(v.path)}
                onContextMenu={(ev) => openVolMenu(v, ev)}
                title={v.path}
              >
                <span class="sb-icon">💽</span>
                <span class="sb-label">{v.name}</span>
                <button
                  class="sb-eject"
                  title={t("sidebar.eject")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void doEject(v);
                  }}
                >
                  ⏏
                </button>
              </div>
            )}
          </For>
        </Show>
        <div class="sb-section">
          <span>{t("sidebar.network")}</span>
          <button
            class="sb-add"
            title={t("network.addDrive")}
            onClick={() => openNetworkStorageDialog()}
          >
            ＋
          </button>
        </div>
        <For each={objectStorageProfiles()}>
          {(profile) => {
            const mount = () => objectMount(profile);
            const home = () => {
              const current = mount();
              return current ? objectHomePath(profile, current) : undefined;
            };
            return (
            <div
              class={`sb-item ${home() && (state[state.active].cwd === home() || state[state.active].cwd.startsWith(`${home()}/`)) ? "active" : ""} ${mount() ? "" : "disconnected"}`}
              onClick={() => {
                const current = mount();
                if (current) void openObjectHome(profile, current);
                else void mountObjectProfile(profile);
              }}
              title={home() ?? `${profile.protocol.toUpperCase()} — ${profile.endpoint}`}
            >
              <span class="sb-icon">{mount() ? "🌐" : "☁"}</span>
              <span class="sb-label">{profile.name}</span>
              <Show when={mounting() === profile.id}>
                <span class="sb-spin">…</span>
              </Show>
              <span class="sb-actions">
                <button
                  class="sb-eject"
                  title={t("object.edit")}
                  onClick={(event) => {
                    event.stopPropagation();
                    openObjectStorageDialog(profile);
                  }}
                >
                  ⚙
                </button>
                <Show when={mount() && mounting() !== profile.id}>
                  <button
                    class="sb-eject"
                    title={t("sidebar.reconnect")}
                    onClick={(event) => {
                      event.stopPropagation();
                      void reconnectObjectProfile(profile);
                    }}
                  >
                    ↻
                  </button>
                  <button
                    class="sb-eject"
                    title={t("sidebar.unmount")}
                    onClick={(event) => {
                      event.stopPropagation();
                      void ejectObjectProfile(profile);
                    }}
                  >
                    ⏏
                  </button>
                </Show>
                <Show when={mounting() !== profile.id}>
                  <button
                    class="sb-eject sb-remove"
                    title={t("sidebar.removeNetwork")}
                    onClick={(event) => {
                      event.stopPropagation();
                      void removeObjectProfile(profile);
                    }}
                  >
                    ×
                  </button>
                </Show>
              </span>
            </div>
            );
          }}
        </For>
        <For each={remoteProfiles()}>
          {(profile) => {
            const mount = () => savedRemoteMount(profile);
            return <div class={`sb-item ${mount() ? "" : "disconnected"}`} onClick={() => void openSavedRemoteProfile(profile)} title={mount() ? mount()!.path : `${profile.host} — ${t("sidebar.clickToMount")}`}>
              <span class="sb-icon">{mount() ? "🌐" : "🔌"}</span>
              <span class="sb-label">{profile.label || profile.host}</span>
              <span class="sb-actions">
                <button class="sb-eject" title={t("network.connectionSettings")} onClick={(event) => { event.stopPropagation(); openRemoteDialog({ ...profile, port: profile.port?.toString() ?? "" }); }}>⚙</button>
                <Show when={mount()}><button class="sb-eject" title={t("sidebar.reconnect")} onClick={(event) => { event.stopPropagation(); void reconnectSavedRemoteProfile(profile); }}>↻</button><button class="sb-eject" title={t("sidebar.unmount")} onClick={(event) => { event.stopPropagation(); void doEject({ name: profile.label || profile.host, path: mount()!.path, kind: "remote" }); }}>⏏</button></Show>
                <button class="sb-eject sb-remove" title={t("sidebar.removeNetwork")} onClick={(event) => { event.stopPropagation(); void removeSavedRemote(profile); }}>×</button>
              </span>
            </div>;
          }}
        </For>
        <For each={nfsProfiles()}>
          {(profile) => {
            const mount = () => savedNfsMount(profile);
            return <div class={`sb-item ${mount() ? "" : "disconnected"}`} onClick={() => void openSavedNfsProfile(profile)} title={mount() ? mount()!.path : `${profile.host}${profile.path} — ${t("sidebar.clickToMount")}`}>
              <span class="sb-icon">{mount() ? "🌐" : "🔌"}</span>
              <span class="sb-label">{profile.label || profile.host}</span>
              <span class="sb-actions">
                <button class="sb-eject" title={t("network.connectionSettings")} onClick={(event) => { event.stopPropagation(); openNfsDialog({ ...profile }); }}>⚙</button>
                <Show when={mount()}><button class="sb-eject" title={t("sidebar.reconnect")} onClick={(event) => { event.stopPropagation(); void reconnectSavedNfsProfile(profile); }}>↻</button><button class="sb-eject" title={t("sidebar.unmount")} onClick={(event) => { event.stopPropagation(); void doEject({ name: profile.label || profile.host, path: mount()!.path, kind: "remote" }); }}>⏏</button></Show>
                <button class="sb-eject sb-remove" title={t("sidebar.removeNetwork")} onClick={(event) => { event.stopPropagation(); void removeSavedNfs(profile); }}>×</button>
              </span>
            </div>;
          }}
        </For>
        <For each={bookmarks()}>
          {(b) => (
            <div
              class={`sb-item ${state[state.active].cwd === b.mountPath ? "active" : ""} ${b.connected ? "" : "disconnected"}`}
              onClick={() => (b.connected ? go(b.mountPath) : mountBookmark(b))}
              title={
                b.connected
                  ? b.mountPath
                  : `${b.url} — ${t("sidebar.clickToMount")}`
              }
            >
              <span class="sb-icon">{b.connected ? "🌐" : "🔌"}</span>
              <span class="sb-label">{b.name}</span>
              <Show when={mounting() === b.url}>
                <span class="sb-spin">…</span>
              </Show>
              <Show when={mounting() !== b.url}>
                <span class="sb-actions">
                <button
                  class="sb-eject"
                  title={t("network.connectionSettings")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    openNetworkStorageDialog(b.url);
                  }}
                >
                  ⚙
                </button>
                <Show when={b.connected}>
                  <button
                  class="sb-eject"
                  title={t("sidebar.reconnect")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void reconnectBookmark(b);
                  }}
                >
                  ↻
                </button>
                <button
                  class="sb-eject"
                  title={t("sidebar.unmount")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void ejectBookmark(b);
                  }}
                >
                  ⏏
                </button>
                </Show>
                <button
                  class="sb-eject sb-remove"
                  title={t("sidebar.removeNetwork")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    void removeBookmark(b);
                  }}
                >
                  ×
                </button>
                </span>
              </Show>
            </div>
          )}
        </For>
        <For
          each={vols().filter(
              (v) =>
              (v.kind === "network" || v.kind === "remote") &&
              !bookmarks().some((b) => b.mountPath === v.path) &&
              !remoteMountList().some(
                (mount) =>
                  mount.path === v.path &&
                  objectStorageProfiles().some(
                    (profile) => mount.descriptor === `${profile.protocol}://${profile.id}`,
                  ),
              ) && !remoteMountList().some(
                (mount) => mount.path === v.path && remoteProfiles().some((profile) => mount.descriptor === remoteDescriptor(profile)),
              ) && !remoteMountList().some(
                (mount) => mount.path === v.path && nfsProfiles().some((profile) => mount.descriptor === nfsDescriptor(profile)),
              ),
          )}
        >
          {(v) => (
            <div
              class={`sb-item ${state[state.active].cwd === v.path ? "active" : ""}`}
              onClick={() => go(v.path)}
              onContextMenu={(ev) => openVolMenu(v, ev)}
              title={v.path}
            >
              <span class="sb-icon">🌐</span>
              <span class="sb-label">{v.name}</span>
              <Show when={remoteMountList().find((mount) => mount.path === v.path)}>{(mount) =>
                <span class="sb-actions">
                  <button class="sb-eject" title={t("network.connectionSettings")} onClick={(ev) => { ev.stopPropagation(); editRemoteMount(mount()); }}>⚙</button>
                  <button class="sb-eject" title={t("sidebar.reconnect")} onClick={(ev) => { ev.stopPropagation(); editRemoteMount(mount()); }}>↻</button>
                  <button
                  class="sb-eject"
                  title={t("sidebar.unmount")}
                    onClick={(ev) => { ev.stopPropagation(); void doEject(v); }}
                  >⏏</button>
                  <button class="sb-eject sb-remove" title={t("sidebar.removeNetwork")} onClick={(ev) => { ev.stopPropagation(); void doEject(v); }}>×</button>
                </span>
              }</Show>
              <Show when={!remoteMountList().some((mount) => mount.path === v.path)}>
                <button class="sb-eject" title={t("sidebar.unmount")} onClick={(ev) => { ev.stopPropagation(); void doEject(v); }}>⏏</button>
              </Show>
            </div>
            )}
          </For>
        <div class="sb-section">{t("sidebar.syncProfiles")}</div>
        <Show
          when={syncProfiles().length > 0}
          fallback={<div class="sb-empty">{t("sidebar.none")}</div>}
        >
          <For each={syncProfiles()}>
            {(profile) => (
              <button
                class="sb-item sb-sync-profile"
                disabled={!!state.job}
                onClick={() => void runSyncProfile(profile.id)}
                title={`${profile.src} → ${profile.dst}`}
              >
                <span class="sb-icon">⇄</span>
                <span class="sb-label">{profile.name}</span>
              </button>
            )}
          </For>
        </Show>
        <Show when={rdp().length > 0}>
          <div class="sb-section">
            {t("sidebar.remoteDesktop")}
          </div>
          <For each={rdp()}>
            {(profile) => (
              <button
                class="sb-item sb-rdp"
                onClick={() => void openRdp(profile)}
                disabled={connectingRdp() !== null}
                title={
                  connectingRdp() === profile.id
                    ? t("rdp.connecting")
                    : t("rdp.connectTitle", {
                        target: profile.host || profile.name,
                      })
                }
              >
                <span class="sb-icon">
                  {connectingRdp() === profile.id ? "⏳" : "🖥"}
                </span>
                <span class="sb-label">{profile.name}</span>
              </button>
            )}
          </For>
        </Show>
        <Show when={menu()}>
          {(m) => (
            <div
              class="ctx-menu sidebar-ctx"
              ref={(el) => {
                el.style.setProperty("--cx", `${m().x}px`);
                el.style.setProperty("--cy", `${m().y}px`);
              }}
              onClick={(e) => e.stopPropagation()}
            >
              <div
                class="ctx-item"
                onClick={() => {
                  const idx = m().idx;
                  closeMenu();
                  startRename(idx);
                }}
              >
                {t("sidebar.rename")}
              </div>
              <div
                class="ctx-item danger"
                onClick={() => {
                  const idx = m().idx;
                  closeMenu();
                  removeAt(idx);
                }}
              >
                {t("sidebar.remove")}
              </div>
            </div>
          )}
        </Show>
        <Show when={volMenu()}>
          {(m) => (
            <div
              class="ctx-menu sidebar-ctx"
              ref={(el) => {
                el.style.setProperty("--cx", `${m().x}px`);
                el.style.setProperty("--cy", `${m().y}px`);
              }}
              onClick={(e) => e.stopPropagation()}
            >
              <div
                class="ctx-item"
                onClick={() => {
                  const v = m().vol;
                  closeMenu();
                  void doEject(v);
                }}
              >
                {t("sidebar.eject")}
              </div>
            </div>
          )}
        </Show>
      </aside>
    </Show>
  );
}
