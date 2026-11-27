import type { ToolCall } from "@/core/protocol";
import { state } from "@/core/store";
import { openChat, setView } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { openSubagent } from "@/features/workspace/agents";
import { openPanel, workspace } from "@/features/workspace/store";
import { showPage } from "@/features/zappers/panel";
import { setInspectorOpen } from "@/features/zappers/state";
import { Icon, iconSvg } from "@/ui/Icon";
import { type Accessor, createMemo, createSignal, For } from "solid-js";
import { Markdown } from "./Markdown";
import { agentStatus } from "./model";
import { findReferences, type InlineReference, MATCH_LIMIT, openAsks, parsePrUrl, type ReferenceCatalog, referenceCatalog } from "./references";
import "./references.css";

const HTML_LIMIT = 200_000;

function ownerZapper(chatId: string) {
  const direct = zapperForChat(chatId);
  if (direct) return direct;
  const linked = state.zappers.filter(zapper => zapper.issues.some(issue => issue.threads.includes(chatId)));
  return linked.length === 1 ? linked[0] : undefined;
}

const signatures = new WeakMap<ReferenceCatalog, string>();
function signature(catalog: ReferenceCatalog) {
  let value = signatures.get(catalog);
  if (value === undefined) {
    value = [...catalog.index.values()].map(ref => [ref.kind, ref.label, ref.target, ref.title, ref.status, ref.text].join("\u0001")).join("\u0002");
    signatures.set(catalog, value);
  }
  return value;
}

const agentTitle = (call: ToolCall) => call.kind === "unknown" && call.name.startsWith("Agent: ") ? call.name.slice(7) : "Subagent";

export function chatReferences(source: { chatId: string }): Accessor<ReferenceCatalog> {
  return createMemo(() => {
    const id = source.chatId;
    const owner = ownerZapper(id);
    const messages = state.transcript?.chatId === id ? state.transcript.messages : [];
    return referenceCatalog({
      issues: (owner?.issues ?? []).map(issue => ({ id: issue.id, number: issue.number, title: issue.details.title, status: issue.details.status })),
      pullRequests: [
        ...Object.values(workspace.changeRequests).flatMap(status => status?.changeRequest ? [{ url: status.changeRequest.url, title: status.changeRequest.title, state: status.changeRequest.state, draft: status.changeRequest.isDraft }] : []),
        ...(owner?.issues ?? []).flatMap(issue => issue.pullRequests),
      ],
      threads: Object.values(state.chats).map(chat => ({ id: chat.id, title: chat.title })),
      agents: messages.flatMap(entry => entry.parts.flatMap(part => part.kind === "tool" && part.subagentRef ? [{ ref: part.subagentRef, title: agentTitle(part.call), status: agentStatus(part) }] : [])),
    });
  }, { equals: (a, b) => signature(a) === signature(b) });
}

const ISSUE_ICONS: Record<string, [string, string]> = {
  needsYou: ["alert-circle", "danger"],
  blocked: ["danger-triangle", "warning"],
  inProgress: ["clock-circle", "accent"],
  inReview: ["clock-circle", "accent"],
  done: ["check-circle", "success"],
  canceled: ["close-circle", "muted"],
  todo: ["clock-circle", "muted"],
  backlog: ["clock-circle", "muted"],
};
const PR_ICONS: Record<string, [string, string]> = {
  open: ["git-state-open", "success"],
  draft: ["git-state-draft", "muted"],
  merged: ["git-state-merged", "merged"],
  closed: ["git-state-closed", "danger"],
};
const AGENT_TONES: Record<string, string> = { running: "accent", done: "success", failed: "danger" };
const STATUS_LABELS: Record<string, string> = {
  needsYou: "Needs you", blocked: "Blocked", inProgress: "In progress", inReview: "In review", done: "Done", canceled: "Canceled", todo: "Todo", backlog: "Backlog",
  open: "Open", draft: "Draft", merged: "Merged", closed: "Closed", running: "Running", failed: "Failed",
};

function presentation(reference: InlineReference): [string, string | undefined] {
  const status = reference.status ?? "";
  if (reference.kind === "issue") return ISSUE_ICONS[status] ?? ["clock-circle", "muted"];
  if (reference.kind === "pr") return PR_ICONS[status] ?? ["git-lane-pr", undefined];
  if (reference.kind === "agent") return ["tool-agent", AGENT_TONES[status]];
  return ["chat-round-line", undefined];
}

function chip(reference: InlineReference, text: string) {
  const [icon, tone] = presentation(reference);
  const anchor = document.createElement("a");
  anchor.className = "md-ref";
  anchor.href = reference.kind === "pr" ? reference.target : "#";
  anchor.dataset.refKind = reference.kind;
  anchor.dataset.ref = reference.label;
  if (reference.status) anchor.dataset.status = reference.status;
  if (tone) anchor.dataset.tone = tone;
  const status = reference.status ? STATUS_LABELS[reference.status] ?? reference.status : undefined;
  anchor.title = status ? `${reference.title} · ${status}` : reference.title;
  anchor.setAttribute("aria-label", `${text}: ${anchor.title}`);
  if (reference.kind === "pr") anchor.rel = "noopener noreferrer";
  const glyph = document.createElement("span");
  glyph.className = "md-ref-icon";
  glyph.setAttribute("aria-hidden", "true");
  glyph.innerHTML = iconSvg(icon).trim().replace(/>\s+</g, "><");
  const lead = document.createElement("span");
  lead.className = "md-ref-lead";
  const first = Array.from(text)[0] ?? "";
  lead.append(glyph, first);
  anchor.append(lead, text.slice(first.length));
  return anchor;
}

export function decorateReferences(html: string, catalog: ReferenceCatalog): string {
  const linksPr = html.includes("github.com/");
  if ((!catalog.pattern && !linksPr) || html.length > HTML_LIMIT) return html;
  const template = document.createElement("template");
  template.innerHTML = html;
  let count = 0;
  if (linksPr) {
    for (const anchor of template.content.querySelectorAll("a[href]")) {
      const href = anchor.getAttribute("href") ?? "";
      const pr = parsePrUrl(href);
      if (!pr || anchor.closest("code, pre") || anchor.textContent?.trim() !== href) continue;
      const known = catalog.index.get(`${pr.owner}/${pr.repo}#${pr.number}`);
      const reference = known ?? { kind: "pr", label: `${pr.owner}/${pr.repo}#${pr.number}`, title: href, target: href.replace(/\/$/, "") };
      anchor.replaceWith(chip(reference, `${pr.repo}#${pr.number}`));
      count++;
    }
  }
  if (catalog.pattern) {
    const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_TEXT);
    const nodes: Text[] = [];
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (!node.parentElement?.closest("a, code, pre, kbd, samp")) nodes.push(node);
    }
    for (const node of nodes) {
      if (count >= MATCH_LIMIT) break;
      const matches = findReferences(node.data, catalog, MATCH_LIMIT - count);
      if (!matches.length) continue;
      const fragment = document.createDocumentFragment();
      let offset = 0;
      for (const match of matches) {
        fragment.append(node.data.slice(offset, match.start), chip(match.reference, match.reference.text ?? match.reference.label));
        offset = match.end;
      }
      fragment.append(node.data.slice(offset));
      node.replaceWith(fragment);
      count += matches.length;
    }
  }
  return count ? template.innerHTML : html;
}

export function openReference(kind: string, target: string, chatId: string) {
  const owner = ownerZapper(chatId);
  if (kind === "thread") return openChat(target);
  if (kind === "agent") return openSubagent(chatId, target);
  if (!owner) {
    if (kind === "pr" && workspace.changeRequests[chatId]?.changeRequest?.url.replace(/\/$/, "") === target) return openPanel("pullRequest");
    if (kind === "pr") window.open(target, "_blank", "noopener,noreferrer");
    return;
  }
  const selected = state.chats[chatId] ? chatId : owner.coordinatorChatId;
  showPage(owner.id, kind === "issue" ? { kind: "issue", id: target } : { kind: "pr", url: target, chatId: selected });
  setInspectorOpen(true);
  setView(draft => {
    draft.route = { kind: "zappers", zapperId: owner.id };
    draft.rightPanel = null;
    draft.selectedChatId = selected;
  });
}

export function referenceClick(event: MouseEvent, catalog: ReferenceCatalog, chatId: string) {
  const anchor = (event.target as Element | null)?.closest<HTMLAnchorElement>("a.md-ref");
  if (!anchor) return;
  const kind = anchor.dataset.refKind ?? "";
  if (kind === "pr" && (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0)) return;
  const label = anchor.dataset.ref ?? "";
  const target = catalog.index.get(label)?.target ?? (kind === "pr" ? anchor.getAttribute("href") : null);
  event.preventDefault();
  if (target) openReference(kind, target, chatId);
}

export function ReferenceMarkdown(props: { text: string; live: boolean; chatId: string; references: ReferenceCatalog }) {
  return (
    <Markdown
      text={props.text}
      live={props.live}
      decorate={(html) => decorateReferences(html, props.references)}
      onClick={(event) => referenceClick(event, props.references, props.chatId)}
    />
  );
}

const DISMISSED = "zuse.dismissedAsks";
const readDismissed = () => {
  try {
    return new Set<string>(JSON.parse(localStorage.getItem(DISMISSED) ?? "[]"));
  } catch {
    return new Set<string>();
  }
};
const [dismissed, setDismissed] = createSignal(readDismissed());
function dismiss(key: string) {
  const next = new Set([...dismissed(), key]);
  setDismissed(next);
  try {
    localStorage.setItem(DISMISSED, JSON.stringify([...next].slice(-500)));
  } catch {
    // Storage can be unavailable in private windows; the dismissal then lasts for the tab.
  }
}

const escapeHtml = (text: string) => text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

export function WaitingBar(props: { chatId: string; references: ReferenceCatalog }) {
  const asks = createMemo(() => openAsks(ownerZapper(props.chatId), props.chatId).filter(ask => !dismissed().has(ask.key)));
  const issueChip = (issueId: string) => {
    const reference = [...props.references.index.values()].find(item => item.kind === "issue" && item.target === issueId);
    return reference ? ` for ${chip(reference, reference.label).outerHTML}` : "";
  };
  return (
    <For each={asks()}>
      {(ask) => (
        <div class="t-waiting" role="status" onClick={(event) => referenceClick(event, props.references, props.chatId)}>
          <Icon name="status-question" size={14} class="t-waiting-icon" />
          {/* eslint-disable-next-line solid/no-innerhtml -- escaped text; chips are built with DOM APIs and bundled SVGs */}
          <span class="t-waiting-text" innerHTML={`Waiting on you${ask.issueId ? issueChip(ask.issueId) : ""}: <span class="t-waiting-ask">${decorateReferences(escapeHtml(ask.text), props.references)}</span>`} />
          <button type="button" class="t-waiting-dismiss" onClick={() => dismiss(ask.key)}>Dismiss</button>
        </div>
      )}
    </For>
  );
}
