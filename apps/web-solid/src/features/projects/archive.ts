// Project archives: archived chats, a read-only transcript of one session, its file archive state, restore.
import { batch } from "solid-js";
import { createStore, type SetStoreFunction } from "solid-js/store";
import { chatRequest } from "@/core/chat";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Chat, SessionMessageEntry, TranscriptUpdate } from "@/core/protocol";
import type { Subscription } from "@/core/rpc";
import { state } from "@/core/store";
import { applyTranscriptFrame } from "@/core/transcript";
import { errorText, projectKey } from "./hosts";
import type { FileArchiveStatus } from "./types";

export const chatDate = (chat: Chat) => chat.lastMessageAt ?? chat.createdAt;

export const chatTitle = (chat: Chat) => (chat.title && chat.title.trim() ? chat.title : "Untitled chat");

export function chatSummary(chat: Chat): string {
  const preview = chat.lastMessagePreview?.split(/\s+/).filter(Boolean).join(" ");
  if (preview) return preview;
  return chat.branch ? `Branch ${chat.branch}` : "Saved conversation";
}

export function dateLabel(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleDateString(undefined, sameYear ? { month: "short", day: "numeric" } : { month: "short", day: "numeric", year: "numeric" });
}

export const activityLabel = (chat: Chat) => `${chat.lastMessageAt ? "Last active" : "Created"} ${dateLabel(chatDate(chat))}`;

/** Space ids a route's project key covers: one space, or every checkout of a repository. */
export function projectSpaces(key: string | undefined): Set<string> | null {
  if (!key) return null;
  return new Set(
    Object.values(state.spaces)
      .filter((space) => space.id === key || projectKey(space) === key)
      .map((space) => space.id),
  );
}

const isTopLevel = (chat: Chat) => !chat.parentChatId;

/** Archived conversations (thread heads), newest first, matching title, preview or branch. */
export function archivedThreads(spaces: Set<string> | null, query: string): Chat[] {
  const needle = query.trim().toLowerCase();
  return Object.values(state.chats)
    .filter((c) => c.archived && !c.threadId && isTopLevel(c) && (!spaces || (c.spaceId && spaces.has(c.spaceId))))
    .filter(
      (c) =>
        !needle ||
        chatTitle(c).toLowerCase().includes(needle) ||
        (c.lastMessagePreview ?? "").toLowerCase().includes(needle) ||
        (c.branch ?? "").toLowerCase().includes(needle),
    )
    .sort((a, b) => chatDate(b).localeCompare(chatDate(a)) || a.id.localeCompare(b.id));
}

/** Every session of a thread, oldest first. */
export function threadSessions(threadId: string, spaces: Set<string> | null): Chat[] {
  return Object.values(state.chats)
    .filter((c) => (c.threadId ?? c.id) === threadId && (!spaces || (c.spaceId && spaces.has(c.spaceId))))
    .sort((a, b) => a.createdAt.localeCompare(b.createdAt));
}

// ── Read-only transcript ─────────────────────────────────────────────────

interface Preview {
  chatId: string | null;
  messages: SessionMessageEntry[];
  reading: boolean;
  error: string | null;
}

export const [preview, setPreview] = createStore<Preview>({ chatId: null, messages: [], reading: false, error: null });

const setMessages: SetStoreFunction<SessionMessageEntry[]> = ((...args: unknown[]) =>
  (setPreview as (...a: unknown[]) => void)("messages", ...args)) as SetStoreFunction<SessionMessageEntry[]>;

let reading: Subscription | null = null;

export function readSession(chatId: string) {
  closeSession();
  setPreview({ chatId, messages: [], reading: true, error: null });
  if (!rpc.connected) {
    setPreview({ reading: false, error: "Computer unavailable. Reconnect to load this transcript." });
    return;
  }
  let received = false;
  const sub = rpc.subscribe<TranscriptUpdate>(
    M.WATCH_DOC_MESSAGES,
    chatRequest(chatId),
    (update) => {
      if (preview.chatId !== chatId || reading !== sub) return;
      try {
        if (!received && !("reset" in update)) throw new Error("missing reset");
        batch(() => {
          applyTranscriptFrame(setMessages, preview.messages, update);
          setPreview({ reading: false, error: null });
        });
        received = true;
      } catch {
        setPreview({ reading: false, error: "Could not read saved transcript. Retry to load it again." });
        sub.cancel();
      }
    },
    (error) => {
      if (preview.chatId !== chatId || reading !== sub) return;
      reading = null;
      if (error || !received) setPreview({ reading: false, error: error ? errorText(error) : "Could not read saved transcript. Retry to load it again." });
    },
  );
  reading = sub;
}

export function closeSession() {
  const sub = reading;
  reading = null;
  sub?.cancel();
  setPreview({ chatId: null, messages: [], reading: false, error: null });
}

// A dropped connection ends the read and the status check; resume both when it returns.
onConnected(() => {
  if (preview.chatId && !reading) readSession(preview.chatId);
  if (files.chatId && files.error) void loadFiles(files.chatId);
});

// ── File archive state ───────────────────────────────────────────────────

interface Files {
  chatId: string | null;
  value: FileArchiveStatus | null;
  loading: boolean;
  error: string | null;
}

export const [files, setFiles] = createStore<Files>({ chatId: null, value: null, loading: false, error: null });

let filesEpoch = 0;
let filesPoll: ReturnType<typeof setTimeout> | undefined;

export const progressing = (status: FileArchiveStatus | null) => status?.phase === "prepared" || status?.phase === "restoring";

export async function loadFiles(chatId: string) {
  clearTimeout(filesPoll);
  if (files.loading && files.chatId === chatId) return;
  const epoch = ++filesEpoch;
  if (files.chatId !== chatId) setFiles({ chatId, value: null, error: null });
  setFiles("loading", true);
  try {
    const status = await rpc.call<FileArchiveStatus>(M.GET_CHAT_ARCHIVE_STATUS, chatRequest(chatId));
    if (epoch !== filesEpoch) return;
    if (status?.chatId === chatId) setFiles({ value: status, error: null, loading: false });
    else setFiles({ error: "File archive response is unavailable.", loading: false });
  } catch (error) {
    if (epoch !== filesEpoch) return;
    setFiles({ error: errorText(error), loading: false });
  }
  if (!files.error && progressing(files.value)) filesPoll = setTimeout(() => void loadFiles(chatId), 2000);
}

export function clearFiles() {
  filesEpoch++;
  clearTimeout(filesPoll);
  setFiles({ chatId: null, value: null, loading: false, error: null });
}

// ── Restore ──────────────────────────────────────────────────────────────

export const [restoring, setRestoring] = createStore<{ busy: string | null; target: string | null; error: string | null }>({
  busy: null,
  target: null,
  error: null,
});

export async function restoreChat(chatId: string): Promise<boolean> {
  if (restoring.busy) return false;
  const chat = state.chats[chatId];
  if (!chat?.archived) return false;
  setRestoring({ busy: chatId, target: chatId, error: null });
  try {
    await rpc.call(M.MUTATE, { op: "setChatArchived", chatId, archived: false, targetDeviceId: chat.deviceId });
    setRestoring({ busy: null });
    return true;
  } catch (error) {
    setRestoring({ busy: null, error: errorText(error, "Could not restore this chat.") });
    return false;
  }
}
