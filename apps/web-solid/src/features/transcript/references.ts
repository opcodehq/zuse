import type { Zapper } from "@/core/zapper-types";

export type ReferenceKind = "issue" | "pr" | "thread" | "agent";

export interface InlineReference {
  kind: ReferenceKind;
  label: string;
  title: string;
  target: string;
  status?: string;
  text?: string;
}

export interface ReferenceSources {
  issues: readonly { id: string; number: number; title: string; status: string }[];
  pullRequests: readonly { url: string; title: string; state?: string; draft?: boolean }[];
  threads: readonly { id: string; title?: string | null }[];
  agents: readonly { ref: string; title: string; status: string }[];
}

export interface ReferenceCatalog {
  index: Map<string, InlineReference>;
  pattern: RegExp | null;
}

export interface ReferenceMatch {
  start: number;
  end: number;
  reference: InlineReference;
}

export interface OpenAsk {
  key: string;
  text: string;
  issueId?: string;
}

export const REFERENCE_LIMIT = 1024;
export const MATCH_LIMIT = 200;
const LABEL_LIMIT = 160;
const PR_URL = /^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/(\d+)\/?$/;

export function parsePrUrl(url: string) {
  const match = PR_URL.exec(url);
  return match ? { owner: match[1]!, repo: match[2]!, number: Number(match[3]) } : null;
}

export function issueLabel(number: number) {
  return `ZAP-${number}`;
}

export function referenceCatalog(sources: ReferenceSources): ReferenceCatalog {
  const index = new Map<string, InlineReference>();
  const ambiguous = new Set<string>();
  const add = (reference: InlineReference) => {
    const { label } = reference;
    if (!label || label.length > LABEL_LIMIT || ambiguous.has(label)) return;
    const previous = index.get(label);
    if (previous && (previous.kind !== reference.kind || previous.target !== reference.target)) {
      index.delete(label);
      ambiguous.add(label);
    } else if (!previous && index.size < REFERENCE_LIMIT) index.set(label, reference);
  };
  for (const issue of sources.issues) add({ kind: "issue", label: issueLabel(issue.number), title: issue.title, target: issue.id, status: issue.status });
  const prs = new Map<string, { url: string; title: string; state?: string; draft?: boolean }>();
  for (const pr of sources.pullRequests) {
    const url = pr.url.replace(/\/$/, "");
    if (!parsePrUrl(url)) continue;
    const known = prs.get(url);
    prs.set(url, { url, title: known?.title || pr.title, state: pr.state ?? known?.state, draft: pr.draft ?? known?.draft });
  }
  for (const pr of prs.values()) {
    const { owner, repo, number } = parsePrUrl(pr.url)!;
    const status = pr.state ? (pr.draft && pr.state === "open" ? "draft" : pr.state) : undefined;
    for (const label of [`${owner}/${repo}#${number}`, `${repo}#${number}`, `#${number}`]) add({ kind: "pr", label, title: pr.title, target: pr.url, status });
  }
  for (const thread of sources.threads) add({ kind: "thread", label: thread.id, title: thread.title || "Conversation", target: thread.id, text: thread.title || undefined });
  for (const agent of sources.agents) add({ kind: "agent", label: agent.ref, title: agent.title, target: agent.ref, status: agent.status, text: agent.title });
  const labels = [...index.keys()].sort((a, b) => b.length - a.length).map(label => label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const pattern = labels.length ? new RegExp(`(?<![\\p{L}\\p{N}_/#.-])(?:${labels.join("|")})(?![\\p{L}\\p{N}_/#-]|\\.[\\p{L}\\p{N}])`, "gu") : null;
  return { index, pattern };
}

export function findReferences(text: string, catalog: ReferenceCatalog, limit = MATCH_LIMIT): ReferenceMatch[] {
  if (!catalog.pattern || limit <= 0) return [];
  const matches: ReferenceMatch[] = [];
  for (const match of text.matchAll(catalog.pattern)) {
    matches.push({ start: match.index, end: match.index + match[0].length, reference: catalog.index.get(match[0])! });
    if (matches.length >= limit) break;
  }
  return matches;
}

export function openAsks(zapper: Zapper | undefined, chatId: string): OpenAsk[] {
  if (!zapper) return [];
  const coordinator = zapper.coordinatorChatId === chatId || zapper.previousCoordinatorChats.includes(chatId);
  const asks: OpenAsk[] = [];
  for (const issue of zapper.issues) {
    if (issue.details.status !== "needsYou" || (issue.details.owner && issue.details.owner.type !== "user")) continue;
    if (!coordinator && !issue.threads.includes(chatId)) continue;
    const question = [...issue.activity].reverse().find(event => event.kind === "question");
    asks.push({ key: question ? `question:${question.id}` : `issue:${issue.id}:${issue.updatedAt}`, text: question?.text.trim() || issue.details.title, issueId: issue.id });
  }
  for (const decision of zapper.decisions) {
    if (decision.answer !== null || (!coordinator && decision.workerChatId !== chatId)) continue;
    asks.push({ key: `decision:${decision.id}`, text: decision.question.trim() });
  }
  return asks;
}
