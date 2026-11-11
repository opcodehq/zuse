// Small pure helpers shared by the extras surfaces. Derived from store data; never stored.
import { harnessIcon as sharedHarnessIcon } from "@/ui/harness";
export { timeAgo, errorText } from "@/core/util";
import { state } from "@/core/store";
import type { Chat } from "@/core/protocol";


const ORCHESTRATOR_PREFIX = "voice-orchestrator-";

/** Sidebar-visible sessions: no side chats and no hidden voice orchestrators. */
export const isTopLevel = (chat: Chat) => !chat.parentChatId && !chat.id.startsWith(ORCHESTRATOR_PREFIX);

export const activityAt = (chat: Chat) => Date.parse(chat.lastMessageAt ?? chat.createdAt) || 0;

export function spaceName(chat: Chat) {
  const space = chat.spaceId ? state.spaces[chat.spaceId] : undefined;
  if (!space) return chat.spaceId ? "?" : null;
  if (space.name?.trim()) return space.name;
  return space.path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || space.path;
}

export const deviceName = (id: string) => state.devices[id]?.name ?? null;

/** "project @ device", or just the device for project-less sessions. */
export function chatLocation(chat: Chat) {
  const project = spaceName(chat);
  const device = deviceName(chat.deviceId);
  if (project && device) return `${project} @ ${device}`;
  return project ?? device ?? "";
}

export const chatTitle = (chat: Chat) => (chat.title?.trim() || "New session").replace(/\s+/g, " ");

/** A path's last segment and the folder holding it. */
export function splitPath(path: string): [string, string] {
  const trimmed = path.replace(/\/+$/, "");
  const at = trimmed.lastIndexOf("/");
  return at < 0 ? [trimmed, ""] : [trimmed.slice(at + 1), trimmed.slice(0, at)];
}

/** Case-insensitive `[start, end)` ranges of every query word in `text`. */
export function matchRanges(text: string, query: string): [number, number][] {
  const lower = text.toLowerCase();
  const ranges: [number, number][] = [];
  for (const word of query.toLowerCase().split(/\s+/).filter(Boolean)) {
    for (let at = lower.indexOf(word); at >= 0; at = lower.indexOf(word, at + word.length)) ranges.push([at, at + word.length]);
  }
  ranges.sort((a, b) => a[0] - b[0]);
  const merged: [number, number][] = [];
  for (const range of ranges) {
    const last = merged[merged.length - 1];
    if (last && range[0] <= last[1]) last[1] = Math.max(last[1], range[1]);
    else merged.push([...range]);
  }
  return merged;
}

/** UTF-8 byte ranges (as the engine reports content matches) to UTF-16 string ranges. */
export function byteRangesToChars(text: string, ranges: [number, number][]): [number, number][] {
  const offsets: number[] = [];
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    offsets[bytes] = i;
    const code = text.codePointAt(i)!;
    const width = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    if (code >= 0x10000) i++;
    bytes += width;
  }
  offsets[bytes] = text.length;
  return ranges
    .map(([start, end]): [number, number] | null => {
      const a = offsets[start];
      const b = offsets[end];
      return a === undefined || b === undefined || b <= a ? null : [a, b];
    })
    .filter((range): range is [number, number] => range !== null);
}

export const harnessIcon = (id: string) => sharedHarnessIcon(id, "chat-round-line");

export const harnessName = (id: string) => state.harnesses.find((h) => h.id === id)?.name ?? id;


export type Indicator = "none" | "working" | "awaitingInput" | "errored";
/** A Working/AwaitingInput row older than this belongs to a dead engine. */
export const SESSION_STALE_MS = 45_000;

/** Desktop's `effective_indicator`: staleness-checked live status. */
export function effectiveIndicator(chatId: string, now = Date.now()): Indicator {
  const session = state.sessions[chatId];
  if (!session) return "none";
  const status = session.status;
  if (status === "errored" || status === "error") return "errored";
  if (status !== "working" && status !== "awaitingInput") return "none";
  const updated = typeof session.updatedAt === "string" ? Date.parse(session.updatedAt) : NaN;
  if (!Number.isFinite(updated) || now - updated > SESSION_STALE_MS) return "none";
  return status;
}
