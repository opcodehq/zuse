import { state } from "@/core/store";
import { view, setView } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { openSubagent } from "@/features/workspace/agents";
import { agentHeadline, chatAgents, statusLabel } from "@/features/transcript/agents";
import { Icon } from "@/ui/Icon";
import { For, Show } from "solid-js";
import { showPage } from "./panel";
import { setInspectorOpen } from "./state";

export const selectedChatAgents = () => state.transcript?.chatId === view.selectedChatId ? chatAgents(state.transcript.messages) : [];
const lastKnown = () => state.connection.kind === "connected" ? "" : "Last known: ";

export function SubagentSummary() {
  const owner = () => zapperForChat(view.selectedChatId);
  const headline = () => agentHeadline(selectedChatAgents());
  return <Show when={owner() && selectedChatAgents().length}>
    <button type="button" class="cmp-pill cmp-subagents" aria-label={`${headline().title}: ${lastKnown()}${headline().detail}. Open Overview`} onClick={() => {
      showPage(owner()!.id, { kind: "section", name: "overview" });
      setInspectorOpen(true);
      setView(draft => { draft.rightPanel = null; });
    }}>
      <span class="cmp-pill-icon"><Icon name="tool-agent" size={14} /></span>
      <span class="cmp-pill-body">
        <span class="cmp-pill-title">{headline().title}</span>
        <span class="cmp-pill-sub cmp-subagent-status">{lastKnown()}{headline().detail}</span>
      </span>
      <span class="cmp-pill-actions"><Icon name="alt-arrow-right" size={12} /></span>
    </button>
  </Show>;
}

export function OverviewSubagents() {
  return <section class="zp-subagents" aria-label="Subagents">
    <div class="zp-section-heading"><h3>Subagents</h3><span class="zp-muted">{selectedChatAgents().length}</span></div>
    <Show when={selectedChatAgents().length} fallback={<p class="zp-muted">No subagents in this conversation yet.</p>}>
      <For each={selectedChatAgents()}>{agent => <button type="button" class="zp-subagent-row" onClick={() => openSubagent(view.selectedChatId!, agent.docId, agent.title)}>
        <Icon name="tool-agent" size={14} />
        <span class="zp-subagent-copy"><strong>{agent.title}</strong><Show when={agent.tail}><span>{agent.tail}</span></Show></span>
        <span class="zp-subagent-state" data-status={agent.status}>{lastKnown()}{statusLabel(agent.status)}</span>
        <Icon name="alt-arrow-right" size={12} />
      </button>}</For>
    </Show>
  </section>;
}
