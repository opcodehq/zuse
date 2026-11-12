// `:shortcode` emoji completion over desktop's gemoji table (crates/ui/src/composer/editing.rs).
import { createSignal } from "solid-js";

export interface EmojiRow { emoji: string; name: string }
export interface EmojiTrigger { start: number; end: number; query: string }

const LIMIT = 8;
const MAX_NAME = 40;
const MIN_QUERY = 2;
const nameChar = (ch: string) => /[A-Za-z0-9_+-]/.test(ch);

// The table loads on the first `:`; until then nothing matches.
const [entries, setEntries] = createSignal<EmojiRow[] | null>(null);
let loading = false;
function ensureLoaded() {
  if (entries() || loading) return;
  loading = true;
  void import("../../../../../crates/ui/src/composer/emoji.tsv?raw").then((module: { default: string }) => {
    const rows: EmojiRow[] = [];
    for (const line of module.default.split("\n")) {
      const [emoji, aliases] = line.split("\t");
      if (emoji && aliases) for (const name of aliases.split(" ")) if (name) rows.push({ emoji, name });
    }
    setEntries(rows);
  });
}

/** Inside an inline or fenced code span the colon is literal. */
function inCode(text: string, at: number) {
  const before = text.slice(0, at);
  const fences = before.match(/^```/gm)?.length ?? 0;
  if (fences % 2 === 1) return true;
  const line = before.slice(before.lastIndexOf("\n") + 1);
  return (line.match(/`/g)?.length ?? 0) % 2 === 1;
}

/** The `:name` run ending at `caret`, when it starts a word. */
function scan(text: string, caret: number): { colon: number; name: string } | null {
  let start = caret;
  while (start > 0 && caret - start <= MAX_NAME && nameChar(text[start - 1]!)) start--;
  if (caret - start > MAX_NAME) return null;
  const colon = start - 1;
  if (colon < 0 || text[colon] !== ":") return null;
  const opener = text[colon - 1];
  if (opener !== undefined && !/\s/.test(opener) && !"([{".includes(opener)) return null;
  if (inCode(text, colon)) return null;
  return { colon, name: text.slice(start, caret) };
}

export function emojiTrigger(text: string, caret: number): EmojiTrigger | null {
  const found = scan(text, caret);
  if (!found || found.name.length < MIN_QUERY) return null;
  ensureLoaded();
  return { start: found.colon, end: caret, query: found.name };
}

/** Prefix matches, shortest first, then names that contain the query; one row per emoji. */
export function emojiSuggestions(query: string): EmojiRow[] {
  const all = entries();
  const needle = query.toLowerCase();
  if (!all || !needle) return [];
  const prefixed = all.filter((e) => e.name.startsWith(needle)).sort((a, b) => a.name.length - b.name.length || a.name.localeCompare(b.name));
  const contained = all.filter((e) => !e.name.startsWith(needle) && e.name.includes(needle));
  const out: EmojiRow[] = [];
  for (const entry of [...prefixed, ...contained]) {
    if (out.length >= LIMIT) break;
    if (!out.some((seen) => seen.emoji === entry.emoji)) out.push(entry);
  }
  return out;
}

/** A just-closed `:name:` at `colon`: the range to replace and its emoji. */
export function completeShortcode(text: string, colon: number): { start: number; end: number; emoji: string } | null {
  if (text[colon] !== ":") return null;
  const found = scan(text, colon);
  if (!found?.name) return null;
  ensureLoaded();
  const emoji = entries()?.find((e) => e.name === found.name)?.emoji;
  return emoji ? { start: found.colon, end: colon + 1, emoji } : null;
}
