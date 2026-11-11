// Code colouring. Shiki tokenizes in a worker (highlight.worker.ts); this side only turns its compact tokens into
// spans, and for long blocks only the lines near the viewport, so a large file never blocks or bloats the page.
import type { TokenizeReply, TokenizeRequest } from "./highlight.worker";

const MAX_CHARS = 200_000;
// Beyond this a block stays plain text: colouring more is little reading value for the memory it holds.
const MAX_LINES = 1_500;
const CACHE_LIMIT = 128;

/** Tokens for one block: `data` holds [offset, length, colour index (-1 for none), font style] per token. */
export interface CodeTokens {
  colors: string[];
  data: Int32Array;
}

const cache = new Map<string, Promise<CodeTokens | null>>();
let worker: Worker | null = null;
let broken = false;
let nextId = 0;
const waiting = new Map<number, (result: CodeTokens | null) => void>();

function tokenizer(): Worker | null {
  if (broken) return null;
  if (!worker) {
    try {
      worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    } catch {
      broken = true;
      return null;
    }
    worker.onmessage = (event: MessageEvent<TokenizeReply>) => {
      waiting.get(event.data.id)?.(event.data.result);
      waiting.delete(event.data.id);
    };
    // A worker that cannot load leaves code plain rather than retrying forever.
    worker.onerror = () => {
      broken = true;
      for (const resolve of waiting.values()) resolve(null);
      waiting.clear();
    };
  }
  return worker;
}

/** Tokens for `code`, or null for unknown languages and oversized blocks. */
export function highlight(code: string, lang: string | undefined): Promise<CodeTokens | null> {
  if (!lang || code.length > MAX_CHARS || code.split("\n", MAX_LINES + 1).length > MAX_LINES) return Promise.resolve(null);
  const key = `${lang}\u0000${code}`;
  let result = cache.get(key);
  if (!result) {
    const target = tokenizer();
    if (!target) return Promise.resolve(null);
    result = new Promise<CodeTokens | null>((resolve) => {
      const id = ++nextId;
      waiting.set(id, resolve);
      target.postMessage({ id, code, lang } satisfies TokenizeRequest);
    });
    cache.set(key, result);
    if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
  }
  return result;
}

/** Blocks up to this many lines are coloured whole; longer ones colour only what is near the viewport. */
export const SPAN_LINES = 200;
const WINDOW_LINES = 50;
const SAFE_COLOR = /^[#\w\s(),.%-]+$/;

/** First token whose offset is at or after `offset`. */
function firstToken(tokens: CodeTokens, offset: number) {
  let lo = 0;
  let hi = tokens.data.length / 4;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (tokens.data[mid * 4]! < offset) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Coloured DOM for `code` between two offsets; token text never passes through HTML parsing. */
function colour(code: string, tokens: CodeTokens, from: number, to: number): DocumentFragment {
  const out = document.createDocumentFragment();
  const { data, colors } = tokens;
  let at = from;
  for (let i = firstToken(tokens, from) * 4; i < data.length && data[i]! < to; i += 4) {
    const start = data[i]!;
    const end = Math.min(to, start + data[i + 1]!);
    if (start < at || end <= start) continue;
    if (start > at) out.append(code.slice(at, start));
    const span = document.createElement("span");
    span.textContent = code.slice(start, end);
    const color = colors[data[i + 2]!];
    if (color && SAFE_COLOR.test(color)) span.style.color = color;
    const style = data[i + 3]!;
    if (style & 1) span.style.fontStyle = "italic";
    if (style & 2) span.style.fontWeight = "600";
    out.append(span);
    at = end;
  }
  if (at < to) out.append(code.slice(at, to));
  return out;
}

/** Colour a whole block in place. */
export function renderTokens(pre: HTMLElement, code: string, tokens: CodeTokens) {
  pre.replaceChildren(colour(code, tokens, 0, code.length));
}

/** Long blocks: plain text in runs of lines, each run coloured only while it is near the viewport. The DOM stays
 *  a few thousand nodes however long the file, and remounting a scrolled-back block costs one run's worth. */
export function paintWindowed(pre: HTMLElement, code: string, tokens: CodeTokens): () => void {
  const runs: { el: HTMLElement; from: number; to: number; coloured: boolean; near: boolean }[] = [];
  const fragment = document.createDocumentFragment();
  for (let start = 0; start < code.length; ) {
    let end = start;
    let lines = 0;
    for (; lines < WINDOW_LINES && end < code.length; lines++) {
      const next = code.indexOf("\n", end);
      end = next < 0 ? code.length : next + 1;
    }
    const el = document.createElement("span");
    el.className = "code-run";
    el.textContent = code.slice(start, end);
    // Off-screen runs skip layout (content-visibility); their exact unwrapped height keeps the row's size stable.
    el.style.setProperty("--lines", String(lines));
    runs.push({ el, from: start, to: end, coloured: false, near: false });
    fragment.append(el);
    start = end;
  }
  pre.replaceChildren(fragment);
  let alive = true;
  let scheduled = false;
  // Colour one run per frame, nearest first, so scrolling never waits on it.
  const drain = () => {
    scheduled = false;
    const next = runs.find((run) => run.near && !run.coloured);
    if (!alive || !next) return;
    next.el.replaceChildren(colour(code, tokens, next.from, next.to));
    next.coloured = true;
    schedule();
  };
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(drain);
  };
  const byEl = new Map(runs.map((run) => [run.el as Element, run]));
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const run = byEl.get(entry.target);
        if (!run) continue;
        run.near = entry.isIntersecting;
        // Text selected inside a run survives as long as the run keeps its nodes.
        if (!run.near && run.coloured && !document.getSelection()?.containsNode(run.el, true)) {
          run.el.textContent = code.slice(run.from, run.to);
          run.coloured = false;
        }
      }
      schedule();
    },
    { rootMargin: "800px 0px" },
  );
  for (const run of runs) observer.observe(run.el);
  return () => {
    alive = false;
    observer.disconnect();
  };
}
