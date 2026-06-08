import { Show, createSignal } from "solid-js";
import { appVersion, checkUpdate, downloadAndOpenUpdate, openUrl } from "../ipc";
import { t, errMsg } from "../i18n";

const [open, setOpen] = createSignal(false);
const [version, setVersion] = createSignal("");

const REPO_URL = "https://github.com/nojan01/macos-dualpane";

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
  const [status, setStatus] = createSignal<string>("");
  const [checking, setChecking] = createSignal(false);
  const [downloading, setDownloading] = createSignal(false);
  // Direkte DMG-URL (falls vorhanden) und Release-Seite als Fallback.
  const [assetUrl, setAssetUrl] = createSignal<string | null>(null);
  const [pageUrl, setPageUrl] = createSignal<string | null>(null);

  function close() {
    setOpen(false);
    setStatus("");
    setAssetUrl(null);
    setPageUrl(null);
  }

  async function doCheck() {
    if (checking()) return;
    setChecking(true);
    setAssetUrl(null);
    setPageUrl(null);
    setStatus(t("about.checking"));
    try {
      const info = await checkUpdate();
      if (info.updateAvailable) {
        setStatus(t("about.updateAvailable", { version: info.latest, current: info.current }));
        setAssetUrl(info.assetUrl || null);
        setPageUrl(info.url);
      } else {
        setStatus(t("about.upToDate", { version: info.current }));
      }
    } catch (err) {
      setStatus(errMsg(err));
    } finally {
      setChecking(false);
    }
  }

  async function doDownload() {
    const u = assetUrl();
    if (!u || downloading()) return;
    setDownloading(true);
    setStatus(t("about.downloading"));
    try {
      await downloadAndOpenUpdate(u);
      setStatus(t("about.downloaded"));
    } catch (err) {
      setStatus(errMsg(err));
    } finally {
      setDownloading(false);
    }
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
            if (ev.key === "Escape") { ev.preventDefault(); close(); }
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
            <dt>{t("about.website")}</dt>
            <dd>
              <a
                href={REPO_URL}
                onClick={(e) => { e.preventDefault(); void openUrl(REPO_URL); }}
              >{REPO_URL}</a>
            </dd>
          </dl>
          <Show when={status()}>
            <p class="about-status pre-wrap">{status()}</p>
          </Show>
          <div class="modal-actions">
            <Show when={assetUrl()}>
              <button disabled={downloading()} onClick={doDownload}>
                {downloading() ? t("about.downloading") : t("about.updateInstall")}
              </button>
            </Show>
            <Show when={pageUrl() && !assetUrl()}>
              <button onClick={() => { const u = pageUrl(); if (u) void openUrl(u); }}>
                {t("about.updateOpen")}
              </button>
            </Show>
            <button class="secondary" disabled={checking() || downloading()} onClick={doCheck}>
              {t("about.checkUpdates")}
            </button>
            <button onClick={close}>{t("common.close")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
}
