// The selected chat's transcript and queue, resubscribed after reconnects and desyncs.
import { batch, createEffect, createRoot, on, onCleanup } from "solid-js";
import { onConnected, rpc } from "./connection";
import { M } from "./methods";
import type { SessionMessageEntry, TranscriptUpdate } from "./protocol";
import { setState, state } from "./store";
import { applyTranscriptFrame, TranscriptDesync } from "./transcript";
import { restoredChatId, setView, view } from "./view";
import type { SetStoreFunction } from "solid-js/store";
import type { Subscription } from "./rpc";

/** Chat-scoped requests name the chat's host so remote devices route correctly. */
export function chatRequest(chatId: string, params: Record<string, unknown> = {}) {
  const chat = state.chats[chatId];
  return { chatId, targetDeviceId: chat?.deviceId ?? null, ...params };
}

const setMessages: SetStoreFunction<SessionMessageEntry[]> = ((...args: unknown[]) =>
  (setState as (...a: unknown[]) => void)("transcript", "messages", ...args)) as SetStoreFunction<SessionMessageEntry[]>;

function watchTranscript(chatId: string): Subscription {
  let ready = false;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let sub: Subscription | undefined;
  const restart = (notice: string) => {
    sub?.cancel();
    setState("transcript", { ready: false, notice });
    retry = setTimeout(open, 600);
  };
  const open = () => {
    ready = false;
    sub = rpc.subscribe<TranscriptUpdate>(
      M.WATCH_DOC_MESSAGES,
      chatRequest(chatId),
      (update) => {
        if (state.transcript?.chatId !== chatId) return;
        const reset = "reset" in update;
        if (!ready && !reset) return restart("Refreshing chat history…");
        try {
          batch(() => {
            applyTranscriptFrame(setMessages, state.transcript!.messages, update);
            setState("transcript", { ready: true, notice: null, contextUsage: update.contextUsage ?? null, ...(reset ? { syncedEpoch: state.epoch } : {}) });
          });
          ready = true;
        } catch (error) {
          // Desyncs are caught before any write. Anything else may have thrown mid-write and left rows' observers
          // stale, so drop every row: the reset then remounts them instead of reconciling into frozen ones.
          if (!(error instanceof TranscriptDesync)) {
            console.error("transcript frame failed", error);
            setState("transcript", "messages", []);
          }
          restart("Refreshing chat history…");
        }
      },
      (error) => {
        if (error && rpc.connected) restart("Refreshing chat history…");
      },
    );
  };
  open();
  return {
    cancel() {
      clearTimeout(retry);
      sub?.cancel();
    },
  };
}

createRoot(() => {
  createEffect(
    on(
      () => [view.selectedChatId, state.epoch, state.connection.kind === "connected"] as const,
      ([chatId, , connected]) => {
        if (!chatId) {
          setState("transcript", null);
          return;
        }
        if (state.transcript?.chatId !== chatId)
          setState("transcript", { chatId, ready: false, messages: [], contextUsage: null, notice: null });
        if (!connected) return;
        const transcript = watchTranscript(chatId);
        const queue = rpc.subscribe<{ items?: unknown[] }>(M.WATCH_QUEUE, chatRequest(chatId), (value) =>
          setState("queue", chatId, value?.items ?? []),
        );
        onCleanup(() => {
          transcript.cancel();
          queue.cancel();
        });
      },
    ),
  );
});

// A chat restored after a reload may have been deleted meanwhile; the first chat list settles it.
createRoot(() => {
  let pending = restoredChatId;
  createEffect(() => {
    if (!pending || !state.workspaceSynced) return;
    const id = pending;
    pending = null;
    if (view.selectedChatId === id && !state.chats[id]) setView({ selectedChatId: null, rightPanel: null });
  });
});

/** The open chat's last known project, so a deletion elsewhere lands on that project's new-chat canvas. */
let seen: { id: string; projectPath: string | null } | null = null;

// The open chat can be deleted elsewhere (another tab or device, an agent). A freshly created chat may be opened
// before its row arrives, so only a chat that was present and then left the list clears the selection.
createRoot(() => {
  createEffect(() => {
    const id = view.selectedChatId;
    if (!id) return void (seen = null);
    const chat = state.chats[id];
    if (chat) {
      seen = { id, projectPath: chat.spaceId ? (state.spaces[chat.spaceId]?.path ?? null) : null };
      return;
    }
    if (!state.workspaceSynced || seen?.id !== id) return;
    const projectPath = seen.projectPath;
    seen = null;
    setView({ selectedChatId: null, rightPanel: null, draftProjectPath: projectPath });
  });
});

export { onConnected };
