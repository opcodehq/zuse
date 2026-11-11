// `@` file mentions and `/` commands: token detection, engine catalogs, and send-time canonical links.
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import type { Draft } from "./drafts";

export interface Token { kind: "mention" | "slash"; start: number; end: number; query: string }
export interface FileMatch { path: string; isDir: boolean }
export interface SlashCommand { name: string; description?: string; inputHint?: string | null }
export interface Skill { name: string; path: string; description: string; enabled: boolean; command?: { name: string; harness: string } | null }

export type WorkspaceCommand = "model" | "new" | "settings" | "diff" | "files" | "terminal" | "stop";
const WORKSPACE_COMMANDS: { command: WorkspaceCommand; name: string; description: string; needsChat: boolean }[] = [
  { command: "model", name: "model", description: "Zuse: choose agent, model, and reasoning", needsChat: false },
  { command: "new", name: "new", description: "Zuse: start a new conversation", needsChat: false },
  { command: "settings", name: "settings", description: "Zuse: open settings", needsChat: false },
  { command: "diff", name: "diff", description: "Zuse: open changes", needsChat: true },
  { command: "files", name: "files", description: "Zuse: open project files", needsChat: true },
  { command: "terminal", name: "terminal", description: "Zuse: open a terminal", needsChat: true },
  { command: "stop", name: "stop", description: "Zuse: stop the active run", needsChat: true },
];

export type SlashRow =
  | { kind: "command"; name: string; description: string; hint?: string | null }
  | { kind: "skill"; name: string; description: string; skill: Skill }
  | { kind: "workspace"; name: string; description: string; command: WorkspaceCommand };

/** The `@word` or leading-word `/word` the caret is inside, if any. */
export function tokenAt(text: string, caret: number): Token | null {
  let start = caret;
  while (start > 0 && !/\s/.test(text[start - 1]!)) start--;
  const word = text.slice(start, caret);
  const prefix = word[0];
  if (prefix !== "@" && prefix !== "/") return null;
  if (prefix === "/" && text.slice(0, start).trim() !== "") return null;
  const query = word.slice(1);
  if (prefix === "/" && query.includes("/")) return null;
  let end = caret;
  while (end < text.length && !/\s/.test(text[end]!)) end++;
  return { kind: prefix === "@" ? "mention" : "slash", start, end, query };
}

/** Where completions search: the chat's checkout, or the new chat's project. */
export function completionScope(chatId: string | null, spaceId: string | undefined): Record<string, string> | null {
  if (chatId) return { chatId };
  if (spaceId) return { spaceId };
  return null;
}

export function searchFiles(query: string, scope: Record<string, string>, targetDeviceId: string | null) {
  const target = targetDeviceId && targetDeviceId !== state.localDeviceId ? { targetDeviceId } : {};
  return rpc.call<FileMatch[]>(M.SEARCH_FILES, { query, ...scope, ...target });
}

const catalogCache = new Map<string, Promise<{ commands: SlashCommand[]; skills: Skill[]; error?: string }>>();
export function slashCatalog(harness: string, scope: Record<string, string> | null, targetDeviceId: string | null) {
  const key = `${state.epoch}|${harness}|${JSON.stringify(scope)}|${targetDeviceId}`;
  let entry = catalogCache.get(key);
  if (!entry) {
    const params = { harness, ...(scope ?? {}), ...(targetDeviceId && targetDeviceId !== state.localDeviceId ? { targetDeviceId } : {}) };
    entry = Promise.allSettled([rpc.call<SlashCommand[]>(M.LIST_COMMANDS, params), rpc.call<Skill[]>(M.LIST_SKILLS, params)]).then(([commands, skills]) => {
      const failure = [commands, skills].find((r) => r.status === "rejected") as PromiseRejectedResult | undefined;
      // A failed lookup should be retried the next time the menu opens.
      if (commands.status === "rejected" && skills.status === "rejected") catalogCache.delete(key);
      return {
        commands: commands.status === "fulfilled" ? commands.value : [],
        skills: skills.status === "fulfilled" ? skills.value.filter((s) => s.enabled !== false) : [],
        error: failure && commands.status === "rejected" && skills.status === "rejected" ? `Couldn't load commands: ${failure.reason instanceof Error ? failure.reason.message : String(failure.reason)}` : undefined,
      };
    });
    catalogCache.set(key, entry);
  }
  return entry;
}

export function slashRows(commands: SlashCommand[], skills: Skill[], inChat: boolean, query: string): SlashRow[] {
  const rows: SlashRow[] = [
    ...commands.map((c) => ({ kind: "command" as const, name: c.name, description: c.description ?? "", hint: c.inputHint })),
    ...skills.map((s) => ({ kind: "skill" as const, name: s.name, description: s.description, skill: s })),
  ];
  for (const w of WORKSPACE_COMMANDS) {
    if (w.needsChat && !inChat) continue;
    let name = w.name;
    while (rows.some((r) => r.name === name)) name = `zuse:${name}`;
    rows.push({ kind: "workspace", name, description: w.description, command: w.command });
  }
  const q = query.toLowerCase();
  if (!q) return rows;
  const scored = rows
    .map((row) => {
      const name = row.name.toLowerCase();
      const score = name.startsWith(q) ? 0 : name.includes(q) ? 1 : row.description.toLowerCase().includes(q) ? 2 : -1;
      return { row, score };
    })
    .filter((s) => s.score >= 0);
  return scored.sort((a, b) => a.score - b.score).map((s) => s.row);
}

/** A typed `/name` that exactly names a Zuse command runs it instead of sending. */
export function workspaceCommandFor(text: string, inChat: boolean): WorkspaceCommand | null {
  const match = /^\s*\/(\S+)\s*$/.exec(text);
  if (!match) return null;
  const name = match[1]!.replace(/^zuse:/, "");
  return WORKSPACE_COMMANDS.find((w) => w.name === name && (!w.needsChat || inChat))?.command ?? null;
}

function encodePath(path: string) {
  return Array.from(new TextEncoder().encode(path))
    .map((b) => (/[A-Za-z0-9\-._~/]/.test(String.fromCharCode(b)) ? String.fromCharCode(b) : `%${b.toString(16).toUpperCase().padStart(2, "0")}`))
    .join("");
}
const escapeLabel = (label: string) => label.replace(/\\/g, "\\\\").replace(/\[/g, "\\[").replace(/\]/g, "\\]").replace(/`/g, "\\`");

export function fileLink(path: string, isDir: boolean) {
  const clean = path.replace(/\/+$/, "");
  const base = clean.split("/").pop() || clean;
  return `[${escapeLabel(base)}](zuse-file:${encodePath(clean + (isDir ? "/" : ""))})`;
}

function skillLink(skill: Skill) {
  const payload: Record<string, unknown> = { kind: "skill", name: skill.name, path: skill.path };
  if (skill.command) payload.command = skill.command;
  const hex = Array.from(new TextEncoder().encode(JSON.stringify(payload)))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `[$${escapeLabel(skill.name)}](zuse-invoke:${hex})`;
}

/** Turn the readable `@path` and `$skill` picks back into the engine's canonical reference links. */
export function canonicalText(draft: Draft): string {
  let text = draft.text;
  const picks = [
    ...draft.mentions.map((m) => ({ token: m.token, link: fileLink(m.path, m.isDir) })),
    ...draft.skills.map((s) => ({ token: s.token, link: skillLink(s.skill) })),
  ].sort((a, b) => b.token.length - a.token.length);
  for (const pick of picks) {
    const pattern = new RegExp(`(^|\\s)${pick.token.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?=\\s|$)`, "g");
    text = text.replace(pattern, (_, lead: string) => `${lead}${pick.link}`);
  }
  return text;
}

function decodePath(encoded: string) {
  try {
    return decodeURIComponent(encoded);
  } catch {
    return encoded;
  }
}

/** The reverse of canonicalText for a sent message: file links read as `@path` mentions again. */
export function readableDraft(raw: string): Draft {
  const mentions: Draft["mentions"] = [];
  const text = raw.replace(/\[(?:[^\]\\]|\\.)*\]\(zuse-file:([^)\s]+)\)/g, (_, encoded: string) => {
    const full = decodePath(encoded);
    const isDir = full.endsWith("/");
    const token = `@${full}`;
    if (!mentions.some((m) => m.token === token)) mentions.push({ token, path: full.replace(/\/+$/, ""), isDir });
    return token;
  });
  return { text, mentions, skills: [] };
}
