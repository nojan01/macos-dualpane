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
