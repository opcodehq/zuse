// Pure helpers for the pull request overview: check ordering and counts, the one-line status, and review
// feedback grouped into open and resolved items, each with a prompt the chat agent can act on.
import { mergeReason } from "./timeline";
import type { ChangeRequestCheck, ChangeRequestComment, ChangeRequestDetail } from "./types";

export type CheckTone = "running" | "failed" | "passed" | "skipped";

const FAILED = /FAILURE|ERROR|TIMED_OUT|ACTION_REQUIRED|CANCELLED|STARTUP_FAILURE/;
const SETTLED = /SUCCESS|NEUTRAL/;
const ORDER: Record<CheckTone, number> = { running: 0, failed: 1, passed: 2, skipped: 3 };

export function checkStatus(check: ChangeRequestCheck) {
  return (check.conclusion || check.state || check.status || "").toUpperCase();
}

export function checkTone(check: ChangeRequestCheck): CheckTone {
  const status = checkStatus(check);
  if (FAILED.test(status)) return "failed";
  if (status === "SKIPPED") return "skipped";
  if (SETTLED.test(status)) return "passed";
  return "running";
}

/** Running checks first, then failed, then passed and skipped; ties keep GitHub's order. */
export function sortChecks(checks: readonly ChangeRequestCheck[]) {
  return checks
    .map((check, index) => ({ check, index, tone: checkTone(check) }))
    .sort((a, b) => ORDER[a.tone] - ORDER[b.tone] || a.index - b.index)
    .map((entry) => entry.check);
}

export function checkCounts(checks: readonly ChangeRequestCheck[]) {
  const counts: Record<CheckTone, number> = { running: 0, failed: 0, passed: 0, skipped: 0 };
  for (const check of checks) counts[checkTone(check)]++;
  return counts;
}

/** "2 running, 7 passed": the counts that are not zero, in the list's order. */
export function checksSummary(checks: readonly ChangeRequestCheck[]) {
  const counts = checkCounts(checks);
  return (Object.keys(ORDER) as CheckTone[]).filter((tone) => counts[tone]).map((tone) => `${counts[tone]} ${tone}`).join(", ");
}

/** The tone that leads the checks row: anything running, else anything failed, else passed. */
export function checksTone(checks: readonly ChangeRequestCheck[]): CheckTone {
  const counts = checkCounts(checks);
  return counts.running ? "running" : counts.failed ? "failed" : counts.passed ? "passed" : "skipped";
}

export interface StatusLine {
  state: "Open" | "Draft" | "Merged" | "Closed";
  tone: "success" | "muted" | "merged" | "danger";
  detail: string | null;
  /** Open, not a draft, mergeable, checks green, reviews satisfied: merge is the next step. */
  ready: boolean;
}

/** "Open · Waiting on checks", "Open · Ready to merge", "Merged". */
export function statusLine(detail: ChangeRequestDetail): StatusLine {
  const state = detail.state.toUpperCase();
  if (state === "MERGED") return { state: "Merged", tone: "merged", detail: null, ready: false };
  if (state === "CLOSED") return { state: "Closed", tone: "danger", detail: null, ready: false };
  if (detail.isDraft) return { state: "Draft", tone: "muted", detail: null, ready: false };
  const open = (text: string, ready = false): StatusLine => ({ state: "Open", tone: "success", detail: text, ready });
  const counts = checkCounts(detail.statusCheckRollup);
  const review = detail.reviewDecision.toUpperCase();
  if (detail.mergeable.toUpperCase() === "CONFLICTING") return open("Has conflicts");
  if (detail.autoMergeRequest) return open(`Auto-merge on (${detail.autoMergeRequest.mergeMethod.toLowerCase()})`);
  if (counts.failed) return open("Checks failing");
  if (counts.running) return open("Waiting on checks");
  if (review === "CHANGES_REQUESTED") return open("Changes requested");
  if (review === "REVIEW_REQUIRED") return open("Review required");
  const reason = mergeReason(detail);
  if (reason) return open(reason);
  return open("Ready to merge", true);
}

export interface FeedbackItem {
  id: string;
  kind: "thread" | "review";
  author: string;
  createdAt: string;
  url: string;
  body: string;
  path: string | null;
  line: number | null;
  diffHunk: string;
  severity: string | null;
  title: string;
  /** The reviewer's own prompt for an AI agent, when the comment carries one. */
  aiPrompt: string | null;
  replies: number;
  resolved: boolean;
  outdated: boolean;
}

const SEVERITY = /(?<![a-z])(critical|major|minor|nitpick|nit|trivial|high|medium|low|info|suggestion|warning|error)(?![a-z])/i;
const PROMPT_BLOCK = /<details>\s*<summary>([\s\S]*?)<\/summary>([\s\S]*?)<\/details>/gi;

const plain = (line: string) =>
  line
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_`~]+/g, "")
    .replace(/^[#>\s-]+/, "")
    .replace(/\s+/g, " ")
    .trim();

/** The prompt-for-AI block review bots attach (CodeRabbit's "Prompt for AI Agents", "Prompt to fix"). */
export function aiPrompt(body: string): string | null {
  for (const match of body.matchAll(PROMPT_BLOCK)) {
    if (!/prompt/i.test(match[1] ?? "")) continue;
    const inner = match[2] ?? "";
    const text = (/```[^\n]*\n([\s\S]*?)```/.exec(inner)?.[1] ?? inner).trim();
    if (text) return text;
  }
  return null;
}

/** The body as readable markdown: markup comments and the AI prompt block go, other disclosures open up. */
export function cleanBody(body: string) {
  return body
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(PROMPT_BLOCK, (all, summary: string) => (/prompt/i.test(summary) ? "" : all))
    .replace(/<summary>([\s\S]*?)<\/summary>/gi, (_, summary: string) => `\n**${plain(summary)}**\n\n`)
    .replace(/<\/?details>/gi, "")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** Severity and a one-line title: the first bold line, else the first line that is not only a severity tag. */
export function feedbackSummary(body: string): { severity: string | null; title: string } {
  const lines = cleanBody(body).split("\n").map((line) => line.trim()).filter(Boolean);
  const found = SEVERITY.exec(lines.slice(0, 3).join(" "))?.[1] ?? null;
  const severity = found ? found[0]!.toUpperCase() + found.slice(1).toLowerCase() : null;
  const bold = lines.find((line) => /^\*\*[^*]+\*\*/.test(line));
  const tagOnly = (line: string) => SEVERITY.test(line) && plain(line).split(/[\s|·]+/).filter(Boolean).length <= 6 && !/[.:]$/.test(plain(line));
  const title = plain(bold ? /^\*\*([^*]+)\*\*/.exec(bold)![1]! : lines.find((line) => !tagOnly(line)) ?? lines[0] ?? "");
  return { severity, title: title.length > 160 ? `${title.slice(0, 159).trimEnd()}…` : title };
}

const item = (comment: ChangeRequestComment, kind: FeedbackItem["kind"], extra: Partial<FeedbackItem> = {}): FeedbackItem => ({
  id: comment.id,
  kind,
  author: comment.author.login || "Someone",
  createdAt: comment.submittedAt || comment.createdAt,
  url: comment.url,
  body: comment.body,
  path: null,
  line: null,
  diffHunk: comment.diffHunk,
  aiPrompt: aiPrompt(comment.body),
  replies: 0,
  resolved: false,
  outdated: false,
  ...feedbackSummary(comment.body),
  ...extra,
});

/**
 * Actionable review feedback, oldest first: inline review threads and review bodies that request changes.
 * Conversation comments (deploy previews, CI and coverage summaries, review-bot walkthroughs) stay in Activity.
 * Open items need action; resolved and outdated threads sit apart.
 */
export function groupFeedback(detail: ChangeRequestDetail) {
  const all: FeedbackItem[] = [
    ...(detail.reviewThreads ?? []).flatMap((thread) => {
      const root = thread.comments[0];
      if (!root) return [];
      return [item(root, "thread", { id: thread.id || root.id, path: thread.path || null, line: thread.line ?? null, replies: thread.comments.length - 1, resolved: thread.isResolved, outdated: thread.isOutdated })];
    }),
    ...detail.reviews.filter((review) => review.body.trim() && review.state.toUpperCase() === "CHANGES_REQUESTED").map((review) => item(review, "review")),
  ].sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt));
  return { open: all.filter((entry) => !entry.resolved && !entry.outdated), closed: all.filter((entry) => entry.resolved || entry.outdated) };
}

const where = (entry: FeedbackItem) => (entry.path ? (entry.line ? `${entry.path}:${entry.line}` : entry.path) : null);

/** The text that asks the agent to fix one item: the bot's own prompt when it has one, else file, line and comment. */
export function fixPrompt(entry: FeedbackItem) {
  const location = where(entry);
  if (entry.aiPrompt) return location && !entry.aiPrompt.includes(entry.path!) ? `In ${location}: ${entry.aiPrompt}` : entry.aiPrompt;
  const source = location ? `review comment from ${entry.author} on ${location}` : `review from ${entry.author}`;
  return `Address this ${source}:\n\n${cleanBody(entry.body)}`;
}

/** Every open item as one numbered prompt. */
export function fixAllPrompt(entries: readonly FeedbackItem[], pr: { number: number; title: string }) {
  if (entries.length === 1) return fixPrompt(entries[0]!);
  const body = entries.map((entry, index) => `${index + 1}. ${fixPrompt(entry).replace(/\n/g, "\n   ")}`).join("\n\n");
  return `Fix these ${entries.length} review comments on #${pr.number} (${pr.title}):\n\n${body}`;
}


/** Removes each whole paragraph-bounded occurrence of `block` from `text`. */
function withoutBlock(text: string, block: string) {
  let out = text;
  for (let at = out.indexOf(block); at >= 0; at = out.indexOf(block, at)) {
    const end = at + block.length;
    const starts = at === 0 || out.slice(Math.max(0, at - 2), at) === "\n\n";
    const ends = end === out.length || out.slice(end, end + 2) === "\n\n";
    if (starts && ends) out = out.slice(0, at) + out.slice(end);
    else at = end;
  }
  return out;
}

/**
 * The draft with `prompt` added as its last paragraph. An earlier add of the same prompt, or of any prompt in
 * `replaces` (the single items an "Add all" covers), is taken out first so nothing appears twice.
 */
export function addPrompt(draft: string, prompt: string, replaces: readonly string[] = []) {
  let rest = draft;
  for (const block of new Set([prompt, ...replaces])) rest = withoutBlock(rest, block);
  rest = rest.replace(/\n{3,}/g, "\n\n").trim();
  return rest ? `${rest}\n\n${prompt}` : prompt;
}
