import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { SessionMessageEntry, ToolCall } from "@/core/protocol";
import { state } from "@/core/store";
import { openChat, setView } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { openSubagent } from "@/features/workspace/agents";
import { fileIconUrl } from "@/features/workspace/fileIcons";
import { files, openFileInPanel } from "@/features/workspace/files/model";
import { openPanel, workspace } from "@/features/workspace/store";
import type { WorkspaceFileSearchMatch } from "@/features/workspace/types";
import { showPage } from "@/features/zappers/panel";
import { setInspectorOpen } from "@/features/zappers/state";
import { iconSvg } from "@/ui/Icon";
import { isIssueStatus, ISSUE_STATUS_LABELS, StatusIcon, statusIconHtml } from "@/ui/StatusIcon";
import { type Accessor, createMemo, createSignal, createStore, For } from "solid-js";
import { Markdown } from "./Markdown";
import { agentStatus, isImagePath, parseAttachments, userRawText } from "./model";
import { decodeFileHref, findCommits, findReferences, type InlineReference, MATCH_LIMIT, openAsks, parseCommitUrl, parsePrUrl, type ReferenceCatalog, referenceCatalog, commitReference, type ReferenceMatch } from "./references";
import "./references.css";

const HTML_LIMIT = 200_000;

export function ownerZapper(chatId: string) {
  const direct = zapperForChat(chatId);
  if (direct) return direct;
  const linked = state.zappers.filter(zapper => zapper.issues.some(issue => issue.threads.includes(chatId)));
  return linked.length === 1 ? linked[0] : undefined;
}

const signatures = new WeakMap<ReferenceCatalog, string>();
function signature(catalog: ReferenceCatalog) {
  let value = signatures.get(catalog);
  if (value === undefined) {
    value = [...catalog.index.values()].map(ref => [ref.kind, ref.label, ref.target, ref.title, ref.status, ref.text, ref.icon].join("\u0001")).join("\u0002");
    signatures.set(catalog, value);
  }
  return value;
}

const agentTitle = (call: ToolCall) => call.kind === "unknown" && call.name.startsWith("Agent: ") ? call.name.slice(7) : "Subagent";

const [checked, setChecked] = createStore<Record<string, Record<string, "file" | "directory" | false>>>({});
const requested = new Set<string>();
const CHECK_LIMIT = 400;
const PATH_CANDIDATE = /(?<![\p{L}\p{N}_/.@:#-])((?:[\w@.-]+\/)+[\w-]+(?:\.[\w-]+)+)(?![\p{L}\p{N}_/#-])/gu;

/** Paths the transcript mentions but no loaded metadata knows are checked once against the chat's workspace. */
function checkPaths(chatId: string, text: string, known: ReferenceCatalog) {
  if (state.connection.kind !== "connected" || !state.chats[chatId]) return;
  for (const match of text.matchAll(PATH_CANDIDATE)) {
    const path = match[1]!;
    const key = `${chatId}\u0000${path}`;
    if (known.index.has(path) || requested.has(key) || requested.size >= CHECK_LIMIT) continue;
    requested.add(key);
    const settle = (kind: "file" | "directory" | false) => setChecked(draft => { (draft[chatId] ??= {})[path] = kind; });
    rpc.call<WorkspaceFileSearchMatch[] | { matches: WorkspaceFileSearchMatch[] }>(M.SEARCH_WORKSPACE_FILES, { chatId, targetDeviceId: state.chats[chatId]?.deviceId ?? null, query: path, limit: 20 }).then(
      (result) => {
        const hit = (Array.isArray(result) ? result : result.matches).find(item => item.path.replace(/\/+$/, "") === path);
        settle(hit ? (hit.kind === "directory" ? "directory" : "file") : false);
      },
      () => settle(false),
    );
  }
}

function toolPaths(messages: readonly SessionMessageEntry[], cwd: string | null | undefined) {
  const paths = new Set<string>();
  const add = (value: unknown) => {
    if (typeof value !== "string" || !value) return;
    if (cwd && value.startsWith(`${cwd.replace(/\/+$/, "")}/`)) paths.add(value.slice(cwd.replace(/\/+$/, "").length + 1));
    else if (!value.startsWith("/")) paths.add(value);
  };
  for (const entry of messages)
    for (const part of entry.parts) {
      if (part.kind !== "tool") continue;
      const call = part.call as unknown as { kind?: string; path?: unknown };
      if (["readFile", "editFile", "writeFile", "applyPatch"].includes(call.kind ?? "")) add(call.path);
      add((part.diff as { path?: unknown } | undefined)?.path);
    }
  return paths;
}

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
      files: [
        ...[...toolPaths(messages, state.chats[id]?.cwd)].map((path): { path: string; directory?: boolean } => ({ path })),
        ...Object.values(files[id]?.dirs ?? {}).flatMap(dir => dir.entries.map(entry => ({ path: entry.path, directory: entry.kind === "directory" }))),
        ...Object.entries(checked[id] ?? {}).flatMap(([path, kind]) => kind ? [{ path, directory: kind === "directory" }] : []),
      ].map(file => ({ ...file, icon: file.directory ? undefined : fileIconUrl(file.path.slice(file.path.lastIndexOf("/") + 1)) })),
      branches: [
        ...Object.values(state.chats).flatMap(chat => chat.branch ? [{ name: chat.branch, chatId: chat.id }] : []),
        ...Object.values(workspace.changeRequests).flatMap(status => status?.changeRequest ? [{ name: status.changeRequest.headRef, chatId: Object.values(state.chats).find(chat => chat.checkoutId === status.checkoutId)?.id }] : []),
      ],
      attachments: messages.flatMap(entry => entry.role === "user" ? parseAttachments(userRawText(entry)).attachments.filter(item => isImagePath(item.path)) : []),
      zappers: state.zappers.map(zapper => ({ id: zapper.id, name: zapper.config.name, status: zapper.status })),
    });
  }, { equals: (a, b) => signature(a) === signature(b) });
}

const PR_ICONS: Record<string, [string, string]> = {
  open: ["git-state-open", "success"],
  draft: ["git-state-draft", "muted"],
  merged: ["git-state-merged", "merged"],
  closed: ["git-state-closed", "danger"],
};
const AGENT_TONES: Record<string, string> = { running: "accent", done: "success", failed: "danger" };
const ZAPPER_TONES: Record<string, string> = { active: "success", paused: "warning" };
const STATUS_LABELS: Record<string, string> = {
  ...ISSUE_STATUS_LABELS,
  open: "Open", draft: "Draft", merged: "Merged", closed: "Closed", running: "Running", failed: "Failed", active: "Active", paused: "Paused", stopped: "Stopped", archived: "Archived",
};
const COMMIT_SVG = '<svg width="100%" height="100%" viewBox="0 0 16 16"><circle cx="8" cy="8" r="2.75" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M1.5 8h3.75M10.75 8h3.75" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>';
const svg = (name: string) => iconSvg(name).trim().replace(/>\s+</g, "><");

function presentation(reference: InlineReference): [string, string | undefined] {
  const status = reference.status ?? "";
  switch (reference.kind) {
    case "issue": return [isIssueStatus(status) ? statusIconHtml(status) : svg("info-circle"), undefined];
    case "pr": {
      const [icon, tone] = PR_ICONS[status] ?? ["git-lane-pr", undefined];
      return [svg(icon), tone];
    }
    case "agent": return [svg("tool-agent"), AGENT_TONES[status]];
    case "file": {
      if (status === "directory") return [svg("folder"), "muted"];
      const img = document.createElement("img");
      img.alt = "";
      img.draggable = false;
      if (reference.icon) img.src = reference.icon;
      return [reference.icon ? img.outerHTML : svg("document"), undefined];
    }
    case "commit": return [COMMIT_SVG, "muted"];
    case "branch": return [svg("git-lane-branch"), "muted"];
    case "attachment": return [svg("gallery"), "muted"];
    case "zapper": return [svg("tool-agent"), ZAPPER_TONES[status] ?? "muted"];
    default: return [svg("chat-round-line"), undefined];
  }
}

const external = (kind: string) => kind === "pr" || kind === "commit";

function chip(reference: InlineReference, text: string) {
  const [icon, tone] = presentation(reference);
  const anchor = document.createElement("a");
  anchor.className = "md-ref";
  anchor.href = external(reference.kind) ? reference.target : "#";
  anchor.dataset.refKind = reference.kind;
  anchor.dataset.ref = reference.label;
  if (reference.kind === "commit" || reference.kind === "file") anchor.dataset.target = reference.target;
  if (reference.line) anchor.dataset.line = String(reference.line);
  if (reference.status) anchor.dataset.status = reference.status;
  if (tone) anchor.dataset.tone = tone;
  const status = reference.status && reference.status !== "directory" ? STATUS_LABELS[reference.status] ?? reference.status : undefined;
  const title = reference.line ? `${reference.title}, line ${reference.line}` : reference.title;
  anchor.title = status ? `${title} · ${status}` : title;
  anchor.setAttribute("aria-label", `${text}: ${anchor.title}`);
  if (external(reference.kind)) anchor.rel = "noopener noreferrer";
  const glyph = document.createElement("span");
  glyph.className = "md-ref-icon";
  glyph.setAttribute("aria-hidden", "true");
  glyph.innerHTML = icon;
  const lead = document.createElement("span");
  lead.className = "md-ref-lead";
  const first = Array.from(text)[0] ?? "";
  lead.append(glyph, first);
  anchor.append(lead, text.slice(first.length));
  return anchor;
}

function replaceText(node: Text, matches: ReferenceMatch[]) {
  const fragment = document.createDocumentFragment();
  let offset = 0;
  for (const match of matches) {
    fragment.append(node.data.slice(offset, match.start), chip(match.reference, match.reference.text ?? node.data.slice(match.start, match.end)));
    offset = match.end;
  }
  fragment.append(node.data.slice(offset));
  node.replaceWith(fragment);
}

function plainTextNodes(root: DocumentFragment) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (!node.parentElement?.closest("a, code, pre, kbd, samp")) nodes.push(node);
  }
  return nodes;
}

const LOCAL_LINE = /^(.*?)(?::(\d{1,7})(?::\d+)?|#L(\d{1,7}))?$/;

export function decorateReferences(html: string, catalog: ReferenceCatalog, chatId?: string): string {
  if (html.length > HTML_LIMIT) return html;
  const template = document.createElement("template");
  template.innerHTML = html;
  let count = 0;
  for (const anchor of template.content.querySelectorAll("a")) {
    if (anchor.closest("code, pre")) continue;
    const href = anchor.getAttribute("href") ?? "";
    const text = anchor.textContent?.trim() ?? "";
    const pr = parsePrUrl(href);
    const commit = parseCommitUrl(href);
    if (pr && text === href) {
      const known = catalog.index.get(`${pr.owner}/${pr.repo}#${pr.number}`);
      anchor.replaceWith(chip(known ?? { kind: "pr", label: `${pr.owner}/${pr.repo}#${pr.number}`, title: href, target: href.replace(/\/$/, "") }, `${pr.repo}#${pr.number}`));
      count++;
    } else if (commit && text === href) {
      anchor.replaceWith(chip(commitReference(commit.repo, commit.sha), `${commit.repo.split("/")[1]}@${commit.sha.slice(0, 7)}`));
      count++;
    } else if (anchor.hasAttribute("data-local")) {
      const local = anchor.getAttribute("title") ?? "";
      const mention = decodeFileHref(local);
      const [, rawPath = "", line, hashLine] = LOCAL_LINE.exec(mention ?? local) ?? [];
      const path = rawPath.replace(/^\.\//, "").replace(/\/+$/, "");
      const known = catalog.index.get(path);
      if (!path || path.startsWith("/") || (mention === null && known?.kind !== "file")) continue;
      const directory = (mention ?? local).endsWith("/") || known?.status === "directory";
      const reference: InlineReference = known?.kind === "file" ? { ...known } : { kind: "file", label: path, title: path, target: path, icon: directory ? undefined : fileIconUrl(path.slice(path.lastIndexOf("/") + 1)), status: directory ? "directory" : undefined };
      const lineNumber = Number(line ?? hashLine);
      if (lineNumber) reference.line = lineNumber;
      anchor.replaceWith(chip(reference, text || path));
      count++;
    }
  }
  for (const node of plainTextNodes(template.content)) {
    if (count >= MATCH_LIMIT) break;
    if (chatId) checkPaths(chatId, node.data, catalog);
    const matches = findReferences(node.data, catalog, MATCH_LIMIT - count);
    if (!matches.length) continue;
    replaceText(node, matches);
    count += matches.length;
  }
  const repos = new Set([...template.content.querySelectorAll<HTMLAnchorElement>("a.md-ref[data-ref-kind=pr]")].map(anchor => { const pr = parsePrUrl(anchor.getAttribute("href") ?? ""); return pr ? `${pr.owner}/${pr.repo}` : ""; }).filter(Boolean));
  const context = repos.size === 1 ? [...repos][0]! : null;
  for (const node of plainTextNodes(template.content)) {
    if (count >= MATCH_LIMIT) break;
    const matches = findCommits(node.data, context, MATCH_LIMIT - count);
    if (!matches.length) continue;
    replaceText(node, matches);
    count += matches.length;
  }
  return count ? template.innerHTML : html;
}

export function openReference(kind: string, target: string, chatId: string, line?: number) {
  if (kind === "thread") return openChat(target);
  if (kind === "agent") return openSubagent(chatId, target);
  if (kind === "file") return openFileInPanel(target, line);
  if (kind === "commit") return void window.open(target, "_blank", "noopener,noreferrer");
  if (kind === "zapper") {
    const zapper = state.zappers.find(item => item.id === target);
    if (zapper) setView(draft => { draft.route = { kind: "zappers", zapperId: zapper.id }; draft.selectedChatId = zapper.coordinatorChatId; });
    return;
  }
  if (kind === "branch") {
    if (target === chatId) return openPanel("git");
    if (!state.chats[target]) return;
    openChat(target);
    return openPanel("git");
  }
  const owner = ownerZapper(chatId);
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
  if (external(kind) && (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0)) return;
  event.preventDefault();
  const label = anchor.dataset.ref ?? "";
  if (kind === "attachment") {
    anchor.closest(".t-root")?.querySelector<HTMLButtonElement>(`.t-thumb[title="${CSS.escape(label)}"]`)?.click();
    return;
  }
  const target = anchor.dataset.target ?? catalog.index.get(label)?.target ?? (kind === "pr" ? anchor.getAttribute("href") : null);
  if (target) openReference(kind, target, chatId, Number(anchor.dataset.line) || undefined);
}

export function ReferenceMarkdown(props: { text: string; live: boolean; chatId: string; references: ReferenceCatalog; htmlPreview?: boolean }) {
  return (
    <Markdown
      text={props.text}
      live={props.live}
      htmlPreview={props.htmlPreview}
      decorate={(html) => decorateReferences(html, props.references, props.chatId)}
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
          <StatusIcon status="needsYou" class="t-waiting-icon" />
          {/* eslint-disable-next-line solid/no-innerhtml -- escaped text; chips are built with DOM APIs and bundled SVGs */}
          <span class="t-waiting-text" innerHTML={`Waiting on you${ask.issueId ? issueChip(ask.issueId) : ""}: <span class="t-waiting-ask">${decorateReferences(escapeHtml(ask.text), props.references)}</span>`} />
          <button type="button" class="t-waiting-dismiss" onClick={() => dismiss(ask.key)}>Dismiss</button>
        </div>
      )}
    </For>
  );
}
