import type { Chat, SessionMessageEntry } from "@/core/protocol";
import type { Zapper } from "@/core/zapper-types";

export interface InlineReference {
  label: string;
  title: string;
  kind: "issue" | "thread" | "agent" | "pr";
  status?: string;
  target: string;
}

export const REFERENCE_LIMIT = 512;
export const REFERENCE_TEXT_LIMIT = 65_536;
export const REFERENCE_MATCH_LIMIT = 100;

export function conversationReferences(zapper: Zapper | undefined, chats: Record<string, Chat>, chatId: string, messages: readonly SessionMessageEntry[], currentPr?: { url: string; title: string } | null): InlineReference[] {
  const references: InlineReference[] = [];
  const add = (reference: InlineReference) => {
    if (references.length < REFERENCE_LIMIT && reference.label.length <= 160) references.push(reference);
  };
  const addPr = (pr: { url: string; title: string }) => {
    const match = /^https:\/\/github\.com\/([^/]+\/[^/]+)\/pull\/(\d+)\/?$/.exec(pr.url);
    if (!match) return;
    for (const label of [`${match[1]}#${match[2]}`, `#${match[2]}`]) add({ label, title: pr.title, kind: "pr", target: pr.url });
  };
  if (currentPr) addPr(currentPr);
  for (const issue of zapper?.issues ?? []) {
    if (references.length >= REFERENCE_LIMIT) break;
    add({ label: `ZAP-${issue.number}`, title: issue.details.title, kind: "issue", status: issue.details.status, target: issue.id });
    for (const pr of issue.pullRequests) {
      if (references.length >= REFERENCE_LIMIT) break;
      addPr(pr);
    }
  }
  const linked = new Set([chatId, ...(zapper?.workers.map(worker => worker.chatId) ?? []), ...(zapper?.issues.flatMap(issue => issue.threads) ?? [])]);
  for (const id of [chats[chatId]?.parentChatId, chats[chatId]?.threadId]) if (id) linked.add(id);
  for (const id of linked) {
    if (references.length >= REFERENCE_LIMIT) break;
    const chat = chats[id];
    if (chat) add({ label: id, title: chat.title || "Thread", kind: "thread", target: id });
  }
  for (const entry of messages) {
    if (references.length >= REFERENCE_LIMIT) break;
    for (const part of entry.parts) {
      if (part.kind !== "tool" || !part.subagentRef) continue;
      add({ label: part.subagentRef, title: "Subagent conversation", kind: "agent", target: part.subagentRef });
      if (references.length >= REFERENCE_LIMIT) break;
    }
  }
  return references.length === REFERENCE_LIMIT ? references.filter(reference => reference.kind !== "pr" || !/^#\d+$/.test(reference.label)) : references;
}

export function referenceIndex(references: readonly InlineReference[]): Map<string, InlineReference> {
  const index = new Map<string, InlineReference>();
  const ambiguous = new Set<string>();
  for (const reference of references.slice(0, REFERENCE_LIMIT)) {
    if (!reference.label || reference.label.length > 160 || ambiguous.has(reference.label)) continue;
    if (reference.kind === "pr" && !/^https:\/\/github\.com\/[^/]+\/[^/]+\/pull\/\d+\/?$/.test(reference.target)) continue;
    const previous = index.get(reference.label);
    if (previous && (previous.kind !== reference.kind || previous.target !== reference.target)) {
      index.delete(reference.label);
      ambiguous.add(reference.label);
    } else index.set(reference.label, reference);
  }
  return index;
}

export function referencePattern(index: Map<string, InlineReference>): RegExp | null {
  const labels = [...index.keys()].sort((a, b) => b.length - a.length);
  return labels.length ? new RegExp(`(?<![\\p{L}\\p{N}_/#-])(?:${labels.map(label => label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})(?![\\p{L}\\p{N}_/#-])`, "gu") : null;
}

export function referenceHtml(html: string, references: readonly InlineReference[], issuePresentation?: (status: string) => { status: string; label: string; svg: string } | undefined): string {
  const index = referenceIndex(references);
  const pattern = referencePattern(index);
  if (!pattern || html.length > REFERENCE_TEXT_LIMIT) return html;
  const template = document.createElement("template");
  template.innerHTML = html;
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (!node.parentElement?.closest("a, code, pre, kbd, samp")) nodes.push(node);
  }
  let count = 0;
  for (const node of nodes) {
    const text = node.data;
    const fragment = document.createDocumentFragment();
    let offset = 0;
    for (const match of text.matchAll(pattern)) {
      if (count >= REFERENCE_MATCH_LIMIT) break;
      const reference = index.get(match[0])!;
      const presentation = reference.kind === "issue" ? issuePresentation?.(reference.status ?? "") : undefined;
      fragment.append(text.slice(offset, match.index));
      const anchor = document.createElement("a");
      anchor.className = "md-reference";
      anchor.href = reference.kind === "pr" ? reference.target : `#reference-${encodeURIComponent(reference.target)}`;
      anchor.dataset.reference = reference.label;
      anchor.dataset.kind = reference.kind;
      if (reference.status) anchor.dataset.status = reference.status;
      anchor.title = reference.title;
      anchor.setAttribute("aria-label", `${reference.label}: ${reference.title}${presentation ? ` (${presentation.label})` : ""}`);
      if (reference.kind === "pr") {
        anchor.target = "_blank";
        anchor.rel = "noopener noreferrer";
      }
      const icon = document.createElement("span");
      icon.className = "md-reference-icon";
      icon.setAttribute("aria-hidden", "true");
      if (presentation) {
        icon.classList.add("issue-state-icon");
        icon.dataset.status = presentation.status;
        icon.innerHTML = presentation.svg;
      }
      const prefix = reference.label.match(/^[^/#-]{1,12}|^#[0-9]+/)?.[0] ?? reference.label.slice(0, 1);
      const lead = document.createElement("span");
      lead.className = "md-reference-lead";
      lead.append(icon, prefix);
      anchor.append(lead, reference.label.slice(prefix.length));
      fragment.append(anchor);
      offset = match.index + match[0].length;
      count++;
    }
    if (offset) {
      fragment.append(text.slice(offset));
      node.replaceWith(fragment);
    }
    if (count >= REFERENCE_MATCH_LIMIT) break;
  }
  return template.innerHTML;
}
