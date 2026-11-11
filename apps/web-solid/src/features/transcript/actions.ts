// Transcript actions: each is one engine request. Nothing here edits server data locally.
import { createEffect, createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { chatRequest } from "@/core/chat";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Chat } from "@/core/protocol";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { openChat, view } from "@/core/view";
import { toast } from "@/ui/primitives";

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

function failure(action: string, error: unknown) {
  // A dropped connection mid-mutation may still have applied it; never resend on the user's behalf.
  if (error instanceof RpcError && error.outcomeUnknown) toast(`${action} may have completed. Check before trying again.`, "danger");
  else toast(`${action} failed: ${errorText(error)}`, "danger");
}

// ---------------------------------------------------------------------------
// Agent questions and approvals
// ---------------------------------------------------------------------------

export interface InputAnswer {
  questionId: string;
  labels: string[];
}

/** Requests answered from this tab, hidden until the engine resolves the part. */
const [answered, setAnswered] = createStore<Record<string, true>>({});
export const isAnswered = (requestId: string) => !!answered[requestId];

/** Answers lost with a dropped connection: hidden only until the next connection's history says whether they landed. */
const unsure = new Map<string, { chatId: string; epoch: number }>();

createRoot(() => {
  createEffect(() => {
    const epoch = state.epoch;
    const transcript = state.transcript;
    for (const [requestId, at] of unsure) {
      if (epoch <= at.epoch) continue;
      // The open chat's fresh reset decides; any other chat reloads its history before showing a card anyway.
      if (transcript?.chatId === at.chatId && transcript.syncedEpoch !== epoch) continue;
      unsure.delete(requestId);
      setAnswered(requestId, undefined as never);
    }
  });
});

async function answer(chatId: string, requestId: string, action: string, send: () => Promise<unknown>) {
  setAnswered(requestId, true);
  try {
    await send();
  } catch (error) {
    if (error instanceof RpcError && error.outcomeUnknown) unsure.set(requestId, { chatId, epoch: state.epoch });
    else setAnswered(requestId, undefined as never);
    failure(action, error);
  }
}

export function respondInput(chatId: string, requestId: string, answers: InputAnswer[]) {
  return answer(chatId, requestId, "Answer", () =>
    rpc.call(M.QUEUE_COMMAND, chatRequest(chatId, { command: { kind: "respondInput", requestId, answers } })),
  );
}

export function submitSecret(chatId: string, requestId: string, secretId: string, value: string, decline: boolean) {
  return answer(chatId, requestId, "Secret", () =>
    rpc.call(M.SUBMIT_PRIVATE_SECRET, chatRequest(chatId, { requestId, secretId, decline, value: decline ? "" : value })),
  );
}

// ---------------------------------------------------------------------------
// Fork
// ---------------------------------------------------------------------------

export type ForkTarget = "conversation" | "thread";

export const FORK_MENU: { target: ForkTarget; label: string; detail: string }[] = [
  { target: "conversation", label: "Fork in new tab", detail: "Continue from here in a new tab of this thread, in the same checkout" },
  { target: "thread", label: "Fork into new thread", detail: "Continue from here in a new thread with its own worktree" },
];

let forking = false;

/** Fork `chatId` through its latest completed response and open the copy (desktop `fork_chat`). */
export async function forkChat(chatId: string, target: ForkTarget) {
  const source = state.chats[chatId];
  if (!source || forking) return;
  forking = true;
  try {
    let placement: Record<string, unknown> = { kind: target };
    const space = target === "thread" && source.spaceId ? state.spaces[source.spaceId] : undefined;
    if (space?.gitDetected && space.deviceId === source.deviceId) {
      // The outcome flattens the worktree's fields (`CreateWorktreeOutcome`).
      let worktree: { path: string; branch: string; checkoutId?: string | null };
      try {
        worktree = await rpc.call(M.CREATE_WORKTREE, {
          repoPath: space.path,
          branch: source.branch?.trim() || "HEAD",
          spaceId: space.id,
          targetDeviceId: source.deviceId,
        });
      } catch (error) {
        return failure("Creating a worktree for the fork", error);
      }
      placement = { kind: "thread", cwd: worktree.path, branch: worktree.branch, checkoutId: worktree.checkoutId ?? null };
    }
    const chat = await rpc.call<Chat>(M.FORK_SIDE_CHAT, {
      chatId: crypto.randomUUID(),
      sourceChatId: chatId,
      placement,
      targetDeviceId: source.deviceId,
    });
    if (chat?.id) openChat(chat.id);
  } catch (error) {
    failure("Fork", error);
  } finally {
    forking = false;
  }
}

// ---------------------------------------------------------------------------
// Conversation tabs (desktop `thread_conversation_tabs`)
// ---------------------------------------------------------------------------

const CLOSED = "zuse.web.closedConversations.v1";
const [closed, setClosed] = createStore<Record<string, string[]>>(load());

function load(): Record<string, string[]> {
  try {
    return JSON.parse(localStorage.getItem(CLOSED) ?? "{}") as Record<string, string[]>;
  } catch {
    return {};
  }
}
function save() {
  try {
    localStorage.setItem(CLOSED, JSON.stringify(closed));
  } catch {
    // Private windows may refuse storage; closed tabs then last for the session.
  }
}

export const threadOf = (chat: Chat) => chat.threadId ?? chat.id;
const lastActive = (chat: Chat) => Date.parse(chat.lastMessageAt ?? chat.createdAt) || 0;

/** The selected thread's open tabs, closed conversations (newest first) and the open chat's side chats. An open
 *  side chat shows its parent's strip with its own tab active, so the way back stays one click away. */
export function conversationTabs() {
  const selected = view.selectedChatId ? state.chats[view.selectedChatId] : undefined;
  if (!selected) return null;
  const parent = selected.parentChatId ? state.chats[selected.parentChatId] : undefined;
  const anchor = parent && !parent.parentChatId ? parent : selected;
  const root = threadOf(anchor);
  const rows = Object.values(state.chats)
    .filter((chat) => threadOf(chat) === root && !chat.parentChatId && !chat.archived)
    .sort((a, b) => Number(a.id !== root) - Number(b.id !== root) || Date.parse(a.createdAt) - Date.parse(b.createdAt));
  const hiddenIds = closed[root] ?? [];
  const open = rows.filter((chat) => chat.id === anchor.id || !hiddenIds.includes(chat.id));
  if (!open.includes(anchor)) open.unshift(anchor);
  const hidden = rows.filter((chat) => !open.includes(chat)).sort((a, b) => lastActive(b) - lastActive(a));
  const side = Object.values(state.chats)
    .filter((chat) => chat.parentChatId === anchor.id && (!chat.archived || chat.id === selected.id))
    .sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt));
  return { root, open, hidden, side, selected, anchor };
}

/** Close a tab; the conversation stays in the thread's history. The last open tab never closes. */
export function closeConversation(chatId: string) {
  const tabs = conversationTabs();
  if (!tabs || tabs.open.length <= 1) return;
  const ids = tabs.open.map((chat) => chat.id);
  const at = ids.indexOf(chatId);
  if (at < 0) return;
  const neighbour = ids[at + 1] ?? ids[at - 1];
  setClosed(tabs.root, (list = []) => (list.includes(chatId) ? list : [...list, chatId]));
  save();
  if (view.selectedChatId === chatId && neighbour) openChat(neighbour);
}

export function restoreConversation(chatId: string) {
  const tabs = conversationTabs();
  if (tabs) {
    setClosed(tabs.root, (list = []) => list.filter((id) => id !== chatId));
    save();
  }
  openChat(chatId);
}

let creating = false;

/** The strip's `+`: a new conversation in this thread, sharing its checkout. */
export async function addConversation() {
  const tabs = conversationTabs();
  if (!tabs || tabs.anchor.archived || creating) return;
  creating = true;
  try {
    const chat = await rpc.call<Chat>(M.CREATE_THREAD_SESSION, {
      chatId: crypto.randomUUID(),
      threadId: tabs.root,
      targetDeviceId: tabs.anchor.deviceId,
    });
    if (chat?.id) openChat(chat.id);
  } catch (error) {
    failure("New conversation", error);
  } finally {
    creating = false;
  }
}
