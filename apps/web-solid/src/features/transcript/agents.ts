import type { SessionMessageEntry } from "@/core/protocol";
import type { ToolCall, ToolPart } from "./model";

export const callOf = (part: ToolPart) => part.call as unknown as ToolCall;
export type SubagentStatus = "running" | "done" | "failed";

export function isSubagentSpawn(call: ToolCall) {
  const name = call.kind === "unknown" ? call.name : call.kind === "mcp" ? call.tool : null;
  return name !== null && (name === "Agent" || name.startsWith("Agent: "));
}

export function agentStatus(part: ToolPart): SubagentStatus {
  if (part.subagentStatus === "running" || part.subagentStatus === "done" || part.subagentStatus === "failed") return part.subagentStatus;
  if (part.isError) return "failed";
  return part.resolved ? "done" : "running";
}

export function chatAgents(entries: SessionMessageEntry[]) {
  const agents = new Map<string, { docId: string; title: string; status: ReturnType<typeof agentStatus>; tail?: string }>();
  for (const entry of entries) for (const part of entry.parts) {
    if (part.kind !== "tool" || !part.subagentRef || !isSubagentSpawn(callOf(part))) continue;
    const call = callOf(part);
    agents.set(part.subagentRef, {
      docId: part.subagentRef,
      title: call.kind === "unknown" ? call.name.replace(/^Agent: ?/, "") || "Agent" : "Agent",
      status: agentStatus(part),
      tail: part.subagentTail,
    });
  }
  return [...agents.values()];
}

export function agentSummary(agents: ReturnType<typeof chatAgents>) {
  const count = (status: string) => agents.filter(agent => agent.status === status).length;
  return ([["running", "working"], ["failed", "failed"], ["done", "complete"]] as const)
    .flatMap(([status, label]) => count(status) ? [`${count(status)} ${label}`] : []).join(" · ");
}

export const statusLabel = (status: SubagentStatus) => status === "running" ? "Working" : status === "done" ? "Complete" : "Failed";

/** The composer line: one agent reads as its own name and state; several read as counts per state. */
export function agentHeadline(agents: ReturnType<typeof chatAgents>) {
  const [only] = agents;
  if (agents.length === 1 && only) return { title: "Subagent", detail: `${only.title} · ${statusLabel(only.status)}` };
  return { title: `${agents.length} subagents`, detail: agentSummary(agents) };
}
