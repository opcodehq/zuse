import { render } from "@solidjs/web";
import { createSignal, flush } from "solid-js";
import { Issues } from "@/features/zappers/Issues";
import { IssuePriorityIcon, IssueStatusIcon, issueStatuses } from "@/features/zappers/IssueMetadata";
import { ZapperSelect } from "@/features/zappers/Controls";
import { setState } from "@/core/store";
import type { IssuePriority, IssueStatus, Zapper } from "@/core/zapper-types";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/features/zappers/zappers.css";

const priorities: IssuePriority[] = ["urgent", "high", "normal", "low"];
const statuses = Object.keys(issueStatuses) as IssueStatus[];
const titles = ["Capture the rollout follow-up", "Triage the next task", "Polish the issue metadata", "Approve the release plan", "Resolve the deployment blocker", "Review the accessibility checks", "Ship the completed changes", "Drop the superseded task"];
const sample = {
  id: "fixture", config: { name: "Release" }, workers: [{ chatId: "worker", deviceId: "local", objective: "Polish metadata" }],
  issues: statuses.map((status, i) => ({ id: status, number: i + 1, details: { title: titles[i], description: "Fixture data for production issue presentation.", status, priority: priorities[i % 4], assigned: true, owner: { type: "thread", chatId: "worker" }, dueDate: null, labels: [], relations: [] }, threads: [], pullRequests: [], activity: [] })),
} as unknown as Zapper;
setState(draft => { draft.connection = { kind: "connected" }; draft.localDeviceId = "local"; draft.sessions.worker = { chatId: "worker", status: "working", updatedAt: new Date().toISOString() }; });
const [status, setStatus] = createSignal<IssueStatus>("needsYou");
const [priority, setPriority] = createSignal<IssuePriority>("urgent");
Object.assign(window, { issueFixture: {
  session: (status: string) => { setState(draft => { draft.sessions.worker!.status = status; }); flush(); },
  stale: (stale: boolean) => { setState(draft => { draft.sessions.worker!.updatedAt = new Date(Date.now() - (stale ? 60_000 : 0)).toISOString(); }); flush(); },
  connected: (connected: boolean) => { setState(draft => { draft.connection = { kind: connected ? "connected" : "connecting" }; }); flush(); },
} });
render(() => <main style={{ width: "740px", margin: "40px auto", color: "var(--text)", background: "var(--bg)", "font-family": "var(--font-sans)" }}><h2 style={{ "font-size": "16px", "font-weight": 500 }}>Issue metadata</h2><p style={{ color: "var(--text-muted)", "font-size": "12px" }}>Production issue rows and picker controls · fixture data</p><div style={{ display: "flex", gap: "12px", "margin-bottom": "24px" }}><ZapperSelect label="Status preview" value={status()} onChange={value => setStatus(value as IssueStatus)} options={statuses.map(value => ({ value, label: issueStatuses[value].label, leading: <IssueStatusIcon status={value} /> }))} /><ZapperSelect label="Priority preview" value={priority()} onChange={value => setPriority(value as IssuePriority)} options={priorities.map(value => ({ value, label: value[0]!.toUpperCase() + value.slice(1), leading: <IssuePriorityIcon priority={value} /> }))} /></div><Issues zapper={sample} /></main>, document.getElementById("root")!);
document.body.style.background = "var(--bg)";
