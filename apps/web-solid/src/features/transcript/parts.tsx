// Small transcript rows: notices, attachments, generated images, UI blocks, the working trailer.
import { createEffect, createSignal, For, on, onCleanup, Show } from "solid-js";
import type { MessagePart } from "@/core/protocol";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { DitherRing } from "@/ui/loaders";
import { Dialog } from "@/ui/primitives";
import { copyText } from "./clipboard";
import { agentBrand, agentLabel, type AgentIdentity, fileBadgeName, fileIcon, formatElapsed, harnessIcon, harnessName, isImagePath, type MessageAttachment } from "./model";
import { imageState, loadImage } from "./resources";

type Part<K extends MessagePart["kind"]> = Extract<MessagePart, { kind: K }>;

/** Desktop `error_chip`: a tinted card whose message wraps, since failures carry exit codes and stderr. */
export function ErrorChip(props: { part: Part<"error"> }) {
  const [copied, setCopied] = createSignal(false);
  let reset: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(reset));
  const copy = async () => {
    if (!(await copyText(props.part.message))) return;
    setCopied(true);
    clearTimeout(reset);
    reset = setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div class="t-error" role="alert">
      <div class="t-error-head">
        <span class="t-error-tile"><Icon name="danger-triangle" size={12} /></span>
        <span class="t-error-label">Error</span>
        <button type="button" class="t-error-copy" aria-label={copied() ? "Copied" : "Copy message"} title="Copy message" onClick={copy}>
          <Icon name={copied() ? "check" : "copy"} size={12} />
        </button>
      </div>
      <div class="t-error-message">{props.part.message}</div>
    </div>
  );
}

/** A passive marker for a question the agent asked; the controls live in the docked panel. */
export function InputChip(props: { part: Part<"input"> }) {
  const header = () => (props.part.questions as { header?: string }[])[0]?.header ?? "";
  return (
    <div class="t-notice">
      <span class="t-notice-tile"><Icon name="chat-round-line" size={12} /></span>
      <span class="t-notice-label">Question</span>
      <span class="t-notice-value">{props.part.resolved ? header() : "Awaiting your answer…"}</span>
    </div>
  );
}

export function ForkMarker(props: { part: Part<"fork"> }) {
  return (
    <div class="t-fork" role="separator" aria-label={`Forked from ${props.part.sourceTitle}`}>
      <div class="t-fork-rule">
        <span />
        <span class="t-fork-label">Forked from</span>
        <span />
      </div>
      <div class="t-fork-title">{props.part.sourceTitle}</div>
    </div>
  );
}

/** Each word is its own flex item so the gap separates them, as desktop's children do. */
export function AgentSwitch(props: { from: AgentIdentity; to: AgentIdentity }) {
  return (
    <div class="t-agent-switch">
      <span>Model switched from</span>
      <Icon name={harnessIcon(agentBrand(props.from))} size={14} />
      <span>{agentLabel(props.from)}</span>
      <span>to</span>
      <Icon name={harnessIcon(agentBrand(props.to))} size={14} />
      <span>{agentLabel(props.to)}</span>
      <span>· Context handed off.</span>
    </div>
  );
}

/** Thumbnails and file pills above a user bubble, right-aligned. */
export function Attachments(props: { items: MessageAttachment[]; device: string | null }) {
  return (
    <div class="t-attachments">
      <For each={props.items}>
        {(item) => (
          <Show when={isImagePath(item.path)} fallback={<FilePill name={item.name} path={item.path} />}>
            <ImageThumb path={item.path} name={item.name} device={props.device} />
          </Show>
        )}
      </For>
    </div>
  );
}

function FilePill(props: { name: string; path: string }) {
  return (
    <span class="t-file-pill" title={props.path}>
      <Icon name={fileIcon(props.name)} size={14} />
      <span class="truncate">{props.name}</span>
    </span>
  );
}

function ImageThumb(props: { path: string; name: string; device: string | null }) {
  const [open, setOpen] = createSignal(false);
  let button!: HTMLButtonElement;
  createEffect(() => void loadImage(props.device, props.path));
  const image = () => imageState(props.device, props.path);
  return (
    <>
      <button ref={button} type="button" class="t-thumb" aria-label={`Open ${props.name}`} title={props.name} onClick={() => setOpen(true)}>
        <Show when={image()?.status === "ready" && image()} fallback={<Icon name={image()?.status === "failed" ? "file-image" : "gallery"} size={16} />}>
          {(ready) => <img src={(ready() as { url: string }).url} alt="" />}
        </Show>
      </button>
      <Lightbox open={open()} onClose={() => setOpen(false)} opener={() => button} name={props.name} path={props.path} device={props.device} />
    </>
  );
}

function Lightbox(props: { open: boolean; onClose: () => void; opener: () => HTMLElement | undefined; name: string; path: string; device: string | null }) {
  const image = () => imageState(props.device, props.path);
  // Opened from a plain button rather than a dialog trigger, so focus goes back to it by hand.
  const close = () => {
    props.onClose();
    queueMicrotask(() => props.opener()?.focus());
  };
  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => !open && close()}
      title={props.name}
      footer={<button type="button" class="btn" onClick={close}>Close</button>}
    >
      <div class="t-lightbox">
        <Show when={image()?.status === "ready" && image()} fallback={<span class="t-muted">{image()?.status === "failed" ? (image() as { error: string }).error : "Loading image…"}</span>}>
          {(ready) => <img src={(ready() as { url: string }).url} alt={props.name} />}
        </Show>
      </div>
    </Dialog>
  );
}

/** An image the agent produced (`image` part), read from the host that owns it. */
export function GeneratedImage(props: { part: Part<"image">; device: string | null }) {
  const [open, setOpen] = createSignal(false);
  let button: HTMLButtonElement | undefined;
  createEffect(() => void loadImage(props.device, props.part.path, props.part.mimeType));
  const image = () => imageState(props.device, props.part.path);
  return (
    <figure class="t-image">
      <Show
        when={image()?.status === "ready" && image()}
        fallback={
          <div class="t-image-placeholder" role="status">
            <Show when={image()?.status === "failed"} fallback={<span>Loading {props.part.name}…</span>}>
              <Icon name="file-image" size={16} />
              <span>{fileBadgeName(props.part.name)} could not be shown: {(image() as { error: string }).error}</span>
              <button type="button" class="t-link-button" onClick={() => void loadImage(props.device, props.part.path, props.part.mimeType, true)}>Try again</button>
            </Show>
          </div>
        }
      >
        {(ready) => (
          <button ref={button} type="button" class="t-image-button" aria-label={`Open ${props.part.name}`} onClick={() => setOpen(true)}>
            <img src={(ready() as { url: string }).url} alt={props.part.name} loading="lazy" />
          </button>
        )}
      </Show>
      <Lightbox open={open()} onClose={() => setOpen(false)} opener={() => button} name={props.part.name} path={props.part.path} device={props.device} />
    </figure>
  );
}

/** Agent UI blocks have no web renderer yet; say so plainly and keep the source readable. */
export function UiBlockNotice(props: { part: Part<"uiBlock"> }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="t-uiblock">
      <div class="t-notice">
        <span class="t-notice-tile"><Icon name="widget" size={12} /></span>
        <span class="t-notice-label">Interactive block</span>
        <span class="t-notice-value">Not supported in the web app yet. Open this chat in the desktop app to use it.</span>
        <button type="button" class="t-notice-action" aria-expanded={open()} onClick={() => setOpen(!open())}>
          {open() ? "Hide source" : "Show source"}
        </button>
      </div>
      <Show when={open()}>
        <pre class="t-uiblock-source" tabIndex={0}>{props.part.spec}</pre>
      </Show>
    </div>
  );
}

const now = (() => {
  const [tick, setTick] = createSignal(Date.now());
  let timer: ReturnType<typeof setInterval> | undefined;
  let users = 0;
  return {
    read: tick,
    acquire() {
      if (users++ === 0) timer = setInterval(() => setTick(Date.now()), 1000);
    },
    release() {
      if (--users === 0) clearInterval(timer);
    },
  };
})();

/** "<Agent> is working" with the turn's elapsed time, or the subagents the turn waits on. */
export function WorkingTrailer(props: { chatId: string }) {
  const session = () => state.sessions[props.chatId];
  const working = () => session()?.status === "working";
  const waiting = () => (working() ? 0 : Number(session()?.runningSubagents ?? 0));
  const started = () => {
    const at = session()?.startedAt;
    return typeof at === "string" ? Date.parse(at) : NaN;
  };
  const agent = () => harnessName(state.chats[props.chatId]?.config?.harness);
  const label = () =>
    waiting() > 0 ? (waiting() === 1 ? "Waiting for 1 agent" : `Waiting for ${waiting()} agents`) : `${agent()} is working`;
  return (
    <Show when={working() || waiting() > 0}>
      <div class="t-working">
        <DitherRing size={14} decorative />
        <span class="t-working-word">{label()}</span>
        <Show when={working() && Number.isFinite(started())}>
          <Elapsed since={started()} />
        </Show>
      </div>
    </Show>
  );
}

/** The turn's state for assistive tech: working, waiting, done. Streamed text and the clock are never announced. */
export function TurnStatus(props: { chatId: string }) {
  const [said, setSaid] = createSignal("");
  createEffect(
    on(
      () => {
        const session = state.sessions[props.chatId];
        const waiting = session?.status === "working" ? 0 : Number(session?.runningSubagents ?? 0);
        return session?.status === "working" ? "working" : waiting > 0 ? `waiting:${waiting}` : "idle";
      },
      (status, previous) => {
        const agent = harnessName(state.chats[props.chatId]?.config?.harness);
        if (status === "working") setSaid(`${agent} is working`);
        else if (status.startsWith("waiting:")) {
          const n = Number(status.slice(8));
          setSaid(n === 1 ? "Waiting for 1 agent" : `Waiting for ${n} agents`);
        } else setSaid(previous && previous !== "idle" ? `${agent} finished` : "");
      },
    ),
  );
  return <div class="t-sr" role="status">{said()}</div>;
}

function Elapsed(props: { since: number }) {
  now.acquire();
  onCleanup(now.release);
  return <span class="t-working-timer" aria-hidden="true">{formatElapsed((now.read() - props.since) / 1000)}</span>;
}
