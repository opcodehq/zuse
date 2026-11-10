// The attached tray: notices, staged attachments, sign-in and error rows, and the message queue.
import { writeClipboard } from "@/ui/clipboard";
import { createEffect, createMemo, createSignal, For, type JSX, Match, on, Show, Switch } from "solid-js";
import type { MessagePart } from "@/core/protocol";
import { state } from "@/core/store";
import { navigate, view } from "@/core/view";
import { ProjectCreationNotice } from "@/features/projects/CreationNotice";
import { Dialog } from "@kobalte/core/dialog";
import { Icon } from "@/ui/Icon";
import { Spinner, toast } from "@/ui/primitives";
import { deviceOnline, draftSpace, resolve, selectedChat } from "./catalog";
import { attachmentsFor, clearFailure, composer, focusComposer, removeAttachment, setComposer } from "./drafts";
import { cancelLogin, login, providerName, reopenLoginPage, signsIn, startLogin } from "./login";
import { beginQueueEdit, cancelQueueEdit, commitQueueEdit, queueEdit, queueTitle, type QueuedMessage } from "./queueEdit";
import { draftKey, hasContent, lastPrompt, queueAction, retry, runLive } from "./send";

type Tone = "default" | "warning" | "danger" | "accent";

/** One tray row: icon, medium title, muted subtitle, trailing actions, and optional details below. */
export function TrayPill(props: {
  tone?: Tone;
  icon?: string;
  iconElement?: JSX.Element;
  title: string;
  subtitle?: string;
  details?: string;
  role?: "status" | "alert";
  children?: JSX.Element;
}) {
  const [expanded, setExpanded] = createSignal(false);
  return (
    <div role={props.role ?? "status"}>
      <div class="cmp-pill" data-tone={props.tone ?? "default"}>
        <span class="cmp-pill-icon">
          <Show when={props.iconElement} fallback={<Icon name={props.icon ?? "info-circle"} size={14} />}>{props.iconElement}</Show>
        </span>
        <Show
          when={props.details}
          fallback={
            <span class="cmp-pill-body">
              <span class="cmp-pill-title">{props.title}</span>
              <Show when={props.subtitle && props.subtitle !== props.title}><span class="cmp-pill-sub">{props.subtitle}</span></Show>
            </span>
          }
        >
          <button
            type="button"
            class="cmp-pill-body"
            aria-expanded={expanded()}
            aria-label={`${expanded() ? "Collapse" : "Expand"} ${props.title}`}
            onClick={() => setExpanded((v) => !v)}
          >
            <span class="cmp-pill-title">{props.title}</span>
            <Show when={props.subtitle && props.subtitle !== props.title}><span class="cmp-pill-sub">{props.subtitle}</span></Show>
          </button>
        </Show>
        <Show when={props.children}>
          <span class="cmp-pill-actions">{props.children}</span>
        </Show>
      </div>
      <Show when={expanded() && props.details}>
        <div class="cmp-pill-details">{props.details}</div>
      </Show>
    </div>
  );
}

function CopyDismiss(props: { copy: string; onDismiss: () => void }) {
  return (
    <>
      <button
        type="button"
        class="cmp-icon-action"
        aria-label="Copy details"
        title="Copy details"
        onClick={() => void writeClipboard(props.copy).then((ok) => (ok ? toast("Copied") : toast("Couldn't copy", "danger")))}
      >
        <Icon name="copy" size={14} />
      </button>
      <button type="button" class="cmp-icon-action" aria-label="Dismiss" title="Dismiss" onClick={() => props.onDismiss()}>
        <Icon name="close" size={14} />
      </button>
    </>
  );
}

type ErrorKind = "usage" | "reconnecting" | "signIn" | "generic";
const ERROR_COPY: Record<ErrorKind, string> = {
  usage: "Usage limit reached",
  reconnecting: "Reconnecting",
  signIn: "Sign-in expired",
  generic: "Run failed",
};

/** crates/ui/src/composer/tray.rs classify_provider_error. */
export function classifyError(message: string): ErrorKind {
  const text = message.toLowerCase();
  const any = (needles: string[]) => needles.some((n) => text.includes(n));
  if (any(["usage limit", "rate limit", "rate_limit", "ratelimit", "quota", "limit reached", "hit your limit", "too many requests", " 429", "429 "])) return "usage";
  if (
    any([
      "not logged in", "not signed in", "please run /login", "please log in", "please sign in", "signed out", "log in again",
      "sign in again", "unauthorized", "unauthenticated", "authentication", "invalid api key", "invalid_api_key", "oauth token",
    ])
  )
    return "signIn";
  if (any(["reconnecting", "connection lost", "connection reset", "temporarily unavailable"])) return "reconnecting";
  return "generic";
}

/** The newest run's provider error, when the transcript ends on one. */
function latestError(): { id: string; message: string } | null {
  const messages = state.transcript?.messages ?? [];
  const last = messages[messages.length - 1];
  if (!last || last.role !== "assistant") return null;
  const errors = (last.parts as MessagePart[]).flatMap((part) =>
    part.kind === "error" && part.message.trim() ? [{ id: `${last.id}#${part.id}`, message: part.message.trim() }] : [],
  );
  // A failed turn often ends with a generic summary after the provider's specific error; prefer the specific one.
  return [...errors].reverse().find((e) => classifyError(e.message) !== "generic") ?? errors[errors.length - 1] ?? null;
}
const firstLine = (text: string) => text.split("\n").map((l) => l.trim()).find(Boolean) ?? "";

/** The chat's host is offline while the engine link is up. A down link has the window's connection strip instead. */
function deliveryDegraded() {
  if (state.connection.kind !== "connected") return false;
  const chat = selectedChat();
  const device = chat?.deviceId ?? draftSpace()?.deviceId;
  return !!device && !deviceOnline(device);
}

function ConnectionRow() {
  return (
    <Show when={deliveryDegraded()}>
      <TrayPill icon="refresh" title="Reconnecting" subtitle="Messages will send once the connection recovers." />
    </Show>
  );
}

function TranscriptNotice() {
  return (
    <Show when={view.selectedChatId && state.transcript?.notice}>
      {(notice) => <TrayPill icon="refresh" title={notice()} />}
    </Show>
  );
}

function SendFailure() {
  const failure = () => (composer.failure?.key === draftKey() ? composer.failure : null);
  return (
    <Show when={failure()}>
      {(f) => {
        const offline = () => f().message === "Engine not connected";
        return (
          <TrayPill
            role="alert"
            tone={offline() ? "warning" : "danger"}
            icon={offline() ? "wifi-off" : "alert-circle"}
            title={offline() ? "Engine not connected" : "Couldn't send"}
            subtitle={offline() ? undefined : f().message}
            details={offline() ? undefined : f().message}
          >
            <CopyDismiss copy={f().message} onDismiss={clearFailure} />
          </TrayPill>
        );
      }}
    </Show>
  );
}

function ProviderRows() {
  const chatId = () => view.selectedChatId;
  const harness = () => resolve().harness;
  const error = createMemo(() => {
    const id = chatId();
    if (!id || runLive(id)) return null;
    const e = latestError();
    return e && composer.dismissedErrors[id] !== e.id ? e : null;
  });
  const kind = () => (error() ? classifyError(error()!.message) : null);
  const dismiss = (id: string) => {
    const chat = chatId();
    if (chat) setComposer("dismissedErrors", chat, id);
  };
  const activeLogin = () => (login()?.chatId === chatId() ? login() : null);
  const showSignIn = () => signsIn(harness()) && (activeLogin() || kind() === "signIn");
  const signIn = () => {
    const id = chatId();
    const h = harness();
    if (!id || !h) return;
    void startLogin(id, h, kind() === "signIn" ? error()?.id : activeLogin()?.errorId, (resolved) => resolved && dismiss(resolved));
  };
  return (
    <>
      <Show when={showSignIn()}>
        <Switch>
          <Match when={activeLogin() && activeLogin()!.step !== "failed"}>
            <TrayPill
              iconElement={<Spinner label="Signing in" cell={2.5} />}
              title={`Signed out of ${providerName(harness())}`}
              subtitle={activeLogin()!.step === "starting" ? `Starting ${providerName(harness())} sign-in…` : activeLogin()!.message || "Waiting for browser sign-in…"}
            >
              <Show when={activeLogin()!.url}>
                <button type="button" class="cmp-text-action" onClick={reopenLoginPage}>
                  <Icon name="arrow-up-right" size={14} style={{ color: "var(--text-muted)" }} />
                  Open browser again
                </button>
              </Show>
              <button type="button" class="cmp-text-action" onClick={cancelLogin}>Cancel</button>
            </TrayPill>
          </Match>
          <Match when={true}>
            <TrayPill icon="logout-2" title={`Signed out of ${providerName(harness())}`} subtitle={activeLogin()?.message ?? "Sign in to continue this chat."}>
              <button type="button" class="cmp-text-action" onClick={signIn} disabled={state.connection.kind !== "connected"}>
                {activeLogin() ? "Retry" : "Sign in"}
              </button>
            </TrayPill>
          </Match>
        </Switch>
      </Show>
      <Show when={!(kind() === "signIn" && signsIn(harness())) ? error() : null}>
        {(current) => {
          const e = current;
          const k = () => kind()!;
          const prompt = () => (k() !== "reconnecting" && !hasContent() ? lastPrompt() : null);
          return (
            <TrayPill
              role="alert"
              tone={k() === "usage" ? "warning" : k() === "reconnecting" ? "default" : "danger"}
              icon={k() === "usage" ? "dashboard-speed" : k() === "reconnecting" ? "refresh" : "alert-circle"}
              title={ERROR_COPY[k()]}
              subtitle={firstLine(e().message)}
              details={e().message}
            >
              <Show when={prompt()}>
                {(text) => (
                  <button
                    type="button"
                    class="cmp-text-action"
                    onClick={() => {
                      dismiss(e().id);
                      void retry(text());
                    }}
                  >
                    Retry
                  </button>
                )}
              </Show>
              <Show when={k() === "usage" || k() === "signIn"}>
                <button type="button" class="cmp-text-action" onClick={() => navigate({ kind: "settings", section: "providers" })}>Settings</button>
              </Show>
              <CopyDismiss copy={e().message} onDismiss={() => dismiss(e().id)} />
            </TrayPill>
          );
        }}
      </Show>
    </>
  );
}

const sizeText = (bytes: number) => (bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(0)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`);

const [lightbox, setLightbox] = createSignal<{ src: string; name: string } | null>(null);

/** Full-size view of a staged image: dimmed backdrop, the image at most 90% by 85% of the window, its name below. */
function Lightbox() {
  return (
    <Dialog open={!!lightbox()} onOpenChange={(open) => !open && setLightbox(null)}>
      <Dialog.Portal>
        <Dialog.Overlay class="cmp-lightbox-scrim" />
        <Dialog.Content class="cmp-lightbox" aria-label={lightbox()?.name} onClick={() => setLightbox(null)} onCloseAutoFocus={(event) => { event.preventDefault(); focusComposer(); }}>
          <img src={lightbox()?.src} alt={lightbox()?.name ?? ""} />
          <span class="cmp-lightbox-name">{lightbox()?.name}</span>
          <Dialog.CloseButton class="cmp-sr">Close preview</Dialog.CloseButton>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog>
  );
}

function AttachmentsRow() {
  const key = () => draftKey();
  return (
    <>
    <Lightbox />
    <Show when={attachmentsFor(key()).length}>
      <div class="cmp-attachments" role="list" aria-label="Attachments">
        <For each={attachmentsFor(key())}>
          {(item) => {
            const state = () => (item.error ? "error" : item.path ? "ready" : "uploading");
            const label = () => `${item.name}, ${sizeText(item.size)}${item.error ? `, ${item.error}` : state() === "uploading" ? ", uploading" : ""}`;
            // A rejected or unreadable image falls back to the file chip, which can show the error.
            const [broken, setBroken] = createSignal(false);
            const preview = () => (item.error || broken() ? undefined : item.preview);
            return (
              <div class="cmp-thumb" classList={{ "cmp-thumb-file": !preview() }} data-state={state()} role="listitem" title={item.error ?? item.name} aria-label={label()}>
                <Show when={preview()} fallback={<><Icon name="document" size={14} /><span class="cmp-truncate">{item.name}</span></>}>
                  {(src) => (
                    <button type="button" class="cmp-thumb-open" aria-label={`Preview ${item.name}`} onClick={() => setLightbox({ src: src(), name: item.name })}>
                      <img src={src()} alt="" onError={() => setBroken(true)} />
                    </button>
                  )}
                </Show>
                <Show when={state() === "uploading" && preview()}>
                  <span class="cmp-thumb-overlay"><Spinner label="Uploading" /></span>
                </Show>
                <Show when={state() === "uploading" && !preview()}><Spinner label="Uploading" /></Show>
                <Show when={state() === "error"}><Icon name="alert-circle" size={12} style={{ color: "var(--danger)" }} /></Show>
                <button type="button" class="cmp-thumb-remove" aria-label={`Remove ${item.name}`} onClick={() => {
                  removeAttachment(key(), item.id);
                  focusComposer();
                }}>
                  <Icon name="close" size={10} />
                </button>
              </div>
            );
          }}
        </For>
      </div>
    </Show>
    </>
  );
}

type Primary = "steer" | "sendNext" | "sendNow";
const PRIMARY_LABEL: Record<Primary, string> = { steer: "Steer", sendNext: "Send next", sendNow: "Send now" };
const PRIMARY_TIP: Record<Primary, string> = {
  steer: "Steer (keep current work running)",
  sendNext: "Send at the next turn (this agent cannot steer mid-turn)",
  sendNow: "Send now (interrupt)",
};

/** crates/protocol/src/presentation/queue.rs queue_primary_action. */
function primaryFor(row: QueuedMessage): Primary | null {
  if (row.deliveryGate) return null;
  if (row.attachments?.length) return "sendNow";
  const h = resolve().harnessInfo;
  return h?.supportsSteering && h.steeringMode === "step-boundary" ? "steer" : "sendNext";
}

export function queuedRows(chatId: string | null): QueuedMessage[] {
  return chatId ? ((state.queue[chatId] ?? []) as QueuedMessage[]) : [];
}

/** Mod+Enter on an empty composer delivers the first waiting message. */
export function activateFirstQueued() {
  const chatId = view.selectedChatId;
  const row = queuedRows(chatId)[0];
  const action = row && primaryFor(row);
  if (!chatId || !row || !action) return false;
  void queueAction(action === "sendNow" ? "sendNow" : "steer", chatId, row.id);
  return true;
}

function QueuePanel() {
  const chatId = () => view.selectedChatId;
  const rows = () => queuedRows(chatId());
  const [drag, setDrag] = createSignal<{ from: number; over: number } | null>(null);
  const [removing, setRemoving] = createSignal<Record<string, boolean>>({});
  // Keyboard reorders re-render the rows; keep the caret on the moved row's handle.
  let refocus: string | null = null;
  let list: HTMLDivElement | undefined;
  createEffect(on(rows, () => {
    if (!refocus || !list) return;
    const handle = list.querySelector<HTMLElement>(`[data-queue-id="${CSS.escape(refocus)}"] .cmp-queue-handle`);
    if (handle) {
      handle.focus();
      refocus = null;
    }
  }));
  const move = (from: number, to: number, keyboard = false) => {
    const id = rows()[from]?.id;
    const chat = chatId();
    if (!id || !chat || from === to || to < 0 || to >= rows().length) return;
    if (keyboard) refocus = id;
    void queueAction("move", chat, id, to);
  };
  const remove = async (id: string) => {
    const chat = chatId();
    if (!chat) return;
    setRemoving((r) => ({ ...r, [id]: true }));
    await queueAction("remove", chat, id);
    setRemoving((r) => ({ ...r, [id]: false }));
  };
  const edit = (row: QueuedMessage) => void beginQueueEdit(chatId()!, row).then(focusComposer);
  const deviceName = (id: string | undefined) => (id ? (id === state.localDeviceId ? "this computer" : state.devices[id]?.name ?? id) : "another device");
  const actions = (row: QueuedMessage) => (
    <>
      <button type="button" class="cmp-queue-action" data-icon="true" aria-label={removing()[row.id] ? "Removing…" : "Remove"} title="Remove" disabled={removing()[row.id]} onClick={() => void remove(row.id)}>
        <Icon name="trash-bin-minimalistic" size={13} />
      </button>
      <button type="button" class="cmp-queue-action" data-icon="true" aria-label="Edit" title="Edit" disabled={!!queueEdit()} onClick={() => edit(row)}>
        <Icon name="pen" size={13} />
      </button>
    </>
  );
  return (
    <Show when={rows().length}>
      <div class="cmp-queue" role="region" aria-label="Queued messages">
        <div class="cmp-queue-head">{state.connection.kind !== "connected" || deliveryDegraded() ? "Waiting for connection" : "Queued"}</div>
        <div class="cmp-queue-list" ref={list}>
          <For each={rows()}>
            {(row, index) => {
              const editing = () => queueEdit()?.id === row.id;
              const gate = () => row.deliveryGate;
              const primary = () => primaryFor(row);
              return (
                <div
                  class="cmp-queue-row"
                  data-queue-id={row.id}
                  data-editing={editing()}
                  data-removing={!!removing()[row.id]}
                  data-dragging={drag()?.from === index()}
                  data-drop={!!drag() && drag()!.over === index() && drag()!.from !== index()}
                  onDragOver={(event) => {
                    if (!drag()) return;
                    event.preventDefault();
                    setDrag({ from: drag()!.from, over: index() });
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    const d = drag();
                    setDrag(null);
                    if (d) move(d.from, index());
                  }}
                >
                  <Show when={!editing()} fallback={<span style={{ width: "14px", "flex-shrink": 0 }} />}>
                    <span
                      class="cmp-queue-handle"
                      draggable={rows().length > 1}
                      tabIndex={rows().length > 1 ? 0 : -1}
                      role="button"
                      aria-label={`Reorder: ${queueTitle(row)}. Use Alt+Up and Alt+Down to move.`}
                      onDragStart={(event) => {
                        event.dataTransfer?.setData("text/plain", row.id);
                        setDrag({ from: index(), over: index() });
                      }}
                      onDragEnd={() => setDrag(null)}
                      onKeyDown={(event) => {
                        if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
                          event.preventDefault();
                          move(index(), index() + (event.key === "ArrowUp" ? -1 : 1), true);
                        }
                      }}
                    >
                      <Icon name="queue-drag-handle" size={13} />
                    </span>
                  </Show>
                  <span class="cmp-queue-text" data-muted={editing()} title={editing() ? undefined : queueTitle(row)}>
                    {editing() ? (queueEdit()?.finishing ? "Saving…" : "Editing in composer") : queueTitle(row)}
                  </span>
                  <Show when={row.attachments?.length && !editing()}>
                    <span class="cmp-queue-meta" aria-label={`${row.attachments!.length} attachments`}>
                      <Icon name="queue-paperclip" size={12} /> {row.attachments!.length}
                    </span>
                  </Show>
                  <span class="cmp-queue-actions">
                    <Switch>
                      <Match when={editing()}>
                        <button type="button" class="cmp-queue-action" data-icon="true" aria-label="Save to queue" title="Save to queue" disabled={queueEdit()?.finishing || !queueEdit()?.lease} onClick={() => commitQueueEdit()}>
                          <Icon name="queue-check" size={13} />
                        </button>
                        <button type="button" class="cmp-queue-action" data-icon="true" aria-label="Cancel" title="Cancel" disabled={queueEdit()?.finishing} onClick={() => cancelQueueEdit()}>
                          <Icon name="queue-close" size={13} />
                        </button>
                      </Match>
                      <Match when={gate()?.kind === "editing"}>
                        <span class="cmp-queue-meta">Editing on {deviceName(gate()!.ownerDeviceId)}</span>
                      </Match>
                      <Match when={gate()?.kind === "reviewRequired"}>
                        <span class="cmp-queue-meta">Needs review</span>
                        {actions(row)}
                      </Match>
                      <Match when={true}>
                        {actions(row)}
                        <button
                          type="button"
                          class="cmp-queue-action"
                          data-primary="true"
                          aria-label={primary() ? PRIMARY_TIP[primary()!] : "Waiting for provider capabilities"}
                          title={primary() ? PRIMARY_TIP[primary()!] : "Waiting for provider capabilities"}
                          disabled={!primary() || state.connection.kind !== "connected"}
                          onClick={() => {
                            const p = primary();
                            if (p) void queueAction(p === "sendNow" ? "sendNow" : "steer", chatId()!, row.id);
                          }}
                        >
                          {primary() ? PRIMARY_LABEL[primary()!] : "Send"}
                        </button>
                      </Match>
                    </Switch>
                  </span>
                </div>
              );
            }}
          </For>
        </div>
      </div>
    </Show>
  );
}

/** Every tray row for the current composer, in desktop's order. */
export function TrayRows() {
  const spaceId = () => selectedChat()?.spaceId ?? draftSpace()?.id;
  return (
    <>
      <ConnectionRow />
      <TranscriptNotice />
      <ProjectCreationNotice spaceId={spaceId()} />
      <AttachmentsRow />
      <SendFailure />
      <ProviderRows />
      <QueuePanel />
    </>
  );
}
