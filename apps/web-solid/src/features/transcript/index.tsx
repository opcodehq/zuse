// The chat transcript: conversation tabs, a virtualized message list that sticks to the bottom while it streams,
// the outline rail, "Jump to latest", and the docked answer surface for the agent's open request.
import { createVirtualizer } from "@tanstack/solid-virtual";
import { batch, createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, untrack } from "solid-js";
import { unwrap } from "solid-js/store";
import type { SessionMessageEntry } from "@/core/protocol";
import { registerShortcut } from "@/core/keys";
import { state } from "@/core/store";
import { openChat, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { DitherRing } from "@/ui/loaders";
import { conversationTabs } from "./actions";
import { Entry, type EntryContext } from "./Entry";
import { FindBar, type FindHost } from "./Find";
import { InputDock } from "./Input";
import { Rail, RAIL_MIN_WIDTH, railTicks } from "./Rail";
import { ConversationTabs } from "./Tabs";
import { TurnStatus, WorkingTrailer } from "./parts";
import "./transcript.css";

/** Re-engage the bottom pin within this distance of the end (desktop STICK_THRESHOLD_PX). */
const STICK_PX = 70;
const JUMP_SHOW_PX = 320;
const AT_BOTTOM_PX = 2;
/** A scroll counts as the user's when it follows their input this closely. */
const INTENT_MS = 400;
const PAD_TOP = 16;
const PAD_BOTTOM = 32;
/** Desktop GLIDE_MAX_VIEWPORTS: farther targets jump instead of gliding. */
const GLIDE_VIEWPORTS = 2.5;

interface Viewport {
  pinned: boolean;
  anchor: string | null;
  offset: number;
}
/** Per-chat reading positions, so returning to a chat lands where it was left. */
const viewports = new Map<string, Viewport>();
const MAX_VIEWPORTS = 256;

// Desktop SelectTab1..8 and SelectLastTab (crates/protocol/src/keybindings.rs); where the browser keeps
// these keys for its own tabs, the strip stays reachable with the keyboard.
for (let n = 1; n <= 9; n++)
  registerShortcut({
    keys: `mod+${n}`,
    label: n === 9 ? "Go to last tab" : `Go to tab ${n}`,
    inInputs: true,
    run: () => {
      const open = view.route.kind === "chat" ? conversationTabs()?.open : undefined;
      const chat = open && (n === 9 ? open[open.length - 1] : open[n - 1]);
      if (chat) openChat(chat.id);
    },
  });

export function Transcript() {
  return (
    <div class="t-root">
      <ConversationTabs />
      <Show when={state.transcript?.chatId} keyed>
        {(chatId) => <ChatTranscript chatId={chatId} />}
      </Show>
    </div>
  );
}

function estimate(entry: SessionMessageEntry | undefined) {
  if (!entry) return 120;
  let chars = 0;
  let blocks = 0;
  for (const part of entry.parts) {
    if (part.kind === "text") chars += part.text.length;
    else blocks++;
  }
  const lines = Math.ceil(chars / 90);
  return entry.role === "user" ? 60 + Math.min(lines, 5) * 22 : 56 + lines * 22 + Math.min(blocks, 1) * 30;
}

function ChatTranscript(props: { chatId: string }) {
  let scroller!: HTMLDivElement;
  let inner!: HTMLDivElement;
  const messages = () => state.transcript?.messages ?? [];
  // Keyed by chat above, so the chat never changes under this component.
  const saved = untrack(() => viewports.get(props.chatId));
  const [pinned, setPinned] = createSignal(saved?.pinned ?? true);
  const [jump, setJump] = createSignal(false);
  const [width, setWidth] = createSignal(0);
  const [height, setHeight] = createSignal(0);
  const [topIndex, setTopIndex] = createSignal(0);
  let intentAt = 0;
  // Estimates only seed unmeasured rows; computing one per entry once keeps a new message O(1) here.
  const estimates = new Map<string, number>();
  let restored = !saved || saved.pinned;

  // The virtualizer walks every row on each count change; it reads plain rows, not store proxies.
  let rows: readonly SessionMessageEntry[] = [];
  const virtualizer = createVirtualizer({
    get count() {
      rows = unwrap(messages());
      return messages().length;
    },
    getScrollElement: () => scroller,
    estimateSize: (index) => {
      const entry = rows[index];
      if (!entry) return 120;
      let size = estimates.get(entry.id);
      if (size === undefined) estimates.set(entry.id, (size = estimate(entry)));
      return size;
    },
    getItemKey: (index) => rows[index]?.id ?? index,
    overscan: 4,
    paddingStart: PAD_TOP,
    paddingEnd: PAD_BOTTOM,
  });

  const distance = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
  const toBottom = () => {
    scroller.scrollTop = scroller.scrollHeight;
  };

  const readPosition = () => {
    const d = distance();
    const item = virtualizer.getVirtualItemForOffset(scroller.scrollTop + PAD_TOP);
    if (item) setTopIndex(item.index);
    setJump((shown) => d > (shown ? AT_BOTTOM_PX : JUMP_SHOW_PX) && !pinned());
    return d;
  };

  const onScroll = () => {
    const d = distance();
    // Layout and our own writes never unpin; only the user's own scroll away does.
    if (d <= STICK_PX) setPinned(true);
    else if (performance.now() - intentAt < INTENT_MS) setPinned(false);
    readPosition();
  };
  const noteIntent = () => (intentAt = performance.now());
  const onKey = (e: KeyboardEvent) => {
    if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) noteIntent();
  };

  // Growth while pinned follows the end in the same frame it lays out.
  const stick = () => {
    if (pinned() && restored) toBottom();
    readPosition();
  };

  onMount(() => {
    const resize = new ResizeObserver(() => {
      batch(() => {
        setWidth(scroller.clientWidth);
        setHeight(scroller.clientHeight);
      });
      stick();
    });
    resize.observe(scroller);
    resize.observe(inner);
    onCleanup(() => resize.disconnect());
  });

  createEffect(on(() => virtualizer.getTotalSize(), () => queueMicrotask(stick)));

  // Restore a saved reading position once the history has arrived.
  createEffect(() => {
    if (restored || !state.transcript?.ready || !saved?.anchor) return;
    const index = untrack(messages).findIndex((m) => m.id === saved.anchor);
    restored = true;
    if (index < 0) return toBottom();
    virtualizer.scrollToIndex(index, { align: "start" });
    // Rows above are only estimated until they render, so settle on the anchor row's real position over a few frames.
    let frames = 0;
    const settle = () => {
      const row = inner.querySelector<HTMLElement>(`[data-entry="${CSS.escape(saved.anchor!)}"]`);
      if (row) {
        const drift = row.getBoundingClientRect().top - scroller.getBoundingClientRect().top + saved.offset;
        if (Math.abs(drift) >= 1) scroller.scrollTop += drift;
        else if (frames > 1) return;
      }
      if (++frames < 8) requestAnimationFrame(settle);
    };
    requestAnimationFrame(settle);
  });
  createEffect(() => {
    if (state.transcript?.ready && !saved) restored = true;
  });

  onCleanup(() => {
    const item = virtualizer.getVirtualItemForOffset(scroller.scrollTop);
    viewports.delete(props.chatId);
    viewports.set(props.chatId, {
      pinned: pinned(),
      anchor: item ? String(item.key) : null,
      offset: item ? scroller.scrollTop - item.start : 0,
    });
    if (viewports.size > MAX_VIEWPORTS) viewports.delete(viewports.keys().next().value!);
  });

  const jumpToLatest = () => {
    // The button disappears once at the end; keep keyboard focus in the conversation instead of losing it.
    if (document.activeElement?.closest(".t-jump")) scroller.focus({ preventScroll: true });
    setPinned(true);
    setJump(false);
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) return toBottom();
    // Glide at most a couple of viewports; anything farther jumps close first.
    const glide = GLIDE_VIEWPORTS * scroller.clientHeight;
    if (distance() > glide) scroller.scrollTop = scroller.scrollHeight - scroller.clientHeight - glide;
    scroller.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" });
  };
  // A glide aims at the end measured when it began; rows measured on the way can leave it short.
  const onScrollEnd = () => {
    if (pinned() && distance() > AT_BOTTOM_PX) toBottom();
  };
  const scrollToEntry = (index: number) => {
    // Picked from the rail's list, which closes and takes focus with it: continue reading from the conversation.
    if (!document.activeElement || document.activeElement === document.body) scroller.focus({ preventScroll: true });
    noteIntent();
    setPinned(false);
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    // Rows far away are only estimated, so a long glide would aim at a moving target; jump there instead.
    const target = virtualizer.getOffsetForIndex(index, "start")?.[0] ?? 0;
    const near = Math.abs(target - scroller.scrollTop) < GLIDE_VIEWPORTS * scroller.clientHeight;
    virtualizer.scrollToIndex(index, { align: "start", behavior: near && !reduce ? "smooth" : "auto" });
  };

  const findHost: FindHost = {
    messages,
    topIndex,
    reveal: (index) => {
      noteIntent();
      setPinned(false);
      virtualizer.scrollToIndex(index, { align: "center" });
    },
    scroller: () => scroller,
    inner: () => inner,
    mounted: () => keys(),
  };

  const items = () => virtualizer.getVirtualItems();
  const keys = createMemo(() => items().map((item) => String(item.key)), [], {
    equals: (a, b) => a.length === b.length && a.every((v, i) => v === b[i]),
  });
  const byKey = createMemo(() => new Map(items().map((item) => [String(item.key), item])));
  const offsetTop = () => items()[0]?.start ?? 0;

  const chat = () => state.chats[props.chatId];
  const ctxFor = (index: () => number): EntryContext => ({
    chatId: props.chatId,
    device: () => chat()?.deviceId ?? null,
    harness: () => chat()?.config?.harness,
    last: () => index() === messages().length - 1,
  });

  // Prompts only arrive or leave with the list's length, so the outline rebuilds from plain rows then.
  const ticks = createMemo(() => {
    const list = messages();
    void list.length;
    void list[list.length - 1]?.id;
    return railTicks(unwrap(list));
  });
  const empty = () => state.transcript?.ready && messages().length === 0;
  const width_ = () => {
    const pref = Number(view.preferences.transcriptWidth);
    return Number.isFinite(pref) && pref >= 560 && pref <= 1200 ? pref : 736;
  };

  return (
    <div class="t-chat">
      <div class="t-viewport">
        <div
          ref={scroller}
          class="t-scroller"
          tabIndex={0}
          role="region"
          aria-label="Conversation"
          aria-busy={!state.transcript?.ready}
          style={{ "--t-column": `${width_()}px` }}
          data-narrow={width() > 0 && width() < RAIL_MIN_WIDTH ? "true" : undefined}
          onScroll={onScroll}
        on:scrollend={onScrollEnd}
          onWheel={noteIntent}
          onTouchMove={noteIntent}
          onPointerDown={noteIntent}
          onKeyDown={onKey}
        >
          <div ref={inner} class="t-list" style={{ height: `${virtualizer.getTotalSize()}px` }}>
            <div class="t-window" style={{ transform: `translateY(${offsetTop()}px)` }}>
              <For each={keys()}>
                {(key) => {
                  const item = () => byKey().get(key);
                  const index = () => item()?.index ?? -1;
                  const entry = () => messages()[index()];
                  let row!: HTMLDivElement;
                  onMount(() => virtualizer.measureElement(row));
                  return (
                    <div ref={row} class="t-row" data-index={index()} data-entry={key}>
                      <div class="t-column">
                        <Show when={entry()}>{(e) => <Entry entry={e()} ctx={ctxFor(index)} />}</Show>
                        <Show when={index() === messages().length - 1}>
                          <WorkingTrailer chatId={props.chatId} />
                        </Show>
                      </div>
                    </div>
                  );
                }}
              </For>
            </div>
          </div>
          <Show when={empty()}>
            <div class="t-empty">
              <WorkingTrailer chatId={props.chatId} />
            </div>
          </Show>
          <Show when={!state.transcript?.ready && messages().length === 0}>
            <div class="t-loading"><DitherRing size={12} tone="faint" label="Loading conversation" /></div>
          </Show>
        </div>
        <TurnStatus chatId={props.chatId} />
        <FindBar host={findHost} />
        <Show when={state.transcript?.notice}>
          {(notice) => <div class="t-notice-bar" role="status">{notice()}</div>}
        </Show>
        <Show when={width() >= RAIL_MIN_WIDTH && ticks().length >= 2}>
          <Rail ticks={ticks()} topIndex={topIndex()} height={height()} onPick={scrollToEntry} />
        </Show>
        <Show when={jump()}>
          <div class="t-jump-wrap">
            <button type="button" class="t-jump" aria-label="Jump to latest" onClick={jumpToLatest}>
              <Show when={state.sessions[props.chatId]?.status === "working"} fallback={<Icon name="alt-arrow-down" size={12} />}>
                <span class="t-jump-pulse" aria-hidden="true" />
              </Show>
              Jump to latest
            </button>
          </div>
        </Show>
      </div>
      <InputDock chatId={props.chatId} />
    </div>
  );
}
