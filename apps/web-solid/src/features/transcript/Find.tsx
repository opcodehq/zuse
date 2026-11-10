// Find in chat (desktop transcript/find.rs): "n of m" over the store's messages, since rows outside the
// virtualized window are not in the page. Highlights use the CSS highlight registry, so no row DOM changes.
import { createEffect, createMemo, createSignal, on, onCleanup, onMount, Show } from "solid-js";
import type { MessagePart, SessionMessageEntry } from "@/core/protocol";
import { registerShortcut } from "@/core/keys";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { parseAttachments, userRawText } from "./model";

/** Desktop chat_find QUERY_MAX_CHARS. */
const QUERY_MAX = 256;

const [requests, setRequests] = createSignal(0);
registerShortcut({
  keys: "mod+f",
  label: "Find in chat",
  inInputs: true,
  // Editors and the terminal keep their own find; elsewhere there is no transcript to search.
  when: (event) => view.route.kind === "chat" && !!state.transcript?.chatId && !(event.target as HTMLElement | null)?.closest(".cm-editor, .xterm"),
  run: () => setRequests((n) => n + 1),
});

interface Match {
  index: number;
  id: string;
  ordinal: number;
}

export const normalizeQuery = (query: string) => query.trim().slice(0, QUERY_MAX).toLowerCase();

function count(text: string, needle: string) {
  let n = 0;
  for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + needle.length)) n++;
  return n;
}

/** Markdown source reduced to roughly what the renderer paints, so counts track the visible text. */
function plain(markdown: string) {
  return markdown
    .replace(/^ {0,3}(`{3,}|~{3,}).*$/gm, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^ {0,3}(#{1,6}\s+|>\s?|[-*+]\s+(\[[ xX]\]\s+)?|\d+[.)]\s+)/gm, "")
    .replace(/\*\*|__|~~|[*`]/g, "");
}

type TextPart = Extract<MessagePart, { kind: "text" }>;

export interface FindHost {
  messages: () => readonly SessionMessageEntry[];
  /** First row in view: a new query starts there. */
  topIndex: () => number;
  /** Bring a row into the window. */
  reveal: (index: number) => void;
  scroller: () => HTMLElement;
  inner: () => HTMLElement;
  /** Keys of mounted rows; highlights repaint when the window moves. */
  mounted: () => readonly string[];
}

export function FindBar(props: { host: FindHost }) {
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal<{ id: string; ordinal: number; index: number } | null>(null);
  let input: HTMLInputElement | undefined;
  let returnFocus: HTMLElement | null = null;

  const needle = createMemo(() => normalizeQuery(query()));
  // Per-part counts keyed by the part's text object, so a streamed token rescans only its own part.
  let counts = new Map<string, { text: string; needle: string; count: number }>();
  const partCount = (key: string, text: string, prepared: () => string, q: string) => {
    const hit = counts.get(key);
    if (hit && hit.text === text && hit.needle === q) return hit.count;
    const n = count(prepared().toLowerCase(), q);
    counts.set(key, { text, needle: q, count: n });
    return n;
  };
  const matches = createMemo<Match[]>(() => {
    const q = needle();
    if (!open() || !q) return [];
    const out: Match[] = [];
    const next = new Map<string, { text: string; needle: string; count: number }>();
    const list = props.host.messages();
    for (let index = 0; index < list.length; index++) {
      const entry = list[index]!;
      let n = 0;
      if (entry.role === "user") {
        const raw = userRawText(entry);
        n = partCount(entry.id, raw, () => parseAttachments(raw).text, q);
        next.set(entry.id, counts.get(entry.id)!);
      } else {
        for (const part of entry.parts) {
          if (part.kind !== "text") continue;
          const key = `${entry.id}#${part.id}`;
          n += partCount(key, (part as TextPart).text, () => plain((part as TextPart).text), q);
          next.set(key, counts.get(key)!);
        }
      }
      for (let ordinal = 0; ordinal < n; ordinal++) out.push({ index, id: entry.id, ordinal });
    }
    counts = next;
    return out;
  });

  const activeIndex = createMemo(() => {
    const current = active();
    if (!current) return -1;
    const list = matches();
    const exact = list.findIndex((m) => m.id === current.id && m.ordinal === current.ordinal);
    if (exact >= 0) return exact;
    // The active match's row changed under it: stay at the same position in the list.
    return list.length ? Math.min(current.index, list.length - 1) : -1;
  });

  const select = (at: number) => {
    const match = matches()[at];
    setActive(match ? { id: match.id, ordinal: match.ordinal, index: at } : null);
    if (match) reveal(match);
  };
  const step = (direction: 1 | -1) => {
    const total = matches().length;
    if (!total) return;
    const current = activeIndex();
    const start = current < 0 && direction < 0 ? 0 : current;
    select((((start + direction) % total) + total) % total);
  };

  // A new query lands on the first match at or below what the reader is looking at.
  createEffect(
    on(needle, () => {
      const list = matches();
      if (!list.length) return setActive(null);
      const from = props.host.topIndex();
      const at = list.findIndex((m) => m.index >= from);
      select(at < 0 ? 0 : at);
    }, { defer: true }),
  );

  const openBar = () => {
    if (!open()) {
      const focused = document.activeElement;
      returnFocus = focused instanceof HTMLElement && focused !== document.body ? focused : null;
      setOpen(true);
    }
    queueMicrotask(() => {
      input?.focus();
      input?.select();
    });
  };
  createEffect(on(requests, openBar, { defer: true }));

  const close = () => {
    clear();
    setOpen(false);
    // Desktop drops the bar with its query; the next Mod+F starts empty.
    setQuery("");
    setActive(null);
    counts = new Map();
    const back = returnFocus?.isConnected ? returnFocus : props.host.scroller();
    returnFocus = null;
    back.focus({ preventScroll: true });
  };

  // --- Highlights -----------------------------------------------------------
  const supported = typeof CSS !== "undefined" && "highlights" in CSS;
  const clear = () => {
    if (!supported) return;
    CSS.highlights.delete("t-find");
    CSS.highlights.delete("t-find-active");
  };
  /** Ranges for every occurrence of `q` inside `root`, matched across the text nodes of each block. */
  const rangesIn = (root: Element, q: string) => {
    const out: Range[] = [];
    const blocks = root.matches(".md") ? Array.from(root.children) : [root];
    for (const block of blocks) {
      const nodes: Text[] = [];
      const starts: number[] = [];
      let text = "";
      const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT, {
        acceptNode: (node) => (node.parentElement?.closest(".code-head, button") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
      });
      for (let node = walker.nextNode() as Text | null; node; node = walker.nextNode() as Text | null) {
        nodes.push(node);
        starts.push(text.length);
        text += node.data;
      }
      const lower = text.toLowerCase();
      const locate = (offset: number): [Text, number] => {
        let i = nodes.length - 1;
        while (i > 0 && starts[i]! > offset) i--;
        return [nodes[i]!, offset - starts[i]!];
      };
      for (let at = lower.indexOf(q); at >= 0; at = lower.indexOf(q, at + q.length)) {
        const range = document.createRange();
        const [startNode, startOffset] = locate(at);
        const [endNode, endOffset] = locate(at + q.length - 1);
        range.setStart(startNode, startOffset);
        range.setEnd(endNode, endOffset + 1);
        out.push(range);
      }
    }
    return out;
  };

  let frame = 0;
  let follow = false;
  const paint = () => {
    frame = 0;
    if (!supported) return;
    const q = needle();
    if (!open() || !q) return clear();
    const all: Range[] = [];
    let current: Range | undefined;
    const hit = matches()[activeIndex()];
    for (const row of props.host.inner().querySelectorAll<HTMLElement>(".t-row[data-entry]")) {
      const roots = row.querySelectorAll(".t-bubble-text, .t-item[data-kind='text'] .md");
      const ranges = Array.from(roots).flatMap((root) => rangesIn(root, q));
      if (hit && row.dataset.entry === hit.id) current = ranges[Math.min(hit.ordinal, ranges.length - 1)];
      all.push(...ranges);
    }
    CSS.highlights.set("t-find", new Highlight(...all.filter((r) => r !== current)));
    if (current) CSS.highlights.set("t-find-active", new Highlight(current));
    else CSS.highlights.delete("t-find-active");
    if (follow && current) {
      follow = false;
      const box = current.getBoundingClientRect();
      const view = props.host.scroller().getBoundingClientRect();
      if (box.top < view.top + 24 || box.bottom > view.bottom - 24) props.host.scroller().scrollTop += box.top - view.top - view.height / 2;
    }
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(paint);
  };
  const reveal = (match: Match) => {
    follow = true;
    props.host.reveal(match.index);
    // The row mounts over the next frames; paint once it is there.
    requestAnimationFrame(() => requestAnimationFrame(schedule));
  };

  createEffect(on([open, needle, matches, activeIndex, () => props.host.mounted()], schedule));
  onMount(() => {
    const observer = new MutationObserver(() => open() && needle() && schedule());
    observer.observe(props.host.inner(), { childList: true, characterData: true, subtree: true });
    onCleanup(() => observer.disconnect());
  });
  onCleanup(() => {
    cancelAnimationFrame(frame);
    clear();
  });

  const total = () => matches().length;
  const label = () => (!needle() ? "" : total() ? `${activeIndex() + 1} of ${total()}` : "No results");

  return (
    <Show when={open()}>
      <div
        class="t-find"
        role="search"
        aria-label="Find in chat"
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            close();
          }
        }}
      >
        <div class="t-find-field">
          <input
            ref={input}
            class="t-find-input"
            type="text"
            placeholder="Find in chat"
            aria-label="Find in chat"
            spellcheck={false}
            autocomplete="off"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key !== "Enter" || e.isComposing) return;
              e.preventDefault();
              step(e.shiftKey ? -1 : 1);
            }}
          />
        </div>
        <span class="t-find-count" data-empty={needle() && !total() ? "true" : undefined} aria-live="polite">{label()}</span>
        <button type="button" class="t-find-button" aria-label="Previous match" title="Previous match" disabled={!total()} onClick={() => step(-1)}>
          <Icon name="alt-arrow-up" size={16} />
        </button>
        <button type="button" class="t-find-button" aria-label="Next match" title="Next match" disabled={!total()} onClick={() => step(1)}>
          <Icon name="alt-arrow-down" size={16} />
        </button>
        <button type="button" class="t-find-button" aria-label="Close find" title="Close find" onClick={close}>
          <Icon name="close" size={16} />
        </button>
      </div>
    </Show>
  );
}
