import { Show, createSignal } from "solid-js";
import { appVersion } from "../ipc";
import { t } from "../i18n";

const [open, setOpen] = createSignal(false);
const [version, setVersion] = createSignal("");

/** Öffnet den „Über DualBeam"-Dialog. */
export async function openAbout() {
  try {
    setVersion(await appVersion());
  } catch {
    setVersion("");
  }
  setOpen(true);
}

export function AboutDialog() {
  function close() {
    setOpen(false);
  }

  return (
    <Show when={open()}>
      <div class="modal-backdrop" onMouseDown={close}>
        <div
          class="modal about-modal"
          role="dialog"
          aria-modal="true"
          aria-label={t("about.title")}
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
          <h2>{t("about.title")}</h2>
          <p class="about-tagline">{t("about.tagline")}</p>
          <dl class="about-grid">
            <dt>{t("about.version")}</dt>
            <dd>{version() || "—"}</dd>
            <dt>{t("about.author")}</dt>
            <dd>N.J.</dd>
            <dt>{t("about.license")}</dt>
            <dd>MIT</dd>
          </dl>
          <div class="modal-actions">
            <button onClick={close}>{t("common.close")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
}
