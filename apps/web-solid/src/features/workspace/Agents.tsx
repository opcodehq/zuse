import { chatRequest } from "@/core/chat";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { SessionMessageEntry, TranscriptUpdate } from "@/core/protocol";
import { state } from "@/core/store";
import { applyTranscriptFrame } from "@/core/transcript";
import { Entry } from "@/features/transcript/Entry";
import { agentStatus, callOf, isSubagentSpawn, type ToolPart } from "@/features/transcript/model";
import { Icon } from "@/ui/Icon";
import { createEffect, createSignal, createStore, For, Show, untrack } from "solid-js";
import { selectedSubagent, setSelectedSubagent } from "./agents";
import "@/features/transcript/transcript.css";

export function Agents(props: { chatId: string; active: boolean }) {
  const agents = () => (state.transcript?.chatId === props.chatId ? state.transcript.messages : []).flatMap(entry =>
    entry.parts.filter((part): part is ToolPart => part.kind === "tool" && isSubagentSpawn(callOf(part)) && !!part.subagentRef));
  const selected = () => selectedSubagent()?.chatId === props.chatId ? selectedSubagent()!.docId : null;
  const [history, setHistory] = createStore<{ messages: SessionMessageEntry[] }>({ messages: [] });
  const [error, setError] = createSignal<string | null>(null);
  const [ready, setReady] = createSignal(false);
  const [retry, setRetry] = createSignal(0);
  createEffect(() => [selected(), state.epoch, state.connection.kind, props.active, retry()] as const, ([docId, , connection, active]) => {
    if (!active || !docId || connection !== "connected") return;
    let cancelled = false;
    let subscription: { cancel(): void } | undefined;
    setHistory(draft => { draft.messages = []; });
    setReady(false);
    setError(null);
    const sourceChat = docId.split("--sub--")[0] || props.chatId;
    const request = { chatId: docId, targetDeviceId: untrack(() => chatRequest(sourceChat).targetDeviceId) };
    const watch = () => {
      let reset = false;
      subscription = rpc.subscribe<TranscriptUpdate>(M.WATCH_DOC_MESSAGES, request, update => {
        if (cancelled) return;
        try {
          if (!reset && !("reset" in update)) throw new Error("History is out of sync. Try again.");
          applyTranscriptFrame(updateMessages => setHistory(draft => { updateMessages(draft.messages); }), history.messages, update);
          reset = true;
          setReady(true);
        } catch (cause) {
          subscription?.cancel();
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      }, cause => { if (!cancelled && cause) setError(cause.message); });
    };
    // Finished agents can have a frozen transcript after the live doc is purged.
    void rpc.call<{ text: string }>(M.FETCH_TOOL_BLOB, {
      blobRef: `${sourceChat}/${docId}`, targetDeviceId: request.targetDeviceId,
    }).then(reply => {
      if (cancelled) return;
      const entries: unknown = JSON.parse(reply.text);
      if (!Array.isArray(entries)) throw new Error("Invalid agent history");
      setHistory(draft => { draft.messages = entries as SessionMessageEntry[]; });
      setReady(true);
    }).catch(() => { if (!cancelled) watch(); });
    return () => { cancelled = true; subscription?.cancel(); };
  });
  return (
    <div class="ws-agents">
      <nav class="ws-agent-list" aria-label="Subagents">
        <Show when={agents().length} fallback={<p class="ws-muted">Delegated agents appear here when your agent starts them.</p>}>
          <For each={agents()}>{part => <button type="button" class="ws-agent-row" aria-pressed={selected() === part.subagentRef ? "true" : "false"} onClick={() => setSelectedSubagent({ chatId: props.chatId, docId: part.subagentRef! })}>
            <Icon name="tool-agent" size={14} />
            <span>{(() => { const call = callOf(part); return call.kind === "unknown" ? call.name : "Agent"; })()}</span>
            <small>{agentStatus(part)}</small>
          </button>}</For>
        </Show>
      </nav>
      <Show when={selected()} fallback={<div class="ws-empty ws-muted">Select an agent to read its conversation.</div>}>
        <Show when={error()}>{message => <div role="alert" class="ws-empty">{message()} <button type="button" onClick={() => setRetry(value => value + 1)}>Try again</button></div>}</Show>
        <Show when={ready()} fallback={<Show when={!error()}><div class="ws-empty" role="status">Loading agent conversation…</div></Show>}>
          <div class="t-root ws-agent-transcript">
            <For each={history.messages}>{entry => <Entry entry={entry} ctx={{ chatId: props.chatId, device: () => state.chats[props.chatId]?.deviceId ?? null, harness: () => state.chats[props.chatId]?.config?.harness, last: () => history.messages.at(-1)?.id === entry.id }} />}</For>
            <Show when={!history.messages.length}><p class="ws-empty ws-muted">Waiting for the agent’s first message…</p></Show>
          </div>
        </Show>
      </Show>
    </div>
  );
}
