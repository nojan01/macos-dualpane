import { Show, For, createSignal } from "solid-js";
import { t, getResolvedLang } from "../i18n";

const [open, setOpen] = createSignal(false);
const [sectionIdx, setSectionIdx] = createSignal(0);

/** Öffnet den Lizenz-Dialog (beginnt immer bei der EULA). */
export function openLicense() {
  setSectionIdx(0);
  setOpen(true);
}

interface Item {
  term: string;
  desc: string;
}
interface Section {
  title: string;
  intro?: string;
  items?: Item[];
  outro?: string;
}

const CONTENT: Record<
  "de" | "en",
  { title: string; navLabel: string; sections: Section[] }
> = {
  de: {
    title: "DualBeam – Lizenz",
    navLabel: "Lizenzthemen",
    sections: [
      {
        title: "Endbenutzer-Lizenzvereinbarung",
        intro:
          "Diese Vereinbarung regelt die Nutzung von DualBeam zwischen Ihnen (dem Endnutzer) und dem Urheber Norbert Jander. Mit der Nutzung der Software erklären Sie sich mit diesen Bedingungen einverstanden.",
        items: [
          {
            term: "1. Vertragsparteien und Geltung",
            desc: "Vertragspartner sind Sie als Endnutzer und der Urheber Norbert Jander.",
          },
          {
            term: "2. Lizenzerteilung und Nutzung",
            desc: "Sie erhalten ein kostenlos verfügbares, nicht exklusives und nicht übertragbares Nutzungsrecht. Die Nutzung ist privat wie kommerziell unbeschränkt erlaubt.",
          },
          {
            term: "3. Nutzungsbeschränkungen",
            desc: "Untersagt sind: den Quellcode zu verändern, anzupassen oder zu übersetzen; die Software zu dekompilieren, zu disassemblieren oder per Reverse Engineering zu untersuchen; abgeleitete Werke auf ihrer Basis zu erstellen.",
          },
          {
            term: "4. Vertrieb und Weitergabe",
            desc: "Die Weitergabe ist ausschließlich in unveränderter Form erlaubt. Verkauf und kommerzielle Vermietung an Dritte sind untersagt.",
          },
          {
            term: "5. Vorrang der Fremdlizenzen",
            desc: "Ziffer 3 und 4 gelten nur für die vom Urheber erstellten Bestandteile. Für mitgelieferte Fremdkomponenten gelten allein deren eigene Lizenzen; sie gehen im Konfliktfall vor. Insbesondere bleibt rclone unter der MIT-Lizenz frei verwendbar, veränderbar und weitergebbar.",
          },
          {
            term: "6. Gewährleistung und Haftung",
            desc: "Die Software wird kostenfrei und „wie gesehen“ bereitgestellt. Der Urheber übernimmt keine Haftung für Fehler und deren Folgen. Bei unentgeltlicher Überlassung ist die Haftung nach deutschem Recht auf Vorsatz und grobe Fahrlässigkeit beschränkt.",
          },
          {
            term: "7. Anwendbares Recht",
            desc: "Es gilt das Recht der Bundesrepublik Deutschland. Gerichtsstand ist, soweit zulässig, der Sitz des Urhebers.",
          },
        ],
        outro:
          "Der vollständige Wortlaut liegt dem Programm als EULA.txt bei und steht in der Datei LICENSE des Projekts.",
      },
      {
        title: "Verwendete Module und deren Lizenzen",
        intro:
          "Genau ein fremdes Programm wird als eigene Datei mitgeliefert. Alles Weitere ist in die Programmdatei einkompiliert.",
        items: [
          {
            term: "rclone 1.74.4 — MIT",
            desc: "Copyright © 2012 Nick Craig-Wood und Mitwirkende. Stellt SFTP- und FTPS-Verbindungen als Laufwerk bereit. rclone ist in Go geschrieben und enthält seinerseits zahlreiche Open-Source-Bibliotheken unter eigenen Lizenzen; deren Auflistung ist Teil des rclone-Projekts.",
          },
          {
            term: "Tauri 2 und Plugins — Apache-2.0 oder MIT",
            desc: "tauri 2.11.2, tauri-plugin-drag 2.1.1. Das Anwendungsgerüst: Fenster, Menüs, Brücke zwischen Weboberfläche und Systemcode.",
          },
          {
            term: "Rust-Bibliotheken — überwiegend MIT oder Apache-2.0",
            desc: "serde, serde_json, dirs, walkdir, trash, notify-debouncer-mini, zip, libc, url, sha2, security-framework, objc2. Ausnahme: notify 6.1.1 steht unter CC0-1.0 (Gemeinfreiheit).",
          },
          {
            term: "SolidJS — MIT",
            desc: "Das Gerüst der Weboberfläche. Vite, TypeScript, Vitest und jsdom sind reine Bauwerkzeuge und werden nicht mitgeliefert.",
          },
          {
            term: "Werkzeuge des Betriebssystems",
            desc: "rsync, curl, open, osascript, security, ssh-keygen, ssh-keyscan, mount, umount, diskutil, hdiutil, mdfind, qlmanage und tmutil werden nur aufgerufen, nicht mitgeliefert — für sie besteht keine Lizenzpflicht.",
          },
        ],
        outro:
          "Die vollständige Aufstellung mit Versionsnummern steht in THIRD_PARTY_LICENSES.md.",
      },
      {
        title: "Lizenztexte im Programmpaket",
        intro:
          "MIT und Apache-2.0 verlangen, dass Urhebervermerk und Lizenztext der Weitergabe beiliegen. Sie finden sie im Programmpaket unter DualBeam.app/Contents/Resources/licenses/.",
        items: [
          {
            term: "EULA.txt",
            desc: "Diese Endbenutzer-Lizenzvereinbarung im vollen Wortlaut.",
          },
          {
            term: "RCLONE-LICENSE.txt",
            desc: "Die MIT-Lizenz von rclone samt Urhebervermerk.",
          },
          {
            term: "MIT.txt und APACHE-2.0.txt",
            desc: "Der Wortlaut beider Lizenzen, unter denen die einkompilierten Bibliotheken stehen.",
          },
          {
            term: "THIRD-PARTY-LICENSES.txt",
            desc: "Die Gesamtübersicht aller Komponenten mit Version und Lizenz.",
          },
        ],
      },
      {
        title: "Hinweis zum Lizenzwechsel",
        intro:
          "DualBeam stand bis einschließlich Version 0.4.0 unter der MIT-Lizenz.",
        items: [
          {
            term: "Was sich ändert",
            desc: "Alle danach veröffentlichten Fassungen unterliegen dieser Endbenutzer-Lizenzvereinbarung.",
          },
          {
            term: "Was bestehen bleibt",
            desc: "Bereits erteilte MIT-Rechte lassen sich nicht widerrufen. Wer eine unter MIT veröffentlichte Fassung erhalten hat, darf mit dieser Fassung weiterhin alles tun, was die MIT-Lizenz gestattet. Die neue Vereinbarung wirkt ausschließlich nach vorn.",
          },
        ],
      },
    ],
  },
  en: {
    title: "DualBeam – Licence",
    navLabel: "Licence topics",
    sections: [
      {
        title: "End User Licence Agreement",
        intro:
          "This agreement governs your use of DualBeam between you (the end user) and the author, Norbert Jander. By using the software you accept these terms.",
        items: [
          {
            term: "1. Parties and scope",
            desc: "The contracting parties are you as the end user and the author, Norbert Jander.",
          },
          {
            term: "2. Grant of licence",
            desc: "You receive a free of charge, non-exclusive and non-transferable right of use. Both private and commercial use are permitted without restriction.",
          },
          {
            term: "3. Restrictions",
            desc: "You may not modify, adapt or translate the source code; decompile, disassemble or reverse engineer the software; or create derivative works based on it.",
          },
          {
            term: "4. Distribution",
            desc: "You may pass the software on in unmodified form only. Selling or commercially renting it to third parties is prohibited.",
          },
          {
            term: "5. Precedence of third-party licences",
            desc: "Clauses 3 and 4 apply only to the parts written by the author. Bundled third-party components are governed solely by their own licences, which take precedence in case of conflict. In particular, rclone remains freely usable, modifiable and redistributable under the MIT licence.",
          },
          {
            term: "6. Warranty and liability",
            desc: "The software is provided free of charge and “as is”. The author accepts no liability for errors or their consequences. For software supplied free of charge, German law limits liability to intent and gross negligence.",
          },
          {
            term: "7. Governing law",
            desc: "The law of the Federal Republic of Germany applies. Place of jurisdiction is, as far as legally permissible, the author's place of business.",
          },
        ],
        outro:
          "The full wording is bundled with the program as EULA.txt and lives in the project's LICENSE file.",
      },
      {
        title: "Components used and their licences",
        intro:
          "Exactly one third-party program ships as a separate file. Everything else is compiled into the executable.",
        items: [
          {
            term: "rclone 1.74.4 — MIT",
            desc: "Copyright © 2012 Nick Craig-Wood and contributors. Provides SFTP and FTPS connections as a mounted drive. rclone is written in Go and itself contains numerous open-source libraries under their own licences; that listing is part of the rclone project.",
          },
          {
            term: "Tauri 2 and plugins — Apache-2.0 or MIT",
            desc: "tauri 2.11.2, tauri-plugin-drag 2.1.1. The application framework: windows, menus, and the bridge between web interface and system code.",
          },
          {
            term: "Rust libraries — mostly MIT or Apache-2.0",
            desc: "serde, serde_json, dirs, walkdir, trash, notify-debouncer-mini, zip, libc, url, sha2, security-framework, objc2. One exception: notify 6.1.1 is CC0-1.0 (public domain).",
          },
          {
            term: "SolidJS — MIT",
            desc: "The web interface framework. Vite, TypeScript, Vitest and jsdom are build tools only and are not shipped.",
          },
          {
            term: "Operating system tools",
            desc: "rsync, curl, open, osascript, security, ssh-keygen, ssh-keyscan, mount, umount, diskutil, hdiutil, mdfind, qlmanage and tmutil are merely invoked, not bundled — no licence obligation arises for them.",
          },
        ],
        outro:
          "The complete list including version numbers is in THIRD_PARTY_LICENSES.md.",
      },
      {
        title: "Licence texts in the app bundle",
        intro:
          "MIT and Apache-2.0 require the copyright notice and licence text to accompany the distribution. You will find them inside the app bundle under DualBeam.app/Contents/Resources/licenses/.",
        items: [
          {
            term: "EULA.txt",
            desc: "This end user licence agreement in full.",
          },
          {
            term: "RCLONE-LICENSE.txt",
            desc: "The MIT licence of rclone including its copyright notice.",
          },
          {
            term: "MIT.txt and APACHE-2.0.txt",
            desc: "The wording of both licences that cover the compiled-in libraries.",
          },
          {
            term: "THIRD-PARTY-LICENSES.txt",
            desc: "The overview of all components with version and licence.",
          },
        ],
      },
      {
        title: "Note on the licence change",
        intro:
          "Up to and including version 0.4.0, DualBeam was released under the MIT licence.",
        items: [
          {
            term: "What changes",
            desc: "Every release after that is covered by this end user licence agreement.",
          },
          {
            term: "What remains",
            desc: "MIT rights already granted cannot be revoked. Anyone who obtained a version released under MIT may continue to do everything the MIT licence permits with that version. The new agreement applies going forward only.",
          },
        ],
      },
    ],
  },
};

function ItemList(props: { items: Item[] }) {
  return (
    <dl class="help-grid">
      <For each={props.items}>
        {(item) => (
          <>
            <dt>{item.term}</dt>
            <dd>{item.desc}</dd>
          </>
        )}
      </For>
    </dl>
  );
}

export function LicenseDialog() {
  function close() {
    setOpen(false);
  }

  const content = () => CONTENT[getResolvedLang() === "en" ? "en" : "de"];
  const section = () =>
    content().sections[Math.min(sectionIdx(), content().sections.length - 1)];

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
          <div class="help-layout">
            <nav class="help-nav" aria-label={content().navLabel}>
              <For each={content().sections}>
                {(s, i) => (
                  <button
                    classList={{ active: i() === sectionIdx() }}
                    onClick={() => setSectionIdx(i())}
                  >
                    {s.title}
                  </button>
                )}
              </For>
            </nav>
            <div class="help-body">
              <section class="help-section">
                <h3>{section().title}</h3>
                <Show when={section().intro}>
                  <p class="help-intro">{section().intro}</p>
                </Show>
                <Show when={section().items}>
                  <ItemList items={section().items!} />
                </Show>
                <Show when={section().outro}>
                  <p class="help-intro">{section().outro}</p>
                </Show>
              </section>
            </div>
          </div>
          <div class="modal-actions">
            <button onClick={close}>{t("common.close")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
}
