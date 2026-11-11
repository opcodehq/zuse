// User actions from the sidebar. Each is one engine request; the watches bring the result back.
import { createEffect, createRoot } from "solid-js";
import { chatRequest } from "@/core/chat";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Chat } from "@/core/protocol";
import { state } from "@/core/store";
import { navigate, openChat, setView, view } from "@/core/view";
import { toast } from "@/ui/primitives";
import { isArchived, overlayArchived, overlayTitle } from "./overlay";
import { projectMembers, unseen } from "./model";
import { reportError, uuid } from "./util";

function connected() {
  if (rpc.connected) return true;
  toast("Engine not connected", "danger");
  return false;
}

const seenSent = new Map<string, string>();

/** Clears a fresh "Done" on every device. Desktop marks on open and while the chat stays open. */
export function markSeen(chatId: string) {
  const chat = state.chats[chatId];
  if (!chat || !unseen(chat) || !rpc.connected) return;
  const marker = chat.lastMessageAt ?? "";
  if (seenSent.get(chatId) === marker) return;
  seenSent.set(chatId, marker);
  rpc.call(M.MUTATE, { op: "markChatSeen", chatId }).catch(() => seenSent.delete(chatId));
}

export function selectChat(chatId: string) {
  openChat(chatId);
  markSeen(chatId);
}

// The open chat stays seen while new replies land in it, as desktop's does.
createRoot(() => {
  createEffect(() => {
    const id = view.route.kind === "chat" ? view.selectedChatId : null;
    const chat = id ? state.chats[id] : undefined;
    if (chat && unseen(chat)) markSeen(chat.id);
  });
});

export function newChat(projectKey?: string) {
  if (projectKey) {
    const space = projectMembers(projectKey)[0];
    setView({ selectedChatId: null, route: { kind: "chat" }, draftProjectPath: space?.path ?? null });
  } else openChat(null);
}

export const newProject = () => navigate({ kind: "newProject" });
export const openPlugins = () => navigate({ kind: "plugins" });

export function toggleSettings() {
  navigate(view.route.kind === "settings" ? { kind: "chat" } : { kind: "settings" });
}

export function renameChat(chatId: string, title: string) {
  const chat = state.chats[chatId];
  const next = title.trim();
  if (!chat || !next || next === (chat.title ?? "") || !connected()) return;
  overlayTitle(chatId, next);
  rpc.call(M.MUTATE, chatRequest(chatId, { op: "renameChat", title: next })).catch((error: unknown) => {
    overlayTitle(chatId, undefined);
    reportError("Couldn't rename", error);
  });
}

export function setArchived(chatId: string, archived: boolean) {
  const chat = state.chats[chatId];
  if (!chat || isArchived(chat) === archived || !connected()) return;
  overlayArchived(chatId, archived);
  rpc.call(M.MUTATE, chatRequest(chatId, { op: "setChatArchived", archived })).catch((error: unknown) => {
    overlayArchived(chatId, undefined);
    reportError(archived ? "Couldn't archive" : "Couldn't unarchive", error);
  });
}

export function deleteChat(chatId: string) {
  if (!connected()) return;
  if (state.sessions[chatId]?.status === "working") {
    toast("Stop this thread’s agent before deleting it.", "danger");
    return;
  }
  rpc.call(M.MUTATE, chatRequest(chatId, { op: "deleteChat" })).then(
    () => {
      if (view.selectedChatId === chatId) openChat(null);
    },
    (error: unknown) => reportError("Couldn't delete", error),
  );
}

let forking = false;

/** Fork through the latest completed response into a thread of its own. In a Git project the copy
 * gets a fresh worktree off the source's branch first, as desktop's "Fork to new thread" does. */
export async function forkChat(chatId: string) {
  const source = state.chats[chatId];
  if (!source || forking || !connected()) return;
  forking = true;
  try {
    const space = source.spaceId ? state.spaces[source.spaceId] : undefined;
    let placement: Record<string, unknown> = { kind: "thread" };
    if (space?.gitDetected && space.deviceId === source.deviceId) {
      type Worktree = { path: string; branch: string; checkoutId?: string | null };
      const created = await rpc.call<Worktree & { worktree?: Worktree }>(M.CREATE_WORKTREE, {
        repoPath: space.path,
        branch: source.branch?.trim() || "HEAD",
        spaceId: space.id,
        targetDeviceId: source.deviceId,
      });
      // Older hosts wrap the worktree in an outcome.
      const worktree = created.worktree ?? created;
      placement = { kind: "thread", cwd: worktree.path, branch: worktree.branch, checkoutId: worktree.checkoutId ?? null };
    }
    const fork = await rpc.call<Chat>(M.FORK_SIDE_CHAT, {
      chatId: uuid(),
      sourceChatId: source.id,
      placement,
      targetDeviceId: source.deviceId,
    });
    if (fork?.id) openChat(fork.id);
  } catch (error) {
    reportError("Couldn't fork", error);
  } finally {
    forking = false;
  }
}

export function renameProject(spaceId: string, name: string) {
  if (!connected()) return;
  const trimmed = name.trim();
  rpc.call(M.MUTATE, { op: "renameSpace", spaceId, name: trimmed || null }).catch((error: unknown) => reportError("Couldn't rename project", error));
}

export function removeProject(spaceId: string) {
  if (!connected()) return;
  rpc.call(M.MUTATE, { op: "deleteSpace", spaceId }).then(
    () => {
      if (view.selectedChatId && state.chats[view.selectedChatId]?.spaceId === spaceId) openChat(null);
    },
    (error: unknown) => reportError("Couldn't remove project", error),
  );
}

export function openProjectArchive(projectKey: string) {
  navigate({ kind: "archive", projectKey });
}

export function toggleRightPanel(panel: "git") {
  const open = view.route.kind === "chat" && view.rightPanel === panel;
  setView({ route: { kind: "chat" }, rightPanel: open ? null : panel });
}

