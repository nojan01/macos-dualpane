import { Show, For, createSignal } from "solid-js";
import { t, getResolvedLang } from "../i18n";

const [open, setOpen] = createSignal(false);
const [sectionIdx, setSectionIdx] = createSignal(0);

/** Öffnet den Lizenz-Dialog (beginnt immer bei der eigenen Lizenz). */
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
        title: "MIT-Lizenz",
        intro:
          "DualBeam steht unter der MIT-Lizenz. Sie erlaubt nahezu jede Verwendung – privat wie geschäftlich – und knüpft daran eine einzige Bedingung.",
        items: [
          {
            term: "Urheber",
            desc: "Copyright © 2026 N.J.",
          },
          {
            term: "Was erlaubt ist",
            desc: "Die Software darf ohne Einschränkung genutzt, kopiert, verändert, zusammengeführt, veröffentlicht, verbreitet, unterlizenziert und verkauft werden.",
          },
          {
            term: "Die einzige Bedingung",
            desc: "Der obige Urhebervermerk und der Lizenztext müssen in allen Kopien oder wesentlichen Teilen der Software enthalten bleiben.",
          },
          {
            term: "Gewährleistung und Haftung",
            desc: "Die Software wird „wie besehen“ bereitgestellt, ohne Gewährleistung jeder Art. Die Urheber haften nicht für Ansprüche, Schäden oder andere Verpflichtungen, die aus der Software oder ihrer Verwendung entstehen.",
          },
        ],
        outro:
          "Der vollständige Wortlaut liegt dem Programm als DUALBEAM-LICENSE.txt bei und steht in der Datei LICENSE des Projekts.",
      },
      {
        title: "Verwendete Module und deren Lizenzen",
        intro:
          "Genau ein fremdes Programm wird als eigene Datei mitgeliefert. Alles Weitere ist in die Programmdatei einkompiliert.",
        items: [
          {
            term: "rclone 1.74.4 — MIT",
            desc: "Copyright © 2012 Nick Craig-Wood und Mitwirkende. Stellt SFTP, FTP/FTPS sowie S3- und OpenStack-Swift-Ziele als Laufwerk bereit. rclone ist in Go geschrieben und enthält seinerseits zahlreiche Open-Source-Bibliotheken unter eigenen Lizenzen; deren Auflistung ist Teil des rclone-Projekts.",
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
            term: "DUALBEAM-LICENSE.txt",
            desc: "Die MIT-Lizenz von DualBeam samt Urhebervermerk.",
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
    ],
  },
  en: {
    title: "DualBeam – Licence",
    navLabel: "Licence topics",
    sections: [
      {
        title: "MIT licence",
        intro:
          "DualBeam is released under the MIT licence. It permits almost any use — private or commercial — and attaches a single condition.",
        items: [
          {
            term: "Copyright holder",
            desc: "Copyright © 2026 N.J.",
          },
          {
            term: "What is permitted",
            desc: "The software may be used, copied, modified, merged, published, distributed, sublicensed and sold without restriction.",
          },
          {
            term: "The only condition",
            desc: "The above copyright notice and the licence text must be included in all copies or substantial portions of the software.",
          },
          {
            term: "Warranty and liability",
            desc: "The software is provided “as is”, without warranty of any kind. The authors are not liable for any claim, damages or other liability arising from the software or its use.",
          },
        ],
        outro:
          "The full wording is bundled with the program as DUALBEAM-LICENSE.txt and lives in the project's LICENSE file.",
      },
      {
        title: "Components used and their licences",
        intro:
          "Exactly one third-party program ships as a separate file. Everything else is compiled into the executable.",
        items: [
          {
            term: "rclone 1.74.4 — MIT",
            desc: "Copyright © 2012 Nick Craig-Wood and contributors. Provides SFTP, FTP/FTPS, S3, and OpenStack Swift targets as mounted drives. rclone is written in Go and itself contains numerous open-source libraries under their own licences; that listing is part of the rclone project.",
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
            term: "DUALBEAM-LICENSE.txt",
            desc: "The MIT licence of DualBeam including its copyright notice.",
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
