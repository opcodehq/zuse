import type { Zapper } from "@/core/zapper-types";

export type ReferenceKind = "issue" | "pr" | "thread" | "agent" | "file" | "commit" | "branch" | "attachment" | "zapper";

export interface InlineReference {
  kind: ReferenceKind;
  label: string;
  title: string;
  target: string;
  status?: string;
  text?: string;
  icon?: string;
  line?: number;
}

export interface ReferenceSources {
  issues: readonly { id: string; number: number; title: string; status: string }[];
  pullRequests: readonly { url: string; title: string; state?: string; draft?: boolean }[];
  threads: readonly { id: string; title?: string | null }[];
  agents: readonly { ref: string; title: string; status: string }[];
  files?: readonly { path: string; directory?: boolean; icon?: string }[];
  branches?: readonly { name: string; chatId: string }[];
  attachments?: readonly { name: string; path: string }[];
  zappers?: readonly { id: string; name: string; status: string }[];
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
  const basenames = new Map<string, { path: string; directory?: boolean; icon?: string } | null>();
  for (const file of sources.files ?? []) {
    const path = file.path.replace(/^\.\//, "").replace(/\/+$/, "");
    if (!path || path.startsWith("/")) continue;
    add({ kind: "file", label: file.directory && !path.includes("/") ? `${path}/` : path, title: path, target: path, icon: file.icon, status: file.directory ? "directory" : undefined });
    const base = path.slice(path.lastIndexOf("/") + 1);
    const seen = basenames.get(base);
    if (base !== path && /\.\w/.test(base)) basenames.set(base, seen === undefined || seen?.path === path ? { ...file, path } : null);
  }
  for (const [base, file] of basenames) if (file && !index.has(base)) add({ kind: "file", label: base, title: file.path, target: file.path, icon: file.icon, status: file.directory ? "directory" : undefined });
  for (const branch of sources.branches ?? []) if (branch.name.length >= 3 && /[/\-_.\d]/.test(branch.name)) add({ kind: "branch", label: branch.name, title: `Branch ${branch.name}`, target: branch.chatId });
  for (const attachment of sources.attachments ?? []) if (/\.\w{2,5}$/.test(attachment.name)) add({ kind: "attachment", label: attachment.name, title: attachment.name, target: attachment.path });
  for (const zapper of sources.zappers ?? []) if (zapper.name.trim().length >= 3) add({ kind: "zapper", label: zapper.name.trim(), title: `Zapper ${zapper.name.trim()}`, target: zapper.id, status: zapper.status });
  const labels = [...index.keys()].sort((a, b) => b.length - a.length).map(label => label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const pattern = labels.length ? new RegExp(`(?<![\\p{L}\\p{N}_/#.-])(?:${labels.join("|")})(?![\\p{L}\\p{N}_/-]|#(?!L\\d)|\\.[\\p{L}\\p{N}])`, "gu") : null;
  return { index, pattern };
}

const LINE = /^(?::(\d{1,7})(?::\d{1,5})?|#L(\d{1,7}))(?![\p{L}\p{N}_])/u;

export function findReferences(text: string, catalog: ReferenceCatalog, limit = MATCH_LIMIT): ReferenceMatch[] {
  if (!catalog.pattern || limit <= 0) return [];
  const matches: ReferenceMatch[] = [];
  let consumed = 0;
  for (const match of text.matchAll(catalog.pattern)) {
    if (match.index < consumed) continue;
    const reference = catalog.index.get(match[0])!;
    let end = match.index + match[0].length;
    const line = reference.kind === "file" ? LINE.exec(text.slice(end)) : null;
    if (line) end += line[0].length;
    consumed = end;
    matches.push({ start: match.index, end, reference: line ? { ...reference, line: Number(line[1] ?? line[2]) } : reference });
    if (matches.length >= limit) break;
  }
  return matches;
}

const COMMIT = /(?<![\p{L}\p{N}_/#.@-])(?:([\w.-]+\/[\w.-]+)@)?([0-9a-f]{7,40})(?![\p{L}\p{N}_/#-]|\.[\p{L}\p{N}])/gu;
const COMMIT_URL = /^https:\/\/github\.com\/([\w.-]+\/[\w.-]+)\/commit\/([0-9a-f]{7,40})\/?$/;

export function parseCommitUrl(url: string) {
  const match = COMMIT_URL.exec(url);
  return match ? { repo: match[1]!, sha: match[2]! } : null;
}

export function commitReference(repo: string, sha: string): InlineReference {
  return { kind: "commit", label: `${repo}@${sha}`, title: `Commit ${sha.slice(0, 12)} in ${repo}`, target: `https://github.com/${repo}/commit/${sha}`, text: sha.slice(0, 7) };
}

/** Commit hashes count only with a repository: written as owner/repo@sha, or bare beside exactly one repository's pull request. */
export function findCommits(text: string, contextRepo: string | null, limit = MATCH_LIMIT): ReferenceMatch[] {
  const matches: ReferenceMatch[] = [];
  if (limit <= 0) return matches;
  for (const match of text.matchAll(COMMIT)) {
    const sha = match[2]!;
    if (!/\d/.test(sha) || !/[a-f]/.test(sha)) continue;
    const repo = match[1] ?? contextRepo;
    if (!repo) continue;
    const reference = commitReference(repo, sha);
    matches.push({ start: match.index, end: match.index + match[0].length, reference: match[1] ? { ...reference, text: `${match[1].split("/")[1]}@${sha.slice(0, 7)}` } : reference });
    if (matches.length >= limit) break;
  }
  return matches;
}

export function decodeFileHref(href: string) {
  const raw = href.startsWith("zuse-file:") ? href.slice(10) : null;
  if (raw === null) return null;
  try {
    return decodeURIComponent(raw);
  } catch {
    return raw;
  }
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
