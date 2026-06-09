import { Show, For, createSignal } from "solid-js";
import { t, getResolvedLang } from "../i18n";

const [open, setOpen] = createSignal(false);

/** Öffnet den Hilfe-Dialog. */
export async function openHelp() {
  setOpen(true);
}

interface Item {
  term: string;
  desc: string;
}
interface Section {
  title: string;
  intro?: string;
  items: Item[];
}

const CONTENT: Record<"de" | "en", { title: string; sections: Section[] }> = {
  de: {
    title: "DualBeam – Hilfe",
    sections: [
      {
        title: "Überblick",
        items: [
          {
            term: "Zwei-Fenster-Ansicht",
            desc: "DualBeam zeigt zwei Dateibereiche nebeneinander. Aktionen wie Kopieren, Verschieben oder Synchronisieren beziehen sich auf den aktiven und den gegenüberliegenden Bereich.",
          },
          {
            term: "Aktiver Bereich",
            desc: "Der Bereich mit dem Tastaturfokus ist hervorgehoben. Mit Tab oder einem Klick wechselt der Fokus.",
          },
          {
            term: "Tabs",
            desc: "Jeder Bereich kann mehrere Tabs (Ordner) enthalten. Neuer Tab mit ⌘T, schließen mit ⌘W.",
          },
        ],
      },
      {
        title: "Wichtigste Funktionen",
        items: [
          { term: "Kopieren (F5)", desc: "Auswahl in den gegenüberliegenden Bereich kopieren." },
          { term: "Verschieben (F6)", desc: "Auswahl in den gegenüberliegenden Bereich verschieben." },
          { term: "Neuer Ordner (F7)", desc: "Im aktiven Bereich einen Ordner anlegen." },
          { term: "Löschen (F8 / ⌫)", desc: "Auswahl in den Papierkorb legen. Mit ⇧ unwiderruflich löschen." },
          { term: "Umbenennen (F2)", desc: "Datei oder Ordner umbenennen; Stapel-Umbenennung wird unterstützt." },
          { term: "Suchen (⌘F)", desc: "Im aktuellen Ordner nach Namen filtern." },
          { term: "Vorschau (⌘I)", desc: "Vorschau-Bereich für die markierte Datei ein-/ausblenden." },
          { term: "Synchronisieren", desc: "Inhalte beider Bereiche abgleichen (einseitig oder beidseitig)." },
          { term: "Mit Server verbinden (⌘K)", desc: "Netzwerkfreigabe über eine URL einbinden – siehe Netzwerkprotokolle." },
          { term: "Neues Fenster (⌘N)", desc: "Ein weiteres unabhängiges DualBeam-Fenster öffnen." },
          { term: "Seitenleiste (⌘B)", desc: "Seitenleiste mit Favoriten, Volumes und Netzwerk ein-/ausblenden." },
          { term: "Im Terminal öffnen", desc: "Den aktuellen Ordner im Terminal öffnen." },
        ],
      },
      {
        title: "Netzwerkprotokolle",
        intro:
          "Über „Mit Server verbinden“ (⌘K) bindest du Freigaben per URL ein. Unterstützte Protokolle:",
        items: [
          { term: "smb:// · cifs://", desc: "Windows-/Samba-Freigaben. Beispiel: smb://server/freigabe. Das Standardprotokoll für die meisten NAS-Geräte und Windows-Rechner." },
          { term: "afp://", desc: "Apple Filing Protocol – ältere macOS-Server und NAS-Geräte. Beispiel: afp://server/freigabe." },
          { term: "nfs://", desc: "Network File System – verbreitet in Unix-/Linux-Umgebungen. Beispiel: nfs://server/export." },
          { term: "ftp:// · ftps://", desc: "File Transfer Protocol, ftps:// mit TLS-Verschlüsselung. Beispiel: ftps://server/pfad." },
          { term: "http:// · https:// (WebDAV)", desc: "WebDAV-Freigaben (z. B. Nextcloud, HiDrive, ownCloud). https:// ist verschlüsselt und empfohlen. Beispiel: https://server/webdav." },
        ],
      },
      {
        title: "Anmeldedaten & Schlüsselbund",
        intro:
          "Das Verbinden übernimmt macOS (Finder), nicht DualBeam. Daraus ergeben sich einige Eigenheiten:",
        items: [
          { term: "Passwortabfrage", desc: "Hast du beim ersten Verbinden „Passwort sichern“ aktiviert, liegt es im macOS-Schlüsselbund. macOS mountet dann ohne erneute Passwortabfrage – das ist normal und kein Fehler von DualBeam." },
          { term: "Dialog „webdavfs_agent…“", desc: "Dieser Schlüsselbund-Dialog stammt von macOS. Er fragt nicht nach deinem HiDrive-Passwort, sondern ob der System-Dienst das gespeicherte Passwort lesen darf." },
          { term: "Mountet trotz „Nicht erlauben“", desc: "Wenn der Schlüsselbund-Eintrag „immer erlauben“ für webdavfs_agent gesetzt ist, greift macOS direkt zu und hängt das Volume parallel ein. Das ist macOS-Verhalten, kein DualBeam-Bug." },
          { term: "Wieder nachfragen lassen", desc: "In der Schlüsselbundverwaltung den Eintrag des Servers öffnen → Reiter „Zugriff“ → webdavfs_agent entfernen bzw. auf „nachfragen“ stellen. Oder den gespeicherten Eintrag löschen, damit macOS beim nächsten Mount neu nach dem Passwort fragt." },
          { term: "Nachfragen abschalten", desc: "Den ständigen Dialog stoppst du in der Schlüsselbundverwaltung: Eintrag des Servers doppelklicken → Reiter „Zugriff“ → entweder „Allen Programmen Zugriff erlauben“ wählen oder sicherstellen, dass webdavfs_agent in der Liste steht und auf „immer erlauben“ steht → „Änderungen sichern“. Kommt der Dialog trotzdem, gibt es oft einen doppelten/alten Eintrag für denselben Host – diesen löschen." },
          { term: "NFS", desc: "NFS kennt keine Passwortabfrage – der Zugriff wird über die Export-/Freigabeliste des Servers (Host/IP) geregelt." },
        ],
      },
    ],
  },
  en: {
    title: "DualBeam – Help",
    sections: [
      {
        title: "Overview",
        items: [
          {
            term: "Dual-pane view",
            desc: "DualBeam shows two file panes side by side. Actions like copy, move or sync operate between the active pane and the opposite one.",
          },
          {
            term: "Active pane",
            desc: "The pane with keyboard focus is highlighted. Press Tab or click to switch focus.",
          },
          {
            term: "Tabs",
            desc: "Each pane can hold several tabs (folders). New tab with ⌘T, close with ⌘W.",
          },
        ],
      },
      {
        title: "Key features",
        items: [
          { term: "Copy (F5)", desc: "Copy the selection to the opposite pane." },
          { term: "Move (F6)", desc: "Move the selection to the opposite pane." },
          { term: "New folder (F7)", desc: "Create a folder in the active pane." },
          { term: "Delete (F8 / ⌫)", desc: "Move the selection to the Trash. Hold ⇧ to delete permanently." },
          { term: "Rename (F2)", desc: "Rename a file or folder; batch renaming is supported." },
          { term: "Search (⌘F)", desc: "Filter the current folder by name." },
          { term: "Preview (⌘I)", desc: "Show or hide the preview pane for the selected file." },
          { term: "Synchronize", desc: "Reconcile the contents of both panes (one-way or two-way)." },
          { term: "Connect to server (⌘K)", desc: "Mount a network share via a URL – see network protocols." },
          { term: "New window (⌘N)", desc: "Open another independent DualBeam window." },
          { term: "Sidebar (⌘B)", desc: "Toggle the sidebar with favorites, volumes and network." },
          { term: "Open in Terminal", desc: "Open the current folder in Terminal." },
        ],
      },
      {
        title: "Network protocols",
        intro:
          "Use “Connect to server” (⌘K) to mount shares by URL. Supported protocols:",
        items: [
          { term: "smb:// · cifs://", desc: "Windows/Samba shares. Example: smb://server/share. The default protocol for most NAS devices and Windows machines." },
          { term: "afp://", desc: "Apple Filing Protocol – older macOS servers and NAS devices. Example: afp://server/share." },
          { term: "nfs://", desc: "Network File System – common in Unix/Linux environments. Example: nfs://server/export." },
          { term: "ftp:// · ftps://", desc: "File Transfer Protocol, ftps:// adds TLS encryption. Example: ftps://server/path." },
          { term: "http:// · https:// (WebDAV)", desc: "WebDAV shares (e.g. Nextcloud, HiDrive, ownCloud). https:// is encrypted and recommended. Example: https://server/webdav." },
        ],
      },
      {
        title: "Credentials & Keychain",
        intro:
          "Mounting is handled by macOS (Finder), not by DualBeam. This leads to a few quirks:",
        items: [
          { term: "Password prompt", desc: "If you ticked “Save password” on first connect, it is stored in the macOS Keychain. macOS then mounts without asking again – this is normal and not a DualBeam bug." },
          { term: "“webdavfs_agent…” dialog", desc: "This Keychain dialog comes from macOS. It does not ask for your HiDrive password, but whether the system service may read the stored password." },
          { term: "Mounts despite “Deny”", desc: "If the Keychain entry has “always allow” set for webdavfs_agent, macOS reads it directly and mounts the volume in parallel. That is macOS behaviour, not a DualBeam bug." },
          { term: "Make it ask again", desc: "In Keychain Access open the server’s entry → “Access Control” tab → remove webdavfs_agent or set it to “ask”. Or delete the stored entry so macOS prompts for the password on the next mount." },
          { term: "Stop the prompt", desc: "To stop the recurring dialog, open Keychain Access: double-click the server’s entry → “Access Control” tab → either choose “Allow all applications to access this item” or make sure webdavfs_agent is in the list and set to “always allow” → “Save Changes”. If the dialog still appears, there is often a duplicate/old entry for the same host – delete it." },
          { term: "NFS", desc: "NFS has no password prompt – access is governed by the server’s export/share list (host/IP)." },
        ],
      },
    ],
  },
};

export function HelpDialog() {
  function close() {
    setOpen(false);
  }

  const content = () => CONTENT[getResolvedLang() === "en" ? "en" : "de"];

  return (
    <Show when={open()}>
      <div class="modal-backdrop" onMouseDown={close}>
        <div
          class="modal help-modal"
          role="dialog"
          aria-modal="true"
          aria-label={content().title}
          onMouseDown={(e) => e.stopPropagation()}
          tabIndex={-1}
          ref={(el) => queueMicrotask(() => el?.focus())}
          onKeyDown={(ev) => {
            ev.stopPropagation();
            if (ev.key === "Escape") {
              ev.preventDefault();
              close();
            }
          }}
        >
          <h2>{content().title}</h2>
          <div class="help-body">
            <For each={content().sections}>
              {(section) => (
                <section class="help-section">
                  <h3>{section.title}</h3>
                  <Show when={section.intro}>
                    <p class="help-intro">{section.intro}</p>
                  </Show>
                  <dl class="help-grid">
                    <For each={section.items}>
                      {(item) => (
                        <>
                          <dt>{item.term}</dt>
                          <dd>{item.desc}</dd>
                        </>
                      )}
                    </For>
                  </dl>
                </section>
              )}
            </For>
          </div>
          <div class="modal-actions">
            <button onClick={close}>{t("common.close")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
}
