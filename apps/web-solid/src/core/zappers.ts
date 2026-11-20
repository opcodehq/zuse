// Ownership is shared by navigation, the sidebar, search, and Zapper controls.
import { state } from "./store";

export function zapperForChat(chatId: string | null | undefined) {
  const visited = new Set<string>();
  const pending = chatId ? [chatId] : [];
  while (pending.length) {
    const id = pending.pop()!;
    if (visited.has(id)) continue;
    visited.add(id);
    const owner = state.zappers.find(
      (z) =>
        z.coordinatorChatId === id ||
        z.previousCoordinatorChats.includes(id) ||
        z.workers.some((w) => w.chatId === id),
    );
    if (owner) return owner;
    const chat = state.chats[id];
    if (chat?.threadId) pending.push(chat.threadId);
    if (chat?.parentChatId) pending.push(chat.parentChatId);
  }
  return undefined;
}

export const isZapperChat = (chatId: string) => !!zapperForChat(chatId);
