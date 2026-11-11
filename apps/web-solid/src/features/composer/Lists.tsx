// The composer's own lists above the card: the draft stash (Ctrl+S) and sent-message search (Alt+R; Ctrl+R on macOS).
import { createEffect, createSignal, For, on, Show } from "solid-js";
import { createStore } from "solid-js/store";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { attachmentsFor, draftFor, fail, setDraft } from "./drafts";
import { readableDraft } from "./completion";
import { timeAgo } from "@/core/util";

export const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
export const stashKeys = mac ? "⌘S" : "Ctrl+S";

interface Stashed { id: string; text: string; createdAt: number }
interface Entry { text: string; at: number }

const STASH = "zuse.web.composer.stash.v1";
const MAX_STASHED = 20;
const MAX_RESULTS = 50;
const ATTACHMENT_TRAILER = /\n\nAttached images \(local files[^\n]*\):\n[\s\S]*$/;

function loadStash(): Stashed[] {
  try {
    const raw = JSON.parse(localStorage.getItem(STASH) ?? "[]") as unknown;
    return Array.isArray(raw) ? (raw as Stashed[]).filter((s) => typeof s?.text === "string") : [];
  } catch {
    return [];
  }
}
const [stash, setStash] = createStore<{ items: Stashed[] }>({ items: loadStash() });
const saveStash = (items: Stashed[]) => {
  setStash("items", items);
  try {
    localStorage.setItem(STASH, JSON.stringify(items));
  } catch {
    // Storage may be refused; the stash then lasts for the tab.
  }
};
export const stashCount = () => stash.items.length;

type List =
  | { kind: "stash"; active: number; opening: string }
  | { kind: "history"; active: number; origin: string; entries: Entry[] };
const [list, setList] = createSignal<List | null>(null);
export { list as composerList };

const [flash, setFlash] = createSignal<"stashed" | "escape" | null>(null);
let flashTimer: ReturnType<typeof setTimeout> | undefined;
export { flash as composerFlash };
export function showFlash(text: "stashed" | "escape") {
  clearTimeout(flashTimer);
  setFlash(text);
  flashTimer = setTimeout(() => setFlash(null), 1500);
}

const oneLine = (text: string) => text.split(/\s+/).join(" ").trim();

/** Sent prompts in this chat, newest first and unique by their one-line text. */
function sentEntries(): Entry[] {
  const seen = new Set<string>();
  const out: Entry[] = [];
  const messages = state.transcript?.messages ?? [];
  for (let i = messages.length - 1; i >= 0; i--) {
    const entry = messages[i]!;
    if (entry.role !== "user") continue;
    const raw = entry.parts.flatMap((p) => (p.kind === "text" ? [p.text] : [])).join("\n").replace(ATTACHMENT_TRAILER, "");
    const text = readableDraft(raw).text;
    const key = oneLine(text);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    out.push({ text, at: entry.createdAt ?? 0 });
  }
  return out;
}

/** Contiguous matches first, then letters in order. */
function search(entries: Entry[], query: string) {
  const needle = oneLine(query).toLowerCase();
  if (!needle) return entries.slice(0, MAX_RESULTS);
  const hay = entries.map((e) => oneLine(e.text).toLowerCase());
  const scattered = (h: string) => {
    let at = 0;
    for (const ch of needle) {
      at = h.indexOf(ch, at);
      if (at < 0) return false;
      at++;
    }
    return true;
  };
  const contains = entries.filter((_, i) => hay[i]!.includes(needle));
  const loose = entries.filter((_, i) => !hay[i]!.includes(needle) && scattered(hay[i]!));
  return [...contains, ...loose].slice(0, MAX_RESULTS);
}

const rows = (key: string): { text: string; at: number; id?: string }[] => {
  const current = list();
  if (!current) return [];
  if (current.kind === "stash") return stash.items.map((s) => ({ text: s.text, at: s.createdAt, id: s.id }));
  return search(current.entries, draftFor(key).text);
};

export function closeList(restore: boolean, key: string) {
  const current = list();
  setList(null);
  if (current?.kind === "history" && restore) setDraft(key, { text: current.origin, mentions: [], skills: [] });
}

export function toggleStash(key: string) {
  if (list()?.kind === "stash") return closeList(false, key);
  closeList(true, key);
  setList({ kind: "stash", active: 0, opening: draftFor(key).text });
}

/** Ctrl+S: set the draft aside, or open the stash on an empty composer. */
export function stashDraft(key: string) {
  const text = draftFor(key).text;
  if (!text.trim()) return toggleStash(key);
  if (attachmentsFor(key).length) return fail(key, "Remove attachments before stashing this draft.");
  saveStash([{ id: crypto.randomUUID(), text, createdAt: Date.now() }, ...stash.items].slice(0, MAX_STASHED));
  closeList(false, key);
  setDraft(key, { text: "", mentions: [], skills: [] });
  showFlash("stashed");
}

function restore(index: number, key: string) {
  const entry = stash.items[index];
  if (!entry) return;
  setList(null);
  const current = draftFor(key).text;
  setDraft(key, { text: current.trim() ? `${current.trimEnd()}\n\n${entry.text}` : entry.text });
  saveStash(stash.items.filter((s) => s.id !== entry.id));
}

function remove(index: number) {
  const entry = stash.items[index];
  if (!entry) return;
  saveStash(stash.items.filter((s) => s.id !== entry.id));
  const current = list();
  if (current?.kind === "stash") setList({ ...current, active: Math.min(current.active, Math.max(0, stash.items.length - 1)) });
}

/** Alt+R: the draft becomes a search over sent messages; pressing it again steps to the next match. */
export function searchHistory(key: string) {
  const current = list();
  if (current?.kind === "history") {
    setList({ ...current, active: Math.min(current.active + 1, Math.max(0, rows(key).length - 1)) });
    return;
  }
  closeList(false, key);
  setList({ kind: "history", active: 0, origin: draftFor(key).text, entries: sentEntries() });
}

function accept(key: string) {
  const current = list();
  if (!current) return;
  if (current.kind === "stash") return restore(current.active, key);
  const row = rows(key)[current.active];
  if (!row) return;
  setList(null);
  setDraft(key, readableDraft(row.text));
}

/** Keys while a list is open: arrows move, Enter takes the row, Escape cancels, Delete drops a stashed row. Returns whether it handled the key. */
export function listKeyDown(event: KeyboardEvent, key: string): boolean {
  const current = list();
  if (!current || event.isComposing) return false;
  const count = rows(key).length;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (count) setList({ ...current, active: (current.active + (event.key === "ArrowDown" ? 1 : -1) + count) % count });
    return true;
  }
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    accept(key);
    return true;
  }
  if (event.key === "Escape") {
    event.preventDefault();
    closeList(true, key);
    return true;
  }
  if (current.kind === "stash" && (event.key === "Delete" || (event.key === "Backspace" && (mac ? event.metaKey : event.ctrlKey)))) {
    event.preventDefault();
    remove(current.active);
    return true;
  }
  return false;
}

export function ComposerList(props: { draftKey: string }) {
  let listbox: HTMLDivElement | undefined;
  const listId = `cmp-list-${Math.random().toString(36).slice(2)}`;
  // Typing puts the stash away; the search follows its query from the first match.
  createEffect(on(() => draftFor(props.draftKey).text, (text) => {
    const current = list();
    if (current?.kind === "stash" && current.opening !== text) setList(null);
    if (current?.kind === "history" && current.active !== 0) setList({ ...current, active: 0 });
  }, { defer: true }));
  createEffect(on(() => view.selectedChatId, () => setList(null), { defer: true }));
  createEffect(on(() => list()?.active, (index) => listbox?.querySelector(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" })));
  const items = () => rows(props.draftKey);
  return (
    <Show when={list()}>
      {(current) => (
        <div
          class="cmp-popup cmp-list"
          role="dialog"
          aria-label={current().kind === "stash" ? "Stashed prompts" : "Sent messages"}
          onMouseDown={(event) => event.preventDefault()}
        >
          <div class="cmp-list-head">
            <Show
              when={current().kind === "stash"}
              fallback={
                <>
                  <Icon name="palette-search" size={14} />
                  <span class="cmp-list-title">Searching sent messages</span>
                  <kbd class="cmp-keycap">↵</kbd> use <kbd class="cmp-keycap">Esc</kbd> cancel
                </>
              }
            >
              <Icon name="bookmark" size={14} />
              <span class="cmp-list-title">Stash</span>
              {stash.items.length}
            </Show>
          </div>
          <Show
            when={items().length}
            fallback={
              <div class="cmp-list-empty">
                {current().kind === "stash"
                  ? `Nothing stashed yet. Press ${stashKeys} with a draft to stash it.`
                  : (current() as Extract<List, { kind: "history" }>).entries.length
                    ? "No matches"
                    : "No messages sent in this chat yet."}
              </div>
            }
          >
            <div class="cmp-popup-list" role="listbox" id={listId} ref={listbox} aria-label={current().kind === "stash" ? "Stashed prompts" : "Sent messages"}>
              <For each={items()}>
                {(row, index) => (
                  <div
                    id={`${listId}-${index()}`}
                    class="cmp-row cmp-list-row"
                    role="option"
                    data-index={index()}
                    aria-selected={current().active === index()}
                    onMouseMove={() => current().active !== index() && setList({ ...current(), active: index() })}
                    onClick={() => {
                      setList({ ...current(), active: index() });
                      accept(props.draftKey);
                    }}
                  >
                    <span class="cmp-list-text">{oneLine(row.text)}</span>
                    <span class="cmp-list-time">{timeAgo(row.at)}</span>
                    <Show when={row.id}>
                      <button
                        type="button"
                        class="cmp-list-delete"
                        aria-label="Delete stashed prompt"
                        title="Delete stashed prompt"
                        tabIndex={-1}
                        onClick={(event) => {
                          event.stopPropagation();
                          remove(index());
                        }}
                      >
                        <Icon name="close" size={12} />
                      </button>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </div>
      )}
    </Show>
  );
}

/** The toolbar's stash key: a bookmark and the count, once something is stashed. */
export function StashButton(props: { draftKey: string }) {
  return (
    <Show when={stashCount() > 0}>
      <button
        type="button"
        class="cmp-key cmp-stash"
        data-open={list()?.kind === "stash"}
        aria-label={`Stashed prompts: ${stashCount()}. Open stash.`}
        aria-expanded={list()?.kind === "stash"}
        title={`Stashed prompts: ${stashCount()}. Open stash.`}
        onMouseDown={(event) => event.preventDefault()}
        onClick={() => toggleStash(props.draftKey)}
      >
        <Icon name="bookmark" size={14} />
        {stashCount()}
      </button>
    </Show>
  );
}
