import { Show, createSignal } from "solid-js";
import { t } from "../i18n";

// ----- Prompt -----
type PromptState = {
  title: string;
  label?: string;
  defaultValue: string;
  placeholder?: string;
  okLabel: string;
};
const [promptState, setPromptState] = createSignal<PromptState | null>(null);
let promptResolve: ((v: string | null) => void) | null = null;

export function askPrompt(opts: {
  title: string;
  label?: string;
  defaultValue?: string;
  placeholder?: string;
  okLabel?: string;
}): Promise<string | null> {
  setPromptState({
    title: opts.title,
    label: opts.label,
    defaultValue: opts.defaultValue ?? "",
    placeholder: opts.placeholder,
    okLabel: opts.okLabel ?? "OK",
  });
  return new Promise((res) => { promptResolve = res; });
}

function resolvePrompt(v: string | null) {
  setPromptState(null);
  const r = promptResolve; promptResolve = null;
  r?.(v);
}

// ----- Confirm -----
type ConfirmState = {
  title: string;
  message?: string;
  okLabel: string;
  cancelLabel: string;
  danger: boolean;
};
const [confirmState, setConfirmState] = createSignal<ConfirmState | null>(null);
let confirmResolve: ((v: boolean) => void) | null = null;

export function askConfirm(opts: {
  title: string;
  message?: string;
  okLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
}): Promise<boolean> {
  setConfirmState({
    title: opts.title,
    message: opts.message,
    okLabel: opts.okLabel ?? "OK",
    cancelLabel: opts.cancelLabel ?? t("common.cancel"),
    danger: !!opts.danger,
  });
  return new Promise((res) => { confirmResolve = res; });
}

function resolveConfirm(v: boolean) {
  setConfirmState(null);
  const r = confirmResolve; confirmResolve = null;
  r?.(v);
}

// ----- Notify (Alert) -----
type NotifyState = {
  title: string;
  message?: string;
  okLabel: string;
  danger: boolean;
};
const [notifyState, setNotifyState] = createSignal<NotifyState | null>(null);
let notifyResolve: (() => void) | null = null;

export function notify(opts: {
  title: string;
  message?: string;
  okLabel?: string;
  danger?: boolean;
}): Promise<void> {
  setNotifyState({
    title: opts.title,
    message: opts.message,
    okLabel: opts.okLabel ?? "OK",
    danger: !!opts.danger,
  });
  return new Promise((res) => { notifyResolve = res; });
}

/** Bequemer Helfer für Fehlermeldungen. */
export function notifyError(message: string): Promise<void> {
  return notify({ title: t("common.errorTitle"), message, danger: true });
}

function resolveNotify() {
  setNotifyState(null);
  const r = notifyResolve; notifyResolve = null;
  r?.();
}

export function Dialogs() {
  return (
    <>
      <Show when={promptState()}>
        {(s) => {
          let inputEl: HTMLInputElement | undefined;
          const [val, setVal] = createSignal(s().defaultValue);
          queueMicrotask(() => {
            if (inputEl) {
              inputEl.focus();
              const dot = inputEl.value.lastIndexOf(".");
              inputEl.setSelectionRange(0, dot > 0 ? dot : inputEl.value.length);
            }
          });
          const submit = () => {
            const v = val();
            if (!v) return;
            resolvePrompt(v);
          };
          return (
            <div class="modal-backdrop" onMouseDown={() => resolvePrompt(null)}>
              <div class="modal" role="dialog" aria-modal="true" aria-label={s().title} onMouseDown={(e) => e.stopPropagation()}>
                <h2>{s().title}</h2>
                <Show when={s().label}><p>{s().label}</p></Show>
                <input
                  ref={inputEl}
                  type="text"
                  class="prompt-input"
                  value={val()}
                  placeholder={s().placeholder}
                  onInput={(e) => setVal(e.currentTarget.value)}
                  onKeyDown={(ev) => {
                    ev.stopPropagation();
                    if (ev.key === "Enter") { ev.preventDefault(); submit(); }
                    else if (ev.key === "Escape") { ev.preventDefault(); resolvePrompt(null); }
                  }}
                />
                <div class="modal-actions">
                  <button onClick={submit}>{s().okLabel}</button>
                  <button class="secondary" onClick={() => resolvePrompt(null)}>{t("common.cancel")}</button>
                </div>
              </div>
            </div>
          );
        }}
      </Show>

      <Show when={confirmState()}>
        {(s) => (
          <div class="modal-backdrop" onMouseDown={() => resolveConfirm(false)}>
            <div
              class="modal"
              role="dialog"
              aria-modal="true"
              aria-label={s().title}
              onMouseDown={(e) => e.stopPropagation()}
              tabIndex={-1}
              ref={(el) => queueMicrotask(() => el?.focus())}
              onKeyDown={(ev) => {
                ev.stopPropagation();
                if (ev.key === "Enter") { ev.preventDefault(); resolveConfirm(true); }
                else if (ev.key === "Escape") { ev.preventDefault(); resolveConfirm(false); }
              }}
            >
              <h2>{s().title}</h2>
              <Show when={s().message}><p>{s().message}</p></Show>
              <div class="modal-actions">
                <button
                  class={s().danger ? "danger" : ""}
                  onClick={() => resolveConfirm(true)}
                >{s().okLabel}</button>
                <button class="secondary" onClick={() => resolveConfirm(false)}>{s().cancelLabel}</button>
              </div>
            </div>
          </div>
        )}
      </Show>

      <Show when={notifyState()}>
        {(s) => (
          <div class="modal-backdrop" onMouseDown={() => resolveNotify()}>
            <div
              class="modal"
              role="dialog"
              aria-modal="true"
              aria-label={s().title}
              onMouseDown={(e) => e.stopPropagation()}
              tabIndex={-1}
              ref={(el) => queueMicrotask(() => el?.focus())}
              onKeyDown={(ev) => {
                ev.stopPropagation();
                if (ev.key === "Enter" || ev.key === "Escape") { ev.preventDefault(); resolveNotify(); }
              }}
            >
              <h2>{s().title}</h2>
              <Show when={s().message}><p class="pre-wrap">{s().message}</p></Show>
              <div class="modal-actions">
                <button class={s().danger ? "danger" : ""} onClick={() => resolveNotify()}>{s().okLabel}</button>
              </div>
            </div>
          </div>
        )}
      </Show>
    </>
  );
}
