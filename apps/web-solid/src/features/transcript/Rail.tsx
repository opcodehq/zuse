// The transcript outline (desktop `rail.rs`): a fixed stack of prompt marks at the top right; hovering or
// clicking opens every prompt as a list, and picking one scrolls the transcript there.
import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import type { SessionMessageEntry } from "@/core/protocol";
import { parseAttachments, singleLine, userRawText } from "./model";

const MAX_TICKS = 12;
const TICK_SLOT = 10;
const TICK_GAP = 3;
const V_MARGIN = 24;
const PREVIEW_CHARS = 160;
const CLOSE_DELAY = 140;
const ROW = 32;
const LIST_MAX = 320;
export const RAIL_MIN_WIDTH = 768;

export interface Tick {
  id: string;
  index: number;
  prompt: string;
}

// Sent prompts never change, so each is parsed once however often the list is rebuilt.
const prompts = new Map<string, string>();

export function railTicks(messages: readonly SessionMessageEntry[]): Tick[] {
  const ticks: Tick[] = [];
  messages.forEach((entry, index) => {
    if (entry.role !== "user") return;
    let prompt = prompts.get(entry.id);
    if (prompt === undefined) {
      const parsed = parseAttachments(userRawText(entry));
      prompt = parsed.text || (parsed.attachments.length ? "Attached image(s)" : "");
      prompts.set(entry.id, prompt);
    }
    ticks.push({ id: entry.id, index, prompt });
  });
  return ticks;
}

/** `tick_buckets`: past the slot count, marks become even buckets over the conversation. */
export function tickBuckets(n: number, capacity: number): [number, number][] {
  if (!n) return [];
  const cap = Math.min(Math.max(capacity, 1), n);
  return Array.from({ length: cap }, (_, k) => [Math.floor((k * n) / cap), Math.floor(((k + 1) * n) / cap)]);
}

const slots = (height: number) => Math.min(MAX_TICKS, Math.max(1, Math.floor((Math.max(height - 2 * V_MARGIN, TICK_SLOT) + TICK_GAP) / (TICK_SLOT + TICK_GAP))));

function preview(text: string) {
  const flat = singleLine(text);
  return flat.length <= PREVIEW_CHARS ? flat : `${flat.slice(0, PREVIEW_CHARS - 1).trimEnd()}…`;
}

export function Rail(props: { ticks: Tick[]; topIndex: number; height: number; onPick: (index: number) => void }) {
  const [open, setOpen] = createSignal(false);
  const [highlight, setHighlight] = createSignal<number | null>(null);
  const [listTop, setListTop] = createSignal(0);
  // Fixed-height rows, so only the visible window of a long conversation's prompts is in the DOM.
  const windowed = createMemo(() => {
    const first = Math.max(0, Math.floor(listTop() / ROW) - 4);
    const last = Math.min(props.ticks.length, first + Math.ceil(LIST_MAX / ROW) + 8);
    return Array.from({ length: Math.max(0, last - first) }, (_, i) => first + i);
  });
  let list: HTMLDivElement | undefined;
  let trigger!: HTMLButtonElement;
  let close: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(close));
  const active = createMemo(() => {
    let found = 0;
    props.ticks.forEach((tick, ix) => {
      if (tick.index <= props.topIndex) found = ix;
    });
    return props.ticks.length ? found : null;
  });
  const buckets = createMemo(() => tickBuckets(props.ticks.length, slots(props.height || 600)));
  const activeBucket = () => {
    const a = active();
    return a === null ? -1 : buckets().findIndex(([s, e]) => a >= s && a < e);
  };
  const show = () => {
    clearTimeout(close);
    if (!open()) {
      setHighlight(active());
      setOpen(true);
      // The list opens on the prompt being read, not at the first one.
      queueMicrotask(() => {
        const a = active();
        if (list && a !== null) list.scrollTop = Math.max(0, a * ROW - list.clientHeight / 2 + ROW / 2);
        if (list) setListTop(list.scrollTop);
      });
    }
  };
  const hide = () => {
    clearTimeout(close);
    setOpen(false);
    setHighlight(null);
  };
  const scheduleHide = () => {
    clearTimeout(close);
    close = setTimeout(hide, CLOSE_DELAY);
  };
  const pick = (ix: number) => {
    const tick = props.ticks[ix];
    hide();
    if (tick) props.onPick(tick.index);
  };
  const reveal = (ix: number) => {
    if (!list) return;
    const top = ix * ROW;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ROW > list.scrollTop + list.clientHeight) list.scrollTop = top + ROW - list.clientHeight + 8;
  };
  const move = (delta: number) => {
    const n = props.ticks.length;
    const next = ((highlight() ?? (delta > 0 ? -1 : n)) + delta + n) % n;
    setHighlight(next);
    reveal(next);
  };
  const onKey = (event: KeyboardEvent) => {
    switch (event.key) {
      case "ArrowDown": move(1); break;
      case "ArrowUp": move(-1); break;
      case "Enter": { const h = highlight(); if (h !== null) pick(h); break; }
      case "Escape": hide(); trigger.focus(); break;
      default: return;
    }
    event.preventDefault();
    event.stopPropagation();
  };
  return (
    <div class="t-rail" onMouseEnter={show} onMouseLeave={scheduleHide}>
      <button
        ref={trigger}
        type="button"
        class="t-rail-trigger"
        aria-label="Navigate conversation"
        aria-expanded={open()}
        aria-haspopup="listbox"
        data-open={open() ? "true" : undefined}
        onClick={() => {
          show();
          queueMicrotask(() => list?.focus());
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            show();
            queueMicrotask(() => list?.focus());
          }
        }}
      >
        <For each={buckets()}>{(_, ix) => <span class="t-rail-mark" data-active={ix() === activeBucket() ? "true" : undefined} />}</For>
      </button>
      <Show when={open()}>
        <div
          ref={list}
          class="t-rail-list"
          role="listbox"
          aria-label="Conversation turns"
          tabIndex={0}
          aria-activedescendant={highlight() !== null ? `rail-turn-${highlight()}` : undefined}
          onKeyDown={onKey}
          onScroll={(e) => setListTop(e.currentTarget.scrollTop)}
          onFocusOut={(e) => {
            if (!e.currentTarget.parentElement?.contains(e.relatedTarget as Node | null)) scheduleHide();
          }}
        >
          <div style={{ height: `${props.ticks.length * ROW}px`, position: "relative" }}>
            <For each={windowed()}>
              {(ix) => {
                const tick = () => props.ticks[ix];
                return (
                  <div
                    id={`rail-turn-${ix}`}
                    class="t-rail-row"
                    role="option"
                    aria-selected={active() === ix}
                    aria-posinset={ix + 1}
                    aria-setsize={props.ticks.length}
                    data-highlight={highlight() === ix ? "true" : undefined}
                    style={{ position: "absolute", top: `${ix * ROW}px`, left: 0, right: 0 }}
                    onMouseMove={() => highlight() !== ix && setHighlight(ix)}
                    onClick={() => pick(ix)}
                  >
                    <span class="truncate">{preview(tick()?.prompt ?? "")}</span>
                    <Show when={active() === ix}><span class="t-rail-dot" aria-hidden="true" /></Show>
                  </div>
                );
              }}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}
