// Pending user intents shown before the engine's watch confirms them, plus the sidebar's small UI
// state. Never a copy of server data: each intent clears once the watched row agrees, or reverts
// when the request fails.
import type { Chat } from "@/core/protocol";
import { state } from "@/core/store";
import { createEffect, createRoot, createSignal, createStore } from "solid-js";

const [archived, setArchived] = createStore<Record<string, boolean | undefined>>({});
const [titles, setTitles] = createStore<Record<string, string | undefined>>({});

export function isArchived(chat: Chat) {
  const pending = archived[chat.id];
  return pending === undefined ? chat.archived : pending;
}

export function titleOf(chat: Chat) {
  return titles[chat.id] ?? chat.title ?? "";
}

export function overlayArchived(chatId: string, value: boolean | undefined) {
  setArchived(draft => { draft[chatId] = value; });
}

export function overlayTitle(chatId: string, value: string | undefined) {
  setTitles(draft => { draft[chatId] = value; });
}

// Drop intents the watch already reflects. Reads only the rows that carry an intent.
createRoot(() => {
  createEffect(
    () => Object.entries(archived).filter(([id, value]) => value !== undefined && (!state.chats[id] || state.chats[id]!.archived === value)).map(([id]) => id),
    ids => { setArchived(draft => { for (const id of ids) delete draft[id]; }); },
  );
  createEffect(
    () => Object.entries(titles).filter(([id, value]) => value !== undefined && (!state.chats[id] || state.chats[id]!.title === value)).map(([id]) => id),
    ids => { setTitles(draft => { for (const id of ids) delete draft[id]; }); },
  );
});

/** The chat being renamed in place, if any. */
export const [renaming, setRenaming] = createSignal<string | null>(null);

/** The chat being dragged, so drop zones can show themselves. */
export const [dragging, setDragging] = createSignal<string | null>(null);

export const [searchOpen, setSearchOpen] = createSignal(false);
export const [query, setQuery] = createSignal("");

export type SidebarDialog =
  | { kind: "deleteChat"; chatId: string }
  | { kind: "renameProject"; spaceId: string }
  | { kind: "removeProject"; spaceId: string }
  | { kind: "section"; sectionId: string | null; assign?: string };

export const [dialog, setDialog] = createSignal<SidebarDialog | null>(null);
