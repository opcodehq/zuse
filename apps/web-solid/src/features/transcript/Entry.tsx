// One message entry: a user bubble or an assistant turn, plus its hover metadata strip.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { createMemo, createSignal, For, lazy, Match, onCleanup, Show, Switch } from "solid-js";
import type { MessagePart, SessionMessageEntry } from "@/core/protocol";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { FORK_MENU, forkChat } from "./actions";
import { copyText } from "./clipboard";
import { frameThrottled } from "./frame";
import { buildItems, foldsTurn, type Item } from "./layout";
import { Markdown } from "./Markdown";
import {
  type AgentIdentity,
  assistantCopyText,
  formatElapsed,
  formatTimestamp,
  parseAttachments,
  userNeedsCollapse,
  userRawText,
} from "./model";
import { AgentSwitch, Attachments, ErrorChip, ForkMarker, GeneratedImage, InputChip, UiBlockNotice } from "./parts";
import { isOpen, toggleOpen } from "./resources";
import { FoldHeader, type GroupContext, ToolGroup } from "./ToolGroup";

// Agent visuals are rare; their renderer loads with the first one.
const VisualPart = lazy(() => import("@/features/extras/Visual").then((m) => ({ default: m.VisualPart })));

const sameList = <T,>(a: readonly T[], b: readonly T[]) => a.length === b.length && a.every((v, i) => v === b[i]);

export interface EntryContext {
  chatId: string;
  /** The device that owns the chat's files. */
  device: () => string | null;
  harness: () => string | undefined;
  /** True for the newest entry of the transcript. */
  last: () => boolean;
}

export function Entry(props: { entry: SessionMessageEntry; ctx: EntryContext }) {
  return (
    <Show when={props.entry.role === "user"} fallback={<AssistantEntry entry={props.entry} ctx={props.ctx} />}>
      <UserEntry entry={props.entry} ctx={props.ctx} />
    </Show>
  );
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

function UserEntry(props: { entry: SessionMessageEntry; ctx: EntryContext }) {
  const parsed = createMemo(() => parseAttachments(userRawText(props.entry)));
  const switches = createMemo(() =>
    props.entry.parts.flatMap((part) =>
      part.kind === "agentContext" && part.previousAgent ? [{ from: part.previousAgent as AgentIdentity, to: part.agent as AgentIdentity }] : [],
    ),
  );
  const foldKey = () => `${props.entry.id}#user`;
  const collapsible = () => userNeedsCollapse(parsed().text);
  const expanded = () => isOpen(foldKey());
  const bodyId = () => `${props.entry.id}-body`;
  return (
    <div class="t-entry" data-role="user">
      <For each={switches()}>{(s) => <AgentSwitch from={s.from} to={s.to} />}</For>
      <Show when={parsed().attachments.length}>
        <Attachments items={parsed().attachments} device={props.ctx.device()} />
      </Show>
      <Show when={parsed().text}>
        <div class="t-user-row">
          <div class="t-bubble">
            <div id={bodyId()} class="t-bubble-text" data-collapsed={collapsible() && !expanded() ? "true" : undefined}>
              {parsed().text}
            </div>
            <Show when={collapsible()}>
              <button type="button" class="t-bubble-toggle" aria-expanded={expanded()} aria-controls={bodyId()} onClick={() => toggleOpen(foldKey())}>
                {expanded() ? "Show less" : "Show more"}
              </button>
            </Show>
          </div>
        </div>
      </Show>
      <MetaStrip entry={props.entry} copy={parsed().text} align="end" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Assistant
// ---------------------------------------------------------------------------

function AssistantEntry(props: { entry: SessionMessageEntry; ctx: EntryContext }) {
  const streaming = () => props.entry.status === "streaming";
  // Visibility of text parts only flips once per part, so this never rebuilds items on a token.
  const filled = createMemo(
    () => props.entry.parts.map((p) => (p.kind === "text" || p.kind === "reasoning" ? /\S/.test(p.text) : true)),
    [],
    { equals: sameList },
  );
  const compact = () => foldsTurn(props.entry) || view.preferences.transcriptCompactMode === true;
  const items = createMemo(() => buildItems(props.entry.id, props.entry.parts, filled(), streaming(), compact()));
  const settledCopy = () => (streaming() ? "" : assistantCopyText(props.entry));
  return (
    <div class="t-entry" data-role="assistant">
      <ItemList items={items()} entry={props.entry} ctx={props.ctx} />
      <Show when={!streaming()}>
        <MetaStrip entry={props.entry} copy={settledCopy()} align="start" fork={!!settledCopy()} chatId={props.ctx.chatId} />
      </Show>
    </div>
  );
}

/** Renders items keyed by `item.key`, so a rebuilt item list keeps every mounted row. */
function ItemList(props: { items: Item[]; entry: SessionMessageEntry; ctx: EntryContext }) {
  const keys = createMemo(() => props.items.map((item) => item.key), [], { equals: sameList });
  const byKey = createMemo(() => new Map(props.items.map((item) => [item.key, item])));
  return (
    <For each={keys()}>
      {(key) => {
        const item = () => byKey().get(key);
        return <Show when={item()}>{(it) => <ItemView item={it()} entry={props.entry} ctx={props.ctx} />}</Show>;
      }}
    </For>
  );
}

function ItemView(props: { item: Item; entry: SessionMessageEntry; ctx: EntryContext }) {
  const group: GroupContext = { harness: () => props.ctx.harness() };
  const streaming = () => props.entry.status === "streaming";
  return (
    <div class="t-item" data-kind={props.item.kind}>
      <Switch>
        <Match when={props.item.kind === "text" && props.item}>
          {(it) => <TextItem part={it().part} live={streaming()} />}
        </Match>
        <Match when={props.item.kind === "group" && props.item}>
          {(it) => <ToolGroup id={it().key} tools={it().tools} live={it().live} ctx={group} />}
        </Match>
        <Match when={props.item.kind === "fold" && props.item}>
          {(it) => (
            <FoldHeader id={it().key} tools={it().tools} live={it().live}>
              <ItemList items={it().body} entry={props.entry} ctx={props.ctx} />
            </FoldHeader>
          )}
        </Match>
        <Match when={props.item.kind === "workflow" && props.item}>
          {(it) => <ToolGroup id={it().key} tools={[{ key: it().part.id, kind: "call", part: it().part }]} live={streaming()} ctx={group} />}
        </Match>
        <Match when={props.item.kind === "image" && props.item}>
          {(it) => <GeneratedImage part={it().part} device={props.ctx.device()} />}
        </Match>
        <Match when={props.item.kind === "visual" && props.item}>
          {(it) => <VisualPart chatId={props.ctx.chatId} part={it().part} />}
        </Match>
        <Match when={props.item.kind === "uiBlock" && props.item}>
          {(it) => <UiBlockNotice part={it().part} />}
        </Match>
        <Match when={props.item.kind === "input" && props.item}>
          {(it) => <InputChip part={it().part} />}
        </Match>
        <Match when={props.item.kind === "error" && props.item}>
          {(it) => <ErrorChip part={it().part} />}
        </Match>
        <Match when={props.item.kind === "fork" && props.item}>
          {(it) => <ForkMarker part={it().part} />}
        </Match>
      </Switch>
    </div>
  );
}

function TextItem(props: { part: Extract<MessagePart, { kind: "text" }>; live: boolean }) {
  // Tokens land in the store as they arrive; the DOM sees at most one update per frame.
  const text = frameThrottled(() => props.part.text, () => props.live);
  return <Markdown text={text()} live={props.live} />;
}

// ---------------------------------------------------------------------------
// Metadata strip: timestamp, worked time, copy and fork, revealed on hover or focus
// ---------------------------------------------------------------------------

function MetaStrip(props: { entry: SessionMessageEntry; copy: string; align: "start" | "end"; fork?: boolean; chatId?: string }) {
  const [copied, setCopied] = createSignal(false);
  let reset: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(reset));
  const copy = async () => {
    if (!(await copyText(props.copy))) return;
    setCopied(true);
    clearTimeout(reset);
    reset = setTimeout(() => setCopied(false), 1500);
  };
  const worked = () => (props.entry.durationMs ? formatElapsed(props.entry.durationMs / 1000) : null);
  const canFork = () => props.fork && props.chatId && state.chats[props.chatId] && !state.chats[props.chatId]!.archived;
  return (
    <div class="t-meta" data-align={props.align}>
      <span class="t-meta-time">
        <time dateTime={new Date(props.entry.createdAt).toISOString()}>{formatTimestamp(props.entry.createdAt)}</time>
        <Show when={worked()}>{(w) => <span title="Time worked">{w()}</span>}</Show>
      </span>
      <Show when={props.copy}>
        <button type="button" class="t-meta-btn" aria-label={copied() ? "Copied" : "Copy message"} title="Copy message" onClick={copy}>
          <Icon name={copied() ? "check" : "copy"} size={14} />
        </button>
      </Show>
      <Show when={canFork()}>
        <ForkMenu chatId={props.chatId!} />
      </Show>
    </div>
  );
}

/** The reply's Fork menu. Non-modal: a modal Kobalte menu locks the page's scroll, and its trigger's keyboard
 *  handler then loops forever looking for a scroll parent inside the transcript's own scroller. */
function ForkMenu(props: { chatId: string }) {
  return (
    <DropdownMenu placement="bottom-start" gutter={4} modal={false} preventScroll={false}>
      <DropdownMenu.Trigger class="t-meta-btn" aria-label="Fork chat" title="Fork chat">
        <Icon name="fork" size={14} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="menu">
          <For each={FORK_MENU}>
            {(option) => (
              <DropdownMenu.Item class="menu-item" onSelect={() => void forkChat(props.chatId, option.target)}>
                <Icon name={option.target === "thread" ? "fork-split" : "fork"} size={14} />
                {option.label}
              </DropdownMenu.Item>
            )}
          </For>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}
