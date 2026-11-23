// The selected chat's transcript and queue, resubscribed after reconnects and desyncs.
import type { StoreSetter } from "solid-js";
import { createEffect, createRoot } from "solid-js";
import { onConnected, rpc } from "./connection";
import { M } from "./methods";
import type { SessionMessageEntry, TranscriptUpdate } from "./protocol";
import type { Subscription } from "./rpc";
import { setState, state } from "./store";
import { applyTranscriptFrame, TranscriptDesync } from "./transcript";
import { restoredChatId, setView, view } from "./view";

/** Chat-scoped requests name the chat's host so remote devices route correctly. */
export function chatRequest(chatId: string, params: Record<string, unknown> = {}) {
  const chat = state.chats[chatId];
  return { chatId, targetDeviceId: chat?.deviceId ?? null, ...params };
}

const setMessages: StoreSetter<SessionMessageEntry[]> = update => {
  setState(draft => { if (draft.transcript) update(draft.transcript.messages); });
};

function watchTranscript(chatId: string): Subscription {
  let ready = false;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let sub: Subscription | undefined;
  const restart = (notice: string) => {
    sub?.cancel();
    setState(draft => { Object.assign(draft["transcript"]!, { ready: false, notice }); });
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
          (() => {
            applyTranscriptFrame(setMessages, state.transcript!.messages, update);
            setState(draft => { Object.assign(draft["transcript"]!, { ready: true, notice: null, contextUsage: update.contextUsage ?? null, ...(reset ? { syncedEpoch: state.epoch } : {}) }); });
          })();
          ready = true;
        } catch (error) {
          // Desyncs are caught before any write. Anything else may have thrown mid-write and left rows' observers
          // stale, so drop every row: the reset then remounts them instead of reconciling into frozen ones.
          if (!(error instanceof TranscriptDesync)) {
            console.error("transcript frame failed", error);
            setState(draft => { draft["transcript"]!["messages"] = []; });
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
  createEffect(() => {
    const chatId=view.selectedChatId;
    const worker=chatId ? state.zappers.flatMap(zapper=>zapper.workers).find(worker=>worker.chatId===chatId) : undefined;
    const waiting=!!worker && (worker.queuePosition !== undefined || !state.chats[worker.chatId]);
    return [chatId,state.epoch,state.connection.kind === "connected",waiting] as const;
  }, ([chatId, , connected, waiting]) => {
    const cleanups: (() => void)[] = [];
    if (!chatId) {
      setState(draft => { draft["transcript"] = null; });
      return;
    }
    if (state.transcript?.chatId !== chatId)
      setState(draft => { draft["transcript"] = { chatId, ready: false, messages: [], contextUsage: null, notice: null }; });
    // A queued/not-yet-created worker has no document to subscribe to. Its
    // Zapper watch will start this effect once the conversation is ready.
    if (!connected || waiting) return;
    const transcript = watchTranscript(chatId);
    const queue = rpc.subscribe<{ items?: unknown[] }>(M.WATCH_QUEUE, chatRequest(chatId), (value) =>
      setState(draft => { draft["queue"]![chatId] = value?.items ?? []; }),
    );
    cleanups.push(() => {
      transcript.cancel();
      queue.cancel();
    });

    return () => cleanups.forEach(cleanup => cleanup());
  });
});

// A chat restored after a reload may have been deleted meanwhile; the first chat list settles it.
createRoot(() => {
  let pending = restoredChatId;
  createEffect(() => [state.workspaceSynced, view.selectedChatId, !!(pending && state.chats[pending])] as const, ([synced, selected, exists]) => {
    if (!pending || !synced) return;
    const id = pending;
    pending = null;
    if (selected === id && !exists) setView(draft => { Object.assign(draft, { selectedChatId: null, rightPanel: null }); });
  });
});

/** The open chat's last known project, so a deletion elsewhere lands on that project's new-chat canvas. */
let seen: { id: string; projectPath: string | null } | null = null;

// The open chat can be deleted elsewhere (another tab or device, an agent). A freshly created chat may be opened
// before its row arrives, so only a chat that was present and then left the list clears the selection.
createRoot(() => {
  createEffect(() => { const id = view.selectedChatId; const chat = id ? state.chats[id] : undefined; return [id, !!chat, chat?.spaceId ? state.spaces[chat.spaceId]?.path ?? null : null, state.workspaceSynced] as const; }, ([id, chat, path, synced]) => {
    if (!id) { seen = null; return; }
    if (chat) {
      seen = { id, projectPath: path };
      return;
    }
    if (!synced || seen?.id !== id) return;
    const projectPath = seen.projectPath;
    seen = null;
    setView(draft => { Object.assign(draft, { selectedChatId: null, rightPanel: null, draftProjectPath: projectPath }); });
  });
});

export { onConnected };
