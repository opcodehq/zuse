// How an issue's activity reads; mirrors crates/viewport/src/zapper_issue_timeline.rs.
import type { IssueActivity, IssueDelivery, IssueStatus, Zapper, ZapperIssue } from "@/core/zapper-types";

const FOLD = 3;
export const statusLabels: Record<IssueStatus, string> = {
  backlog: "Backlog",
  todo: "Todo",
  inProgress: "In progress",
  needsYou: "Needs you",
  blocked: "Blocked",
  inReview: "In review",
  done: "Done",
  canceled: "Canceled",
};
const priorityLabels: Record<string, string> = { urgent: "Urgent", high: "High", normal: "Normal", low: "Low" };

export type Item =
  | { kind: "card"; event: IssueActivity }
  | { kind: "event"; event: IssueActivity }
  | { kind: "group"; events: IssueActivity[] };

export const isDiscussion = (e: IssueActivity) => ["comment", "workerReport", "question"].includes(e.kind);
const statusOf = (value?: string): IssueStatus | undefined =>
  value && value in statusLabels ? (value as IssueStatus) : undefined;

function needsAttention(e: IssueActivity) {
  if (!e.changes?.length) return e.text.includes("Needs you") || e.text.includes("Blocked");
  return e.changes.some(c => c.field === "status" && (c.after === "needsYou" || c.after === "blocked"));
}

/** Top-level activity in stored order, with quiet runs folded. */
export function timeline(activity: IssueActivity[]): Item[] {
  const items: Item[] = [];
  let run: IssueActivity[] = [];
  const flush = () => {
    if (run.length >= FOLD) items.push({ kind: "group", events: run });
    else items.push(...run.map(event => ({ kind: "event" as const, event })));
    run = [];
  };
  for (const event of activity.filter(e => !e.replyTo)) {
    if (isDiscussion(event)) { flush(); items.push({ kind: "card", event }); }
    else if (needsAttention(event)) { flush(); items.push({ kind: "event", event }); }
    else run.push(event);
  }
  flush();
  return items;
}

export function summary(events: IssueActivity[], pullRequest: (url: string) => string) {
  const parts: string[] = [];
  const updates = events.filter(e => ["updated", "created", "closed"].includes(e.kind)).length;
  if (updates) parts.push(`${updates} issue ${updates === 1 ? "update" : "updates"}`);
  const threads = events.filter(e => e.kind === "thread" && (e.changes?.some(c => c.field === "thread" && c.after) || (!e.changes?.length && !e.text.startsWith("Unlinked")))).length;
  if (threads) parts.push(`${threads} ${threads === 1 ? "thread" : "threads"} linked`);
  for (const change of events.flatMap(e => e.changes ?? []))
    if (change.field === "pullRequest" && change.after) parts.push(`linked ${pullRequest(change.after)}`);
  const status = statusOf([...events].reverse().flatMap(e => [...(e.changes ?? [])].reverse()).find(c => c.field === "status")?.after);
  if (status) parts.push(`now ${statusLabels[status]}`);
  if (!parts.length) parts.push(`${events.length} updates`);
  return parts.join(", ");
}

export type Target = { kind: "thread" | "pullRequest"; value: string; label: string };
export interface Phrase { action: string; target?: Target; tail?: string; status?: IssueStatus }

/** undefined for entries saved before typed changes: they keep their text. */
export function phrase(e: IssueActivity, name: (id: string) => string, pullRequest: (url: string) => string): Phrase | undefined {
  if (e.kind === "created") return { action: "created the issue" };
  if (!e.changes?.length) return undefined;
  const actions: string[] = [];
  let target: Target | undefined;
  let status: IssueStatus | undefined;
  // Linking a thread that also takes the issue reads as one act.
  const linked = e.changes.find(c => c.field === "thread")?.after;
  for (const c of e.changes) {
    if (c.field === "owner" && c.after && c.after === linked) continue;
    if (c.field === "status") {
      status = statusOf(c.after);
      actions.push(`moved the issue to ${status ? statusLabels[status] : "an unknown status"}`);
    } else if (c.field === "priority") actions.push(`set priority to ${priorityLabels[c.after ?? ""] ?? "unknown"}`);
    else if (c.field === "owner")
      actions.push(c.after ? (c.before ? `assigned ${name(c.after)} and unassigned ${name(c.before)}` : `assigned ${name(c.after)}`) : "removed the owner");
    else if (c.field === "thread" || c.field === "pullRequest") {
      const value = c.after ?? c.before;
      if (!value || target) continue;
      actions.unshift(c.after ? "linked" : "unlinked");
      target = { kind: c.field, value, label: c.field === "thread" ? name(value) : pullRequest(value) };
    } else if (c.field === "title") actions.push("renamed the issue to “" + (c.after ?? "") + "”");
    else if (c.field === "dueDate") actions.push(c.after ? "set the due date to " + c.after : "cleared the due date");
    else if (c.field === "labels") actions.push(c.after ? "set labels to " + c.after : "cleared the labels");
    else if (c.field === "description") actions.push("edited the description");
    else if (c.field === "doneCriteria") actions.push("edited what done means");
    else if (c.field === "outOfScope") actions.push("edited what's out of scope");
    else actions.push("updated the details");
  }
  const unique = actions.filter((a, i) => a !== actions[i - 1]);
  if (target && unique.length) return { action: unique[0]!, target, tail: unique.slice(1).map(a => "and " + a).join(" "), status };
  return { action: unique.join(" and "), target, status };
}

export type RunState = "running" | "awaitingInput" | "failed" | "unconfirmed";
export type Indicator = { kind: "working" | "needsInput" | "failed" | "unconfirmed" | "queued" | "replyQueued" | "replyRetrying"; name: string; position?: number };
export function indicatorText(i: Indicator) {
  switch (i.kind) {
    case "working": return `${i.name} is working`;
    case "needsInput": return `${i.name} needs input`;
    case "failed": return `${i.name} stopped with an error`;
    case "unconfirmed": return `Can't confirm ${i.name} is running`;
    case "queued": return i.position ? `${i.name} is queued (#${i.position})` : `${i.name} is queued`;
    case "replyQueued": return `Your reply is queued for ${i.name}`;
    case "replyRetrying": return `Retrying delivery to ${i.name}`;
  }
}

/** What the issue's accountable thread (else the Zapper) is doing, from confirmed run state only. */
export function indicator(zapper: Zapper, issue: ZapperIssue, run: (chat: string) => RunState | undefined, name: (id: string) => string): Indicator | undefined {
  if (issue.details.status === "done" || issue.details.status === "canceled") return undefined;
  const delivery = [...issue.activity].reverse().map(a => a.delivery).find(d => d && d.chatId !== zapper.coordinatorChatId);
  if (delivery?.state === "queued") return { kind: "replyQueued", name: name(delivery.chatId) };
  if (delivery?.state === "retrying") return { kind: "replyRetrying", name: name(delivery.chatId) };
  const owner = issue.details.owner ?? (issue.details.assigned ? { type: "zapper" as const } : undefined);
  if (owner?.type === "user") return undefined;
  const chat = owner?.type === "thread" ? owner.chatId : zapper.coordinatorChatId;
  const worker = zapper.workers.find(w => w.chatId === chat && w.queuePosition != null);
  if (worker) return { kind: "queued", name: name(chat), position: worker.queuePosition ?? undefined };
  const state = run(chat);
  if (!state) return undefined;
  const kind = ({ running: "working", awaitingInput: "needsInput", failed: "failed", unconfirmed: "unconfirmed" } as const)[state];
  return { kind, name: name(chat) };
}

/** Only a fresh session report confirms a run (the native host uses the same 45 s window). */
export function runState(session: { status?: string; updatedAt?: unknown } | undefined, now = Date.now()): RunState | undefined {
  if (!session) return undefined;
  const updated = typeof session.updatedAt === "string" ? Date.parse(session.updatedAt) : typeof session.updatedAt === "number" ? session.updatedAt : NaN;
  const fresh = Number.isFinite(updated) && now - updated < 45_000;
  if (session.status === "working") return fresh ? "running" : "unconfirmed";
  if (!fresh) return undefined;
  if (session.status === "awaitingInput") return "awaitingInput";
  if (session.status === "error" || session.status === "errored") return "failed";
  return undefined;
}

export function deliveryNote(d: IssueDelivery, name: string): { text: string; attention: boolean } {
  const withDetail = (text: string) => (d.detail ? `${text} · ${d.detail}` : text);
  switch (d.state) {
    case "queued": return { text: withDetail(`Queued for ${name}`), attention: false };
    case "retrying": return { text: withDetail(`Couldn't reach ${name} yet`), attention: true };
    case "sent": return { text: withDetail(`Sent to ${name}`), attention: false };
    case "answered": return { text: `Answered by ${name}`, attention: false };
    default: return { text: withDetail(`Not sent to ${name}`), attention: true };
  }
}
