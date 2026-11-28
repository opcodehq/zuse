// The pull request's conversation as compact one-line rows, the way GitHub's timeline reads: consecutive
// commits fold into one "pushed N commits" row, and runs of minor events fold under a summary.
import type { ChangeRequestDetail, TimelineItem, TimelineSubject } from "./types";

export type TimelinePart =
  | string
  | { chip: string; kind: "sha" | "label" | "ref" | "user" | "subject"; href?: string; color?: string; title?: string };

export interface TimelineEvent {
  type: "event";
  id: string;
  item: TimelineItem;
  parts: TimelinePart[];
  /** Comments and reviews carry a body shown under the row. */
  body: string;
}
export interface TimelineCommits {
  type: "commits";
  id: string;
  actor: TimelineItem["actor"];
  createdAt: string;
  commits: TimelineItem[];
  /** The check result of the push's last commit. */
  checks: "passed" | "failed" | "running" | null;
}
export interface TimelineGroup {
  type: "group";
  id: string;
  summary: string;
  rows: TimelineEvent[];
}
export type TimelineRow = TimelineEvent | TimelineCommits | TimelineGroup;

const MINOR = new Set(["labeled", "unlabeled", "reviewRequested", "reviewRequestRemoved", "autoMergeEnabled", "autoMergeDisabled", "crossReferenced", "connected", "disconnected"]);
/** Runs of this many minor events or more fold under a summary. */
export const GROUP_AFTER = 3;

const short = (oid?: string | null) => (oid ?? "").slice(0, 7);
const subject = (s: TimelineSubject): TimelinePart => ({ chip: `#${s.number}`, kind: "subject", href: s.url, title: s.title });

export function eventParts(item: TimelineItem, base: string): TimelinePart[] {
  switch (item.kind) {
    case "opened":
      return ["opened this pull request"];
    case "forcePush":
      return ["force-pushed", { chip: short(item.from), kind: "sha" }, "to", { chip: short(item.to), kind: "sha" }];
    case "review": {
      const state = (item.state ?? "").toUpperCase();
      return [state === "APPROVED" ? "approved these changes" : state === "CHANGES_REQUESTED" ? "requested changes" : state === "DISMISSED" ? "had a review dismissed" : "reviewed"];
    }
    case "comment":
      return ["commented"];
    case "reviewRequested":
      return ["requested a review from", { chip: item.name ?? "someone", kind: "user" }];
    case "reviewRequestRemoved":
      return ["removed the review request for", { chip: item.name ?? "someone", kind: "user" }];
    case "labeled":
      return ["added the", { chip: item.name ?? "", kind: "label", color: item.color ?? undefined }, "label"];
    case "unlabeled":
      return ["removed the", { chip: item.name ?? "", kind: "label", color: item.color ?? undefined }, "label"];
    case "readyForReview":
      return ["marked this ready for review"];
    case "convertedToDraft":
      return ["converted this to a draft"];
    case "baseRefChanged":
      return ["changed the base branch from", { chip: item.from ?? "", kind: "ref" }, "to", { chip: item.to ?? "", kind: "ref" }];
    case "autoMergeEnabled":
      return ["enabled auto-merge"];
    case "autoMergeDisabled":
      return ["disabled auto-merge"];
    case "merged":
      return item.to ? ["merged commit", { chip: short(item.to), kind: "sha" }, "into", { chip: base, kind: "ref" }] : ["merged this into", { chip: base, kind: "ref" }];
    case "closed":
      return ["closed this"];
    case "reopened":
      return ["reopened this"];
    case "crossReferenced":
      return item.subject ? ["mentioned this in", subject(item.subject)] : ["mentioned this"];
    case "connected":
      return item.subject ? ["linked", subject(item.subject)] : ["linked an issue"];
    case "disconnected":
      return item.subject ? ["unlinked", subject(item.subject)] : ["unlinked an issue"];
    default:
      return [item.kind];
  }
}

const NOUNS: Record<string, [string, string]> = {
  labeled: ["label change", "label changes"],
  unlabeled: ["label change", "label changes"],
  reviewRequested: ["review request", "review requests"],
  reviewRequestRemoved: ["review request", "review requests"],
  autoMergeEnabled: ["auto-merge change", "auto-merge changes"],
  autoMergeDisabled: ["auto-merge change", "auto-merge changes"],
  crossReferenced: ["mention", "mentions"],
  connected: ["linked issue", "linked issues"],
  disconnected: ["linked issue", "linked issues"],
};

/** "5 events: 3 label changes, 2 review requests". */
export function groupSummary(rows: readonly { item: TimelineItem }[]) {
  const counts = new Map<string, number>();
  for (const { item } of rows) {
    const [one] = NOUNS[item.kind] ?? [item.kind];
    counts.set(one, (counts.get(one) ?? 0) + 1);
  }
  const nouns = Object.fromEntries(Object.values(NOUNS));
  const parts = [...counts].map(([one, n]) => `${n} ${n === 1 ? one : nouns[one] ?? `${one}s`}`);
  return `${rows.length} events: ${parts.join(", ")}`;
}

const pushChecks = (commit: TimelineItem): TimelineCommits["checks"] => {
  const state = (commit.state ?? "").toUpperCase();
  if (state === "SUCCESS") return "passed";
  if (state === "FAILURE" || state === "ERROR") return "failed";
  if (state === "PENDING" || state === "EXPECTED") return "running";
  return null;
};

/** Null when the engine sent no timeline (an older engine), so the caller can fall back to the plain list. */
export function buildTimeline(detail: ChangeRequestDetail): TimelineRow[] | null {
  if (!detail.timeline) return null;
  const items: TimelineItem[] = [
    ...(detail.createdAt ? [{ kind: "opened", actor: detail.author, createdAt: detail.createdAt }] : []),
    ...detail.timeline,
  ].map((item, index) => ({ item, index }))
    .sort((a, b) => Date.parse(a.item.createdAt) - Date.parse(b.item.createdAt) || a.index - b.index)
    .map((entry) => entry.item);
  // gh's comment and review lists don't always carry the timeline's URLs, so author and time match too.
  const sources = [...detail.comments.map((c) => ({ c, kind: "comment" })), ...detail.reviews.map((c) => ({ c, kind: "review" }))];
  const bodyFor = (item: TimelineItem) =>
    sources.find(({ c }) => item.url && c.url === item.url)?.c.body ??
    sources.find(({ c, kind }) => kind === item.kind && c.author.login === item.actor?.login && Date.parse(c.submittedAt || c.createdAt) === Date.parse(item.createdAt))?.c.body ??
    "";
  const rows: TimelineRow[] = [];
  let minor: TimelineEvent[] = [];
  const flush = () => {
    if (minor.length >= GROUP_AFTER) rows.push({ type: "group", id: `group:${minor[0]!.id}`, summary: groupSummary(minor), rows: minor });
    else rows.push(...minor);
    minor = [];
  };
  items.forEach((item, index) => {
    const id = `${item.kind}:${item.createdAt}:${index}`;
    if (item.kind === "commit") {
      flush();
      const last = rows.at(-1);
      if (last?.type === "commits") {
        last.commits.push(item);
        last.checks = pushChecks(item);
      } else rows.push({ type: "commits", id, actor: item.actor ?? null, createdAt: item.createdAt, commits: [item], checks: pushChecks(item) });
      return;
    }
    const event: TimelineEvent = { type: "event", id, item, parts: eventParts(item, detail.baseRefName), body: item.kind === "comment" || item.kind === "review" ? bodyFor(item) : "" };
    if (MINOR.has(item.kind)) return void minor.push(event);
    flush();
    rows.push(event);
  });
  flush();
  return rows;
}

/** "Behind main", "Blocked by branch rules": GitHub's mergeability reason when it adds something. */
export function mergeReason(detail: Pick<ChangeRequestDetail, "mergeStateStatus" | "baseRefName">) {
  switch ((detail.mergeStateStatus ?? "").toUpperCase()) {
    case "BEHIND":
      return `Behind ${detail.baseRefName}`;
    case "BLOCKED":
      return "Blocked by branch rules";
    case "DIRTY":
      return "Has conflicts";
    default:
      return null;
  }
}
