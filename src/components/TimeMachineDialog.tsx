import { Show, For, createSignal, createResource } from "solid-js";
import {
  tmListBackups,
  tmDeleteBackup,
  tmWipeVolume,
  tmListWipeableVolumes,
  tmListLocalSnapshots,
  tmDeleteLocalSnapshot,
  type TmVolume,
} from "../ipc";
import { t } from "../i18n";

const [isOpen, setIsOpen] = createSignal(false);

export function openTimeMachine() {
  setIsOpen(true);
}

function closeDialog() {
  setIsOpen(false);
}

type Tab = "backups" | "snapshots";

function formatBackupDate(path: string): string {
  // Backup paths end with /YYYY-MM-DD-HHMMSS[.previous] etc.
  const m = path.match(/(\d{4})-(\d{2})-(\d{2})-(\d{2})(\d{2})(\d{2})/);
  if (!m) return path.split("/").pop() || path;
  return `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}:${m[6]}`;
}

function formatSnapshotDate(date: string): string {
  // Format: YYYY-MM-DD-HHMMSS
  const m = date.match(/^(\d{4})-(\d{2})-(\d{2})-(\d{2})(\d{2})(\d{2})$/);
  if (!m) return date;
  return `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}:${m[6]}`;
}

export function TimeMachineDialog() {
  const [tab, setTab] = createSignal<Tab>("backups");
  const [selectedVolume, setSelectedVolume] = createSignal<string>("");
  const [wipeArmed, setWipeArmed] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);

  const [vols, { refetch: refetchVols }] = createResource(isOpen, async (open) => {
    if (!open) return [] as TmVolume[];
    try {
      const list = await tmListWipeableVolumes();
      if (list.length > 0 && !selectedVolume()) setSelectedVolume(list[0].path);
      return list;
    } catch (e) {
      setErr(String(e));
      return [];
    }
  });

  const backupsKey = () => (isOpen() && tab() === "backups" ? selectedVolume() : null);
  const [backups, { refetch: refetchBackups }] = createResource(backupsKey, async (mp) => {
    if (!mp) return [] as string[];
    try {
      return await tmListBackups(mp);
    } catch (e) {
      setErr(String(e));
      return [];
    }
  });

  const snapsKey = () => isOpen() && tab() === "snapshots";
  const [snaps, { refetch: refetchSnaps }] = createResource(snapsKey, async (on) => {
    if (!on) return [] as string[];
    try {
      return await tmListLocalSnapshots();
    } catch (e) {
      setErr(String(e));
      return [];
    }
  });

  const onDeleteBackup = async (path: string) => {
    setBusy(true); setErr(null);
    try {
      await tmDeleteBackup(path);
      await refetchBackups();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onDeleteSnap = async (date: string) => {
    setBusy(true); setErr(null);
    try {
      await tmDeleteLocalSnapshot(date);
      await refetchSnaps();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onWipeVolume = async () => {
    const mp = selectedVolume();
    if (!mp) return;
    if (!wipeArmed()) {
      setWipeArmed(true);
      setErr(t("tm.wipeConfirm").replace("{mount}", mp));
      window.setTimeout(() => setWipeArmed(false), 6000);
      return;
    }
    setWipeArmed(false);
    setBusy(true); setErr(null);
    try {
      const out = await tmWipeVolume(mp);
      setErr(out || null);
      await refetchVols();
      await refetchBackups();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const refreshAll = async () => {
    setErr(null);
    await refetchVols();
    if (tab() === "backups") await refetchBackups();
    else await refetchSnaps();
  };

  return (
    <Show when={isOpen()}>
      <div
        class="modal-backdrop"
        onMouseDown={(ev) => { if (ev.target === ev.currentTarget) closeDialog(); }}
      >
        <div
          class="modal"
          style={{ "min-width": "640px", "max-width": "900px", "max-height": "80vh", display: "flex", "flex-direction": "column" }}
          onMouseDown={(e) => e.stopPropagation()}
          onKeyDown={(ev) => { ev.stopPropagation(); if (ev.key === "Escape") closeDialog(); }}
          tabIndex={-1}
          ref={(el) => queueMicrotask(() => el?.focus())}
        >

          <div style={{ display: "flex", gap: "8px", "margin-bottom": "12px", "align-items": "center" }}>
            <label style={{ "font-size": "12px", "margin-right": "6px" }}>{t("tm.wipeTarget")}</label>
            <Show when={(vols() ?? []).length > 0} fallback={<span style={{ "font-size": "12px", opacity: "0.7" }}>{t("tm.noVolumes")}</span>}>
              <select
                value={selectedVolume()}
                onChange={(e) => setSelectedVolume(e.currentTarget.value)}
                style={{ "min-width": "220px" }}
              >
                <For each={vols() ?? []}>
                  {(v) => (
                    <option value={v.path}>{v.name} ({v.path})</option>
                  )}
                </For>
              </select>
            </Show>
            <button
              class={tab() === "backups" ? "active" : "secondary"}
              onClick={() => setTab("backups")}
              disabled={busy()}
            >{t("tm.tabBackups")}</button>
            <button
              class={tab() === "snapshots" ? "active" : "secondary"}
              onClick={() => setTab("snapshots")}
              disabled={busy()}
            >{t("tm.tabSnapshots")}</button>
            <button
              class="danger"
              style={{ "margin-left": "8px" }}
              disabled={busy() || !selectedVolume()}
              onClick={() => void onWipeVolume()}
              title={t("tm.wipeHint")}
            >{wipeArmed() ? t("tm.wipeConfirmBtn") : t("tm.wipeVolume")}</button>
            <button class="secondary" style={{ "margin-left": "auto" }} onClick={() => void refreshAll()} disabled={busy()}>{t("tm.refresh")}</button>
          </div>

          <Show when={err()}>
            <div style={{ "background": "rgba(208,69,69,0.12)", "border": "1px solid rgba(208,69,69,0.5)", "padding": "6px 8px", "border-radius": "4px", "font-size": "12px", "margin-bottom": "8px", "white-space": "pre-wrap" }}>
              {err()}
            </div>
          </Show>

          <div style={{ flex: 1, "overflow-y": "auto", border: "1px solid var(--border, #888)", "border-radius": "4px", padding: "4px" }}>
            <Show when={tab() === "backups"}>
              <Show when={!backups.loading} fallback={<p style={{ padding: "8px" }}>{t("common.loading")}</p>}>
                <Show when={(backups() ?? []).length > 0} fallback={<p style={{ padding: "8px", opacity: "0.7" }}>{t("tm.noBackups")}</p>}>
                  <For each={backups() ?? []}>
                    {(b) => (
                      <div style={{ display: "flex", "align-items": "center", gap: "8px", padding: "4px 6px", "border-bottom": "1px solid var(--border-subtle, rgba(128,128,128,0.2))" }}>
                        <div style={{ flex: 1, "font-family": "monospace", "font-size": "12px" }}>
                          <div>{formatBackupDate(b)}</div>
                          <div style={{ opacity: "0.6", "font-size": "11px" }}>{b}</div>
                        </div>
                        <button class="danger" disabled={busy()} onClick={() => void onDeleteBackup(b)}>{t("common.delete")}</button>
                      </div>
                    )}
                  </For>
                </Show>
              </Show>
            </Show>

            <Show when={tab() === "snapshots"}>
              <Show when={!snaps.loading} fallback={<p style={{ padding: "8px" }}>{t("common.loading")}</p>}>
                <Show when={(snaps() ?? []).length > 0} fallback={<p style={{ padding: "8px", opacity: "0.7" }}>{t("tm.noSnapshots")}</p>}>
                  <For each={snaps() ?? []}>
                    {(s) => (
                      <div style={{ display: "flex", "align-items": "center", gap: "8px", padding: "4px 6px", "border-bottom": "1px solid var(--border-subtle, rgba(128,128,128,0.2))" }}>
                        <div style={{ flex: 1, "font-family": "monospace", "font-size": "12px" }}>
                          <div>{formatSnapshotDate(s)}</div>
                          <div style={{ opacity: "0.6", "font-size": "11px" }}>com.apple.TimeMachine.{s}.local</div>
                        </div>
                        <button class="danger" disabled={busy()} onClick={() => void onDeleteSnap(s)}>{t("common.delete")}</button>
                      </div>
                    )}
                  </For>
                </Show>
              </Show>
            </Show>
          </div>

          <p style={{ "font-size": "11px", opacity: "0.7", margin: "8px 0 0" }}>{t("tm.hint")}</p>

          <div class="modal-actions" style={{ "margin-top": "16px" }}>
            <button class="secondary" onClick={closeDialog}>{t("common.close")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
}
