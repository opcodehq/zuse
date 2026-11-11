// What the sidebar shows, derived from the store: row status, grouping, ordering and layout.
// Mirrors crates/ui (state.rs sidebar_chats, spaces.rs render_active_rows) and crates/protocol view.rs.
import type { Chat, Session, Space } from "@/core/protocol";
import { state } from "@/core/store";
import { isArchived, titleOf } from "./overlay";
import { pinnedIds, sections, type SidebarSection } from "./pins";
import { isCollapsed, option, organization } from "./prefs";
import { basename, time } from "./util";
import { deviceOnline } from "@/core/util";

export type Indicator = "working" | "awaitingInput" | "errored" | "completed" | "idle";

/** A working or waiting session nobody refreshed inside this window belongs to a dead engine. */
const STALE_MS = 45_000;

export function unseen(chat: Chat) {
  const message = time(chat.lastMessageAt);
  if (!message) return false;
  const seen = time(chat.lastSeenAt);
  return !seen || message > seen;
}

function liveStatus(session: Session | undefined, at: number) {
  const status = session?.status;
  if (status === "errored" || status === "error") return "errored";
  if (status !== "working" && status !== "awaitingInput") return null;
  const updated = time(session?.updatedAt as string | undefined);
  return updated && at - updated > STALE_MS ? null : status;
}

export function indicatorFor(chat: Chat, at: number): Indicator {
  const live = liveStatus(state.sessions[chat.id], at);
  if (live === "working" || live === "awaitingInput") return live;
  if (live === "errored" && unseen(chat)) return "errored";
  return unseen(chat) ? "completed" : "idle";
}

const loudness: Record<Indicator, number> = { awaitingInput: 4, working: 3, errored: 2, completed: 1, idle: 0 };
const louder = (a: Indicator | undefined, b: Indicator) => (a === undefined || loudness[b] > loudness[a] ? b : a);

export type AwaitingKind = "question" | "permission" | "plan";
export const awaitingKind = (chatId: string) => (state.sessions[chatId]?.awaitingInput as AwaitingKind | undefined) ?? null;

/** Threads with a live working session, as the footer's agent count shows them. */
export function workingThreads(at: number) {
  const roots = new Set<string>();
  for (const chat of Object.values(state.chats)) {
    if (chat.parentChatId || isArchived(chat)) continue;
    if (liveStatus(state.sessions[chat.id], at) === "working") roots.add(chat.threadId ?? chat.id);
  }
  return roots.size;
}

// ---- spaces and projects ----

export const projectKey = (space: Space) => (space.repositoryId ? `repo:${space.repositoryId}` : space.id);
export const spaceName = (space: Space) => space.name?.trim() || basename(space.path);

/** The oldest member speaks for a repository's project, so it looks alike on every device. */
export function representative(space: Space) {
  if (!space.repositoryId) return space;
  let best = space;
  for (const other of Object.values(state.spaces)) {
    if (other.repositoryId !== space.repositoryId) continue;
    const a = time(other.createdAt);
    const b = time(best.createdAt);
    if (a < b || (a === b && other.id < best.id)) best = other;
  }
  return best;
}

/** Checkouts of one project, this device's first. */
export function projectMembers(key: string) {
  const local = state.localDeviceId;
  return Object.values(state.spaces)
    .filter((space) => projectKey(space) === key)
    .sort((a, b) => Number(b.deviceId === local) - Number(a.deviceId === local) || time(a.createdAt) - time(b.createdAt) || a.id.localeCompare(b.id));
}

export const spaceOf = (chat: Chat) => (chat.spaceId ? state.spaces[chat.spaceId] : undefined);
export const deviceName = (deviceId: string) => state.devices[deviceId]?.name;
export const isRemote = (chat: Chat) => !!state.localDeviceId && chat.deviceId !== state.localDeviceId;

export function projectGroup(chat: Chat): { key: string; name: string } {
  const space = spaceOf(chat);
  if (space) return { key: projectKey(space), name: spaceName(representative(space)) };
  if (chat.spaceId) return { key: chat.spaceId, name: "?" };
  return { key: `home:${chat.deviceId}`, name: "No project" };
}

/** Row sub-line: "project", "project @ device" for another machine, or the device alone. */
export function locationLabel(chat: Chat) {
  const space = spaceOf(chat);
  const project = space ? spaceName(space) : chat.spaceId ? "?" : null;
  const device = deviceName(chat.deviceId);
  if (project && device) return chat.deviceId === state.localDeviceId ? project : `${project} @ ${device}`;
  return project ?? device ?? "";
}

export function branchOf(chat: Chat) {
  const source = chat.sourceContext as { branch?: string } | null | undefined;
  return (source?.branch ?? chat.branch ?? "").trim();
}

export const harnessOf = (chat: Chat) => chat.config?.harness ?? null;

export function inProjectFilter(chat: Chat, filter: string | null) {
  if (!filter) return true;
  if (chat.spaceId === filter) return true;
  const target = state.spaces[filter];
  const own = spaceOf(chat);
  return !!target && !!own && projectKey(own) === projectKey(target);
}

const activityAt = (chat: Chat) => time(chat.lastMessageAt) || time(chat.createdAt);

function compare(sort: "updated" | "created") {
  return (a: Chat, b: Chat) => {
    const primary = sort === "created" ? time(b.createdAt) - time(a.createdAt) : activityAt(b) - activityAt(a);
    return primary || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
  };
}

// ---- search (crates/ui shell/thread_search.rs) ----

function fieldScore(haystack: string, term: string, weight: number) {
  let best: number | null = null;
  for (let at = haystack.indexOf(term); at >= 0; at = haystack.indexOf(term, at + 1)) {
    const previous = haystack[at - 1];
    if (previous === undefined || !/[\p{L}\p{N}]/u.test(previous)) return weight + weight / 2;
    best = weight;
  }
  return best;
}

function scoreChat(chat: Chat, terms: string[]) {
  const space = spaceOf(chat);
  const fields: [string, number][] = [
    [titleOf(chat).toLowerCase(), 100],
    [(space ? spaceName(space) : "No project").toLowerCase(), 40],
    [branchOf(chat).toLowerCase(), 30],
    [(deviceName(chat.deviceId) ?? "").toLowerCase(), 20],
  ];
  let total = 0;
  for (const term of terms) {
    let best: number | null = null;
    for (const [haystack, weight] of fields) {
      const score = fieldScore(haystack, term, weight);
      if (score !== null && (best === null || score > best)) best = score;
    }
    if (best === null) return null;
    total += best;
  }
  return isArchived(chat) ? total / 2 : total;
}

// ---- layout ----

export type Zone = { kind: "pinned" } | { kind: "section"; id: string } | { kind: "regular" };

interface Placed {
  key: string;
  /** Space above the item's content, so the virtualizer can use exact sizes. */
  top: number;
  size: number;
}

export type Item =
  | (Placed & { kind: "pinned"; open: boolean; count: number })
  | (Placed & { kind: "section"; section: SidebarSection; count: number })
  | (Placed & { kind: "group"; groupKey: string; label: string; project: boolean; spaceId: string | null; open: boolean; count: number })
  | (Placed & { kind: "row"; chatId: string; indicator: Indicator; nested: boolean; twoLine: boolean; zone: Zone })
  | (Placed & { kind: "note"; text: string; zone: Zone | null });

export type RowItem = Extract<Item, { kind: "row" }>;

const HEADER = 28;
const ROW_ONE = 28;
const ROW_TWO = 44;
const BODY_INSET = 4;
const LIST_GAP = 2;
const SECTION_GAP = 12;
const PROJECT_GAP = 2;
const THREADS_GAP = 10;

export interface Layout {
  items: Item[];
  working: number;
  mode: "list" | "search";
  total: number;
}

export interface LayoutInput {
  at: number;
  query: string;
  dragging: boolean;
}

export function layout(input: LayoutInput): Layout {
  const at = input.at;
  const org = organization();
  const filter = option("spaceFilter");
  const all = Object.values(state.chats);
  const topLevel = all.filter((chat) => !chat.parentChatId);

  // A row stands for its whole thread: a sibling conversation working lights the row too.
  const live = topLevel.filter((chat) => !isArchived(chat) && (!chat.spaceId || !!state.spaces[chat.spaceId]));
  const status = new Map<string, Indicator>();
  for (const chat of live) {
    const root = chat.threadId ?? chat.id;
    status.set(root, louder(status.get(root), indicatorFor(chat, at)));
  }
  const roots = live.filter((chat) => !chat.threadId);
  let working = 0;
  for (const chat of roots) if (status.get(chat.id) === "working") working++;

  const items: Item[] = [];
  const row = (chat: Chat, top: number, twoLine: boolean, nested: boolean, zone: Zone) =>
    items.push({
      kind: "row",
      key: `c:${chat.id}`,
      top,
      size: top + (twoLine ? ROW_TWO : ROW_ONE),
      chatId: chat.id,
      indicator: status.get(chat.id) ?? indicatorFor(chat, at),
      nested,
      twoLine,
      zone,
    });

  const query = input.query.trim();
  if (query) {
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    const scored = topLevel
      .filter((chat) => !chat.threadId)
      .map((chat) => ({ chat, score: scoreChat(chat, terms) }))
      .filter((entry): entry is { chat: Chat; score: number } => entry.score !== null)
      .sort((a, b) => b.score - a.score || activityAt(b.chat) - activityAt(a.chat))
      .slice(0, 100);
    scored.forEach(({ chat }, index) => row(chat, index ? LIST_GAP : 0, true, false, { kind: "regular" }));
    if (!scored.length) items.push({ kind: "note", key: "empty", top: 0, size: 40, text: "No matching threads", zone: null });
    return { items, working, mode: "search", total: scored.length };
  }

  const shown = roots.filter((chat) => inProjectFilter(chat, filter)).sort(compare(option("sort")));
  const compact = option("compact") && org !== "threads";
  const showLabel = option("showLocation") || org === "threads";
  const twoLine = (nested: boolean) => !(compact || nested || !showLabel);

  const byId = new Map(shown.map((chat) => [chat.id, chat]));
  const pins = [...new Set(pinnedIds())].filter((id) => byId.has(id));
  const pinned = new Set(pins);
  const custom = sections();
  const inSection = new Set<string>();
  for (const section of custom) for (const id of section.sessionIds) if (byId.has(id) && !pinned.has(id)) inSection.add(id);
  const regular = shown.filter((chat) => !pinned.has(chat.id) && !inSection.has(chat.id));

  if (pins.length || input.dragging) {
    const open = option("pinnedOpen");
    items.push({ kind: "pinned", key: "pinned", top: 0, size: HEADER, open, count: pins.length });
    if (open) pins.forEach((id, index) => row(byId.get(id)!, index ? LIST_GAP : BODY_INSET, twoLine(false), false, { kind: "pinned" }));
  }

  for (const section of custom) {
    const members = section.sessionIds.filter((id) => byId.has(id) && !pinned.has(id));
    items.push({ kind: "section", key: `s:${section.id}`, top: SECTION_GAP, size: SECTION_GAP + HEADER, section, count: members.length });
    if (section.collapsed) continue;
    const zone: Zone = { kind: "section", id: section.id };
    if (!members.length) items.push({ kind: "note", key: `s:${section.id}:empty`, top: BODY_INSET, size: BODY_INSET + 40, text: "Drop sessions here", zone });
    members.forEach((id, index) => row(byId.get(id)!, index ? LIST_GAP : BODY_INSET, twoLine(false), false, zone));
  }

  const regularZone: Zone = { kind: "regular" };
  if (org === "threads") {
    const lead = items.length ? THREADS_GAP : 0;
    regular.forEach((chat, index) => row(chat, index ? LIST_GAP : lead, twoLine(false), false, regularZone));
    if (!regular.length && input.dragging && items.length)
      items.push({ kind: "note", key: "unpin", top: lead, size: lead + 48, text: "Drop here to unpin", zone: regularZone });
  } else {
    const groups: { key: string; label: string; chats: Chat[] }[] = [];
    for (const chat of regular) {
      const group =
        org === "projects"
          ? projectGroup(chat)
          : { key: `device:${chat.deviceId}`, name: deviceName(chat.deviceId) ?? "Unknown device" };
      const existing = groups.find((g) => g.key === group.key);
      if (existing) existing.chats.push(chat);
      else groups.push({ key: group.key, label: group.name, chats: [chat] });
    }
    if (org === "projects" && filter && state.spaces[filter]) {
      // The filtered project keeps its heading, so the filter reads as active.
      const space = state.spaces[filter]!;
      const key = projectKey(space);
      if (!groups.some((g) => g.key === key)) groups.push({ key, label: spaceName(representative(space)), chats: [] });
    }
    if (org === "devices" && state.localDeviceId) {
      const local = groups.findIndex((g) => g.key === `device:${state.localDeviceId}`);
      if (local > 0) groups.unshift(...groups.splice(local, 1));
    }
    const project = org === "projects";
    for (const group of groups) {
      const open = !isCollapsed(group.key);
      const gap = project ? PROJECT_GAP : SECTION_GAP;
      const first = group.chats[0];
      const spaceId = project
        ? (first?.spaceId && state.spaces[first.spaceId] ? first.spaceId : projectMembers(group.key)[0]?.id ?? null)
        : null;
      items.push({ kind: "group", key: `g:${group.key}`, top: gap, size: gap + HEADER, groupKey: group.key, label: group.label, project, spaceId, open, count: group.chats.length });
      if (!open) continue;
      if (!group.chats.length)
        items.push({ kind: "note", key: `g:${group.key}:empty`, top: BODY_INSET, size: BODY_INSET + 28, text: "No threads in this project", zone: regularZone });
      group.chats.forEach((chat, index) => row(chat, index ? LIST_GAP : BODY_INSET, twoLine(project), project, regularZone));
    }
  }

  if (!items.length) items.push({ kind: "note", key: "empty", top: 0, size: 40, text: "No sessions yet", zone: null });
  return { items, working, mode: "list", total: shown.length };
}

/** Every project once, named by its representative, for the project filter. */
export function projectList() {
  const seen = new Map<string, Space>();
  for (const space of Object.values(state.spaces)) {
    const key = projectKey(space);
    if (!seen.has(key)) seen.set(key, representative(space));
  }
  return [...seen.entries()]
    .map(([key, space]) => ({ key, spaceId: projectMembers(key)[0]?.id ?? space.id, name: spaceName(space) }))
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.key.localeCompare(b.key));
}

/** Remotes that can name a project's GitHub owner, from every checkout. */
export function projectRemotes(space: Space) {
  return projectMembers(projectKey(space)).flatMap((member) => [member.remoteRepository, member.repositoryId]);
}

/** The space whose project a row's avatar shows. */
export const avatarSpace = (chat: Chat) => {
  const space = spaceOf(chat);
  return space ? representative(space) : undefined;
};

export { deviceOnline };

export const projectOffline = (space: Space, at: number) => !projectMembers(projectKey(space)).some((m) => deviceOnline(m.deviceId, at));
