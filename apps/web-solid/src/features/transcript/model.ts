// Pure transcript logic ported from crates/ui/src/transcript.rs and zuse_protocol::view.
import type { MessagePart, SessionMessageEntry } from "@/core/protocol";
import { state } from "@/core/store";

export interface TodoItem {
  text: string;
  done: boolean;
  status?: "pending" | "inProgress" | "completed" | string;
}

export type ToolCall =
  | { kind: "exec"; command: string }
  | { kind: "readFile"; path: string }
  | { kind: "writeFile"; path: string; content?: string }
  | { kind: "editFile"; path: string; old_string?: string; new_string?: string }
  | { kind: "applyPatch"; path?: string }
  | { kind: "search"; pattern: string; path?: string }
  | { kind: "glob"; pattern: string }
  | { kind: "webFetch"; url: string; prompt?: string }
  | { kind: "webSearch"; query: string }
  | { kind: "todo"; items?: TodoItem[] }
  | { kind: "mcp"; server: string; tool: string; input?: unknown }
  | { kind: "unknown"; name: string; input?: unknown };

export interface ToolDiff {
  path: string;
  oldText?: string | null;
  newText: string;
}

export interface ToolDiffStat {
  path: string;
  additions: number;
  deletions: number;
}

export type ToolPart = Extract<MessagePart, { kind: "tool" }>;
export type TextPart = Extract<MessagePart, { kind: "text" }>;
export type ReasoningPart = Extract<MessagePart, { kind: "reasoning" }>;
export type InputPart = Extract<MessagePart, { kind: "input" }>;

export const callOf = (part: ToolPart) => part.call as unknown as ToolCall;

export type SubagentStatus = "running" | "done" | "failed";

/** One chip inside a tool group. Thoughts and notes are synthesized from reasoning and text parts. */
export interface ToolItem {
  key: string;
  kind: "call" | "thought" | "note";
  part: ToolPart | ReasoningPart | TextPart;
}

/** `single_line`: collapse all whitespace so a chip stays one line. */
export const singleLine = (text: string) => text.split(/\s+/).filter(Boolean).join(" ");

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** `tool_chip_content`: per-kind label and one-line detail. */
export function toolChipContent(call: ToolCall): [string, string] {
  const [label, detail] = chipContentRaw(call);
  return [label, singleLine(detail)];
}

function chipContentRaw(call: ToolCall): [string, string] {
  switch (call.kind) {
    case "exec": return ["Run", call.command];
    case "readFile": return ["Read", call.path];
    case "writeFile": return ["Write", call.path];
    case "editFile": return ["Edit", call.path];
    case "applyPatch": return ["Patch", call.path ?? "workspace"];
    case "search": return ["Search", call.path ? `${call.pattern} in ${call.path}` : call.pattern];
    case "glob": return ["Glob", call.pattern];
    case "webFetch": return ["Fetch", call.url];
    case "webSearch": return ["Web", call.query];
    case "todo": {
      const items = call.items ?? [];
      return ["Todo", `${items.filter((i) => i.done).length}/${items.length} done`];
    }
    case "mcp": return ["MCP", `${call.server} · ${call.tool}`];
    case "unknown":
      if (call.name.startsWith("Agent: ")) return ["Agent", call.name.slice(7)];
      if (call.name === "Agent") return ["Agent", ""];
      return ["Tool", call.name];
    default: return ["Tool", ""];
  }
}

/** The protocol's one-line group summary ("Ran 3 commands · edited 2 files"), used for accessible descriptions. */
export function protocolGroupSummary(tools: { call: ToolCall; isError: boolean }[]): string {
  let commands = 0, reads = 0, searches = 0, fetches = 0, todos = 0, other = 0, failed = 0;
  const edited: string[] = [];
  for (const { call, isError } of tools) {
    if (isError) failed++;
    switch (call.kind) {
      case "exec": commands++; break;
      case "writeFile": case "editFile": if (!edited.includes(call.path)) edited.push(call.path); break;
      case "applyPatch": { const p = call.path ?? "patch"; if (!edited.includes(p)) edited.push(p); break; }
      case "readFile": reads++; break;
      case "search": case "glob": case "webSearch": searches++; break;
      case "webFetch": fetches++; break;
      case "todo": todos++; break;
      default: other++;
    }
  }
  const segments: string[] = [];
  if (commands) segments.push(`ran ${plural(commands, "command", "commands")}`);
  if (edited.length) segments.push(`edited ${plural(edited.length, "file", "files")}`);
  if (reads) segments.push(`read ${plural(reads, "file", "files")}`);
  if (searches) segments.push(`searched ${plural(searches, "time", "times")}`);
  if (fetches) segments.push(`fetched ${plural(fetches, "page", "pages")}`);
  if (todos) segments.push("updated todos");
  if (other) segments.push(`called ${plural(other, "tool", "tools")}`);
  if (!segments.length) segments.push(plural(tools.length, "tool", "tools"));
  if (failed) segments.push(`${failed} failed`);
  const summary = segments.join(" · ");
  return summary.charAt(0).toUpperCase() + summary.slice(1);
}

export function isSubagentSpawn(call: ToolCall) {
  const name = call.kind === "unknown" ? call.name : call.kind === "mcp" ? call.tool : null;
  return name !== null && (name === "Agent" || name.startsWith("Agent: "));
}

export const isWorkflowRun = (call: ToolCall) => call.kind === "unknown" && call.name === "Workflow";

export const isAgentItem = (item: ToolItem) => item.kind === "call" && isSubagentSpawn(callOf(item.part as ToolPart));

export function agentStatus(part: ToolPart): SubagentStatus {
  const status = part.subagentStatus as SubagentStatus | undefined;
  if (status) return status;
  if (part.isError) return "failed";
  return part.resolved ? "done" : "running";
}

/** `tool_group_collapses`: ordinary tool groups fold behind a header; lone agents and pure thinking do not. */
export function groupCollapses(tools: ToolItem[]) {
  return tools.some((t) => !isAgentItem(t) && t.kind !== "thought") || tools.filter(isAgentItem).length > 1;
}

export type ToolActivity = "command" | "read" | "search" | "edit" | "write" | "webSearch" | "fetch" | "other";

export function activityOf(call: ToolCall): ToolActivity {
  switch (call.kind) {
    case "exec": return "command";
    case "readFile": return "read";
    case "search": case "glob": return "search";
    case "editFile": case "applyPatch": return "edit";
    case "writeFile": return "write";
    case "webSearch": return "webSearch";
    case "webFetch": return "fetch";
    default: return "other";
  }
}

const ACTIVITY_SUMMARY: Record<ToolActivity, (n: number) => string> = {
  command: (n) => `ran ${plural(n, "command", "commands")}`,
  read: (n) => `read ${plural(n, "file", "files")}`,
  search: (n) => `ran ${plural(n, "search", "searches")}`,
  edit: (n) => `made ${plural(n, "edit", "edits")}`,
  write: (n) => `wrote ${plural(n, "file", "files")}`,
  webSearch: (n) => `ran ${plural(n, "web search", "web searches")}`,
  fetch: (n) => `fetched ${plural(n, "page", "pages")}`,
  other: (n) => `used ${plural(n, "tool", "tools")}`,
};

const ACTIVITY_RUNNING: Record<Exclude<ToolActivity, "other">, string> = {
  command: "Running command…",
  read: "Reading files…",
  search: "Searching files…",
  edit: "Editing files…",
  write: "Writing files…",
  webSearch: "Searching the web…",
  fetch: "Fetching page…",
};

function toolDisplayName(call: ToolCall) {
  if (isSubagentSpawn(call)) return "Agent";
  if (call.kind === "mcp") return call.tool;
  if (call.kind === "unknown") return call.name;
  return toolChipContent(call)[0];
}

export interface GroupSummary {
  text: string;
  running: boolean;
}

/** `tool_group_summary` + `ToolGroupSummary::text`. */
export function toolGroupSummary(tools: ToolItem[], live: boolean): GroupSummary {
  const calls = tools.filter((t) => t.kind === "call").map((t) => t.part as ToolPart);
  if (calls.length > 1 && calls.every((c) => isSubagentSpawn(callOf(c)))) {
    const count = (s: SubagentStatus) => calls.filter((c) => agentStatus(c) === s).length;
    const running = count("running");
    if (running > 0) return { text: `${running} of ${calls.length} running`, running: false };
    const done = count("done"), failed = count("failed");
    return { text: [done && `${done} done`, failed && `${failed} failed`].filter(Boolean).join(" · "), running: false };
  }
  if (live) {
    const pending = [...calls].reverse().find((c) => !c.resolved);
    if (pending) {
      const call = callOf(pending);
      const activity = activityOf(call);
      return { text: activity === "other" ? `Using ${toolDisplayName(call)}…` : ACTIVITY_RUNNING[activity], running: true };
    }
  }
  if (!calls.length) {
    const notes = tools.filter((t) => t.kind === "note").length;
    if (tools.some((t) => t.kind === "thought")) return { text: "Thinking", running: false };
    if (notes) return { text: notes === 1 ? "Wrote a note" : `Wrote ${notes} notes`, running: false };
    return { text: "", running: false };
  }
  const counts = new Map<ToolActivity, number>();
  for (const call of calls) {
    const activity = activityOf(callOf(call));
    counts.set(activity, (counts.get(activity) ?? 0) + 1);
  }
  let text = [...counts].map(([activity, n]) => ACTIVITY_SUMMARY[activity](n)).join(", ");
  text = text.charAt(0).toUpperCase() + text.slice(1);
  const failed = calls.filter((c) => c.isError).length;
  if (failed) text += ` · ${failed} failed`;
  return { text, running: false };
}

/** `turn_summary`: a settled turn's fold header. */
export function turnSummary(tools: ToolItem[]): string {
  const calls = tools.filter((t) => t.kind === "call");
  const messages = tools.length - calls.length;
  const failed = calls.filter((t) => (t.part as ToolPart).isError).length;
  return [
    calls.length && plural(calls.length, "tool call", "tool calls"),
    messages && plural(messages, "message", "messages"),
    failed && `${failed} failed`,
  ].filter(Boolean).join(" · ");
}

/** `tool_icon_path`, mapped to the bundled icon names. */
export function toolIcon(item: ToolItem): string {
  if (item.kind === "thought") return "tool-thinking";
  if (item.kind === "note") return "pen";
  const call = callOf(item.part as ToolPart);
  switch (call.kind) {
    case "exec": return "dither-term";
    case "readFile": return "dither-note";
    case "writeFile": case "editFile": case "applyPatch": return "dither-pen";
    case "search": case "glob": return "dither-search";
    case "webFetch": case "webSearch": return "tool-web";
    case "todo": return "tool-todo";
    default:
      if (isSubagentSpawn(call)) return "tool-agent";
      if (call.kind === "unknown" && call.name === "Wait for agents") return "tool-agent";
      return call.kind === "mcp" ? "widget" : "tool-generic";
  }
}

/** The opening sentence of a thought, for its one-line teaser. */
export function thoughtTeaser(text: string) {
  const flat = singleLine(text);
  const match = /[.?!](\s|$)/.exec(flat);
  return match ? flat.slice(0, match.index + 1) : flat;
}

/** First non-empty line of a note, flattened. */
export function noteTeaser(text: string) {
  for (const line of text.split("\n")) {
    const flat = singleLine(line.replace(/^[#>*\-\s`]+/, ""));
    if (flat) return flat;
  }
  return "";
}

/** `chip_label`: the verb in the tense of the call, and its argument. */
export function chipLabel(item: ToolItem, text: () => string): [string, string] {
  if (item.kind === "thought") return ["Thinking", thoughtTeaser(text())];
  if (item.kind === "note") return ["Wrote", noteTeaser(text())];
  const part = item.part as ToolPart;
  const call = callOf(part);
  const [label, detail] = toolChipContent(call);
  const pending = !part.resolved;
  switch (call.kind) {
    case "readFile": return [pending ? "Reading" : label, detail];
    case "writeFile": return [pending ? "Writing" : "Wrote", detail];
    case "editFile": case "applyPatch": return [pending ? "Editing" : "Edited", detail];
    default: return [label, detail];
  }
}

export const fileBadgeName = (path: string) => path.split(/[/\\]/).filter(Boolean).pop() ?? path;

export function filePathOf(call: ToolCall): string | null {
  switch (call.kind) {
    case "readFile": case "writeFile": case "editFile": return call.path;
    case "applyPatch": return call.path ?? null;
    default: return null;
  }
}

export function chipDiffStats(part: ToolPart): [number, number] | null {
  const stats = part.diffStats as ToolDiffStat[] | undefined;
  let added = 0, removed = 0;
  if (stats?.length) {
    for (const stat of stats) { added += stat.additions; removed += stat.deletions; }
  } else if (part.diff) {
    const diff = part.diff as ToolDiff;
    const old = diff.oldText ?? "";
    // Exact counts come from the diff itself once opened; this is the header estimate.
    if (!old) added = diff.newText.split("\n").length;
    else return null;
  } else return null;
  return added || removed ? [added, removed] : null;
}

export const OUTPUT_DETAIL_MAX_LINES = 24;
export const FULL_OUTPUT_MAX_LINES = 400;
const OUTPUT_LINE_MAX_CHARS = 8 * 1024;
const CALL_WRAP_COLS = 80;

export interface OutputDetail {
  lines: string[];
  truncatedBy: number;
}

const clipLine = (line: string) => (line.length > OUTPUT_LINE_MAX_CHARS ? `${line.slice(0, OUTPUT_LINE_MAX_CHARS)}…` : line);

export function outputDetail(text: string | undefined | null, max = OUTPUT_DETAIL_MAX_LINES): OutputDetail | null {
  if (!text) return null;
  const lines = text.split("\n").map(clipLine);
  while (lines.length && !lines[lines.length - 1]!.trim()) lines.pop();
  if (!lines.length) return null;
  return { lines: lines.slice(0, max), truncatedBy: Math.max(0, lines.length - max) };
}

function wrapCols(line: string, cols: number): string[] {
  const out: string[] = [];
  let rest = line;
  while (rest.length > cols) {
    const window = rest.slice(0, cols);
    const space = window.search(/\s\S*$/);
    const cut = space + 1 >= cols / 2 ? space + 1 : cols;
    out.push(rest.slice(0, cut));
    rest = rest.slice(cut);
  }
  out.push(rest);
  return out;
}

const todoMark = (item: TodoItem) =>
  item.done || item.status === "completed" ? "[x]" : item.status === "inProgress" || item.status === "in_progress" ? "[~]" : "[ ]";

/** `call_block`: the complete invocation the one-line header truncates. */
export function callBlock(call: ToolCall): OutputDetail | null {
  const json = (value: unknown) => {
    try { return JSON.stringify(value, null, 2); } catch { return ""; }
  };
  let text: string;
  switch (call.kind) {
    case "exec": text = call.command; break;
    case "readFile": text = call.path; break;
    case "writeFile": text = call.content ? `${call.path}\n${call.content}` : call.path; break;
    case "editFile": text = call.path; break;
    case "applyPatch": text = call.path ?? "workspace"; break;
    case "search": text = call.path ? `${call.pattern} in ${call.path}` : call.pattern; break;
    case "glob": text = call.pattern; break;
    case "webFetch": text = call.prompt ? `${call.url}\n${call.prompt}` : call.url; break;
    case "webSearch": text = call.query; break;
    case "todo": text = (call.items ?? []).map((i) => `${todoMark(i)} ${i.text}`).join("\n"); break;
    case "mcp": text = call.input !== undefined && call.input !== null ? `${call.server} · ${call.tool}\n${json(call.input)}` : `${call.server} · ${call.tool}`; break;
    case "unknown": text = call.input !== undefined && call.input !== null ? `${call.name}\n${json(call.input)}` : call.name; break;
    default: return null;
  }
  const lines = text.split("\n").flatMap((l) => wrapCols(l, CALL_WRAP_COLS));
  while (lines.length && !lines[lines.length - 1]!.trim()) lines.pop();
  if (!lines.length) return null;
  return { lines: lines.slice(0, OUTPUT_DETAIL_MAX_LINES), truncatedBy: Math.max(0, lines.length - OUTPUT_DETAIL_MAX_LINES) };
}

export function formatKb(bytes: number) {
  return bytes < 1024 ? `${bytes} B` : `${Math.ceil(bytes / 1024)} KB`;
}

/** `format_elapsed`: at most two units. */
export function formatElapsed(secs: number) {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
}

const timestampFormat = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
/** "Jul 1, 3:45 PM". */
export const formatTimestamp = (ms: number) => timestampFormat.format(new Date(ms)).replace(" at ", ", ");

// ---------------------------------------------------------------------------
// User messages
// ---------------------------------------------------------------------------

export interface MessageAttachment {
  path: string;
  name: string;
}

const attachmentDisplayName = (name: string) => {
  const dash = name.indexOf("-");
  return dash === 8 && /^[0-9a-f]{8}$/i.test(name.slice(0, 8)) ? name.slice(9) : name;
};

/** `message_attachments::parse`: split the attachment trailer off a sent prompt. */
export function parseAttachments(content: string): { text: string; attachments: MessageAttachment[] } {
  const plain = { text: content, attachments: [] };
  const lower = content.toLowerCase();
  const needle = "\n\nattached images (local files";
  let from = 0;
  for (;;) {
    const gap = lower.indexOf(needle, from);
    if (gap < 0) return plain;
    const lineStart = gap + 2;
    const newline = content.indexOf("\n", lineStart);
    const lineEnd = newline < 0 ? content.length : newline;
    from = lineStart;
    if (!content.slice(lineStart, lineEnd).replace(/\r+$/, "").endsWith("):")) continue;
    const attachments: MessageAttachment[] = [];
    for (const line of content.slice(Math.min(lineEnd + 1, content.length)).split("\n")) {
      if (!line.trim()) continue;
      const trimmed = line.trimStart();
      if (!trimmed.startsWith("- ")) return plain;
      const path = trimmed.slice(2).trim();
      if (!path) return plain;
      attachments.push({ path, name: attachmentDisplayName(path.split(/[/\\]/).pop() ?? path) });
    }
    if (!attachments.length) return plain;
    const body = content.slice(0, gap).trimEnd();
    return {
      text: body === "See the attached image(s)." || body === "See the attached file(s)." ? "" : body,
      attachments,
    };
  }
}

export const isImagePath = (path: string) => /\.(png|jpe?g|gif|webp|bmp|svg|avif|heic)$/i.test(path);

export function userRawText(entry: SessionMessageEntry) {
  return entry.parts
    .filter((p): p is TextPart => p.kind === "text")
    .map((p) => p.text)
    .join("\n\n");
}

/** Clipboard text for an assistant entry: authored text parts only. */
export function assistantCopyText(entry: SessionMessageEntry) {
  return entry.parts
    .filter((p): p is TextPart => p.kind === "text" && /\S/.test(p.text))
    .map((p) => p.text)
    .join("\n\n");
}

export const USER_COLLAPSED_LINES = 5;
export const USER_COLLAPSE_CHARS = 400;
export const userNeedsCollapse = (text: string) =>
  text.split("\n").length > USER_COLLAPSED_LINES || text.length > USER_COLLAPSE_CHARS;

// ---------------------------------------------------------------------------
// Agent identity (AgentContext parts)
// ---------------------------------------------------------------------------

export interface AgentIdentity {
  harness: string;
  model?: string | null;
}

/** The engine's display name for a harness; names are never hardcoded in the client. */
export function harnessName(harness: string | undefined | null) {
  if (!harness) return "Agent";
  const row = state.harnesses.find((h) => h.id === harness);
  return row?.name?.trim() || "Agent";
}

export { harnessIcon } from "@/ui/harness";

const titleCase = (word: string) => word.charAt(0).toUpperCase() + word.slice(1);

/** Desktop `AgentIdentity::label`: `claude-fable-5-1` reads "Fable 5.1", `gpt-5-codex` "GPT-5 Codex". */
export function agentLabel(agent: AgentIdentity) {
  const id = agent.model?.split("/").pop();
  if (!id) return harnessName(agent.harness);
  if (id.startsWith("claude-")) {
    const [family = "", ...rest] = id.slice(7).split("-");
    const version: string[] = [];
    for (const part of rest) {
      if (!/^\d{0,2}$/.test(part)) break;
      version.push(part);
    }
    return version.length ? `${titleCase(family)} ${version.join(".")}` : titleCase(family);
  }
  if (id.startsWith("gpt-")) {
    const [version = "", ...rest] = id.slice(4).split("-");
    return `GPT-${version} ${rest.map(titleCase).join(" ")}`.trimEnd();
  }
  return id;
}

/** Desktop `AgentIdentity::brand`: the model's provider decides the mark, since one adapter can run several. */
export function agentBrand(agent: AgentIdentity) {
  const id = agent.model?.split("/").pop() ?? "";
  if (id.startsWith("claude-")) return "claude-code";
  if (/^(gpt-|o1|o3|o4)/.test(id)) return "codex";
  return agent.harness;
}

/** The extension's icon in the bundled set; the desktop's full file-icon theme is not shipped to the web. */
export function fileIcon(path: string) {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (/^(png|jpe?g|gif|webp|svg|bmp|ico|avif)$/.test(ext)) return "file-image";
  if (/^(md|mdx|markdown|txt|rst)$/.test(ext)) return "file-markdown";
  if (/^(css|scss|sass|less|styl)$/.test(ext)) return "file-style";
  if (/^(json|ya?ml|toml|csv|tsv|xml|lock|ini|env)$/.test(ext)) return "file-data";
  if (ext && ext !== path.toLowerCase()) return "file-code";
  return "document";
}
