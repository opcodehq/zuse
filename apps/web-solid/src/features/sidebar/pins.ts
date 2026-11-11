// Pins and custom sections. The engine's WatchSidebarPreferences row is the truth; queued
// intents are an overlay projected onto it until the engine acknowledges them, in order.
import { batch } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { toast } from "@/ui/primitives";

export interface SidebarSection {
  id: string;
  name: string;
  sessionIds: string[];
  collapsed: boolean;
}

interface PreferencesState {
  revision: number;
  synced: boolean;
  initialized: boolean;
  pinnedSessionIds: string[];
  sections: SidebarSection[];
}

export type SectionChange =
  | { action: "create"; id: string; name: string }
  | { action: "rename"; id: string; name: string }
  | { action: "collapse"; id: string; collapsed: boolean }
  | { action: "delete"; id: string }
  | { action: "assign"; sessionId: string; sectionId: string | null; after?: string | null; before?: string | null };

export type PinChange =
  | { action: "pin"; sessionId: string; after: string | null; before: string | null }
  | { action: "move"; sessionId: string; after: string | null; before: string | null }
  | { action: "unpin"; sessionId: string }
  | { action: "section"; change: SectionChange };

const [store, setStore] = createStore<{ confirmed: PreferencesState | null; queue: PinChange[] }>({ confirmed: null, queue: [] });
let sending = false;
let epoch = 0;
// Revisions are per engine attachment, so the first frame after a reconnect always wins.
let fresh = true;

function normalize(value: unknown): PreferencesState | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Partial<PreferencesState>;
  return {
    revision: typeof raw.revision === "number" ? raw.revision : 0,
    synced: !!raw.synced,
    initialized: !!raw.initialized,
    pinnedSessionIds: Array.isArray(raw.pinnedSessionIds) ? raw.pinnedSessionIds.filter((id) => typeof id === "string") : [],
    // Sections serialize their members as `session_ids` (crates/protocol SidebarSection).
    sections: Array.isArray(raw.sections)
      ? (raw.sections as (SidebarSection & { session_ids?: string[] })[]).map((s) => ({
          id: s.id,
          name: s.name,
          sessionIds: s.sessionIds ?? s.session_ids ?? [],
          collapsed: !!s.collapsed,
        }))
      : [],
  };
}

/** Watch frames and mutation replies share one ordered revision; older frames never win. */
function accept(value: unknown) {
  const next = normalize(value);
  if (!next) return;
  if (!fresh && store.confirmed && next.revision < store.confirmed.revision) return;
  fresh = false;
  setStore("confirmed", reconcile(next));
}

onConnected((current) => {
  epoch = current;
  sending = false;
  fresh = true;
  setStore("queue", []);
  return rpc.subscribe(M.WATCH_SIDEBAR_PREFERENCES, {}, accept);
});

function insertIndex(ids: string[], after?: string | null, before?: string | null) {
  const b = before ? ids.indexOf(before) : -1;
  if (b >= 0) return b;
  const a = after ? ids.indexOf(after) : -1;
  return a >= 0 ? a + 1 : ids.length;
}

/** Port of SidebarPinChange::project: rebase a pending intent on the latest confirmed list. */
function projectPins(change: PinChange, ids: string[]) {
  if (change.action === "section") {
    if (change.change.action === "assign") {
      const id = change.change.sessionId;
      return ids.filter((v) => v !== id);
    }
    return ids;
  }
  const id = change.sessionId;
  if (change.action === "move" && !ids.includes(id)) return ids;
  const rest = ids.filter((v) => v !== id);
  if (change.action === "unpin") return rest;
  rest.splice(insertIndex(rest, change.after, change.before), 0, id);
  return rest;
}

function projectSections(change: PinChange, sections: SidebarSection[]): SidebarSection[] {
  if (change.action === "pin") return sections.map((s) => ({ ...s, sessionIds: s.sessionIds.filter((id) => id !== change.sessionId) }));
  if (change.action !== "section") return sections;
  const c = change.change;
  switch (c.action) {
    case "create":
      return sections.some((s) => s.id === c.id) ? sections : [...sections, { id: c.id, name: c.name, sessionIds: [], collapsed: false }];
    case "rename":
      return sections.map((s) => (s.id === c.id ? { ...s, name: c.name } : s));
    case "collapse":
      return sections.map((s) => (s.id === c.id ? { ...s, collapsed: c.collapsed } : s));
    case "delete":
      return sections.filter((s) => s.id !== c.id);
    case "assign":
      return sections.map((s) => {
        const ids = s.sessionIds.filter((id) => id !== c.sessionId);
        if (s.id !== c.sectionId) return { ...s, sessionIds: ids };
        ids.splice(insertIndex(ids, c.after, c.before), 0, c.sessionId);
        return { ...s, sessionIds: ids, collapsed: false };
      });
  }
}

/** Pinned chat ids as the user sees them: confirmed state plus queued intents. */
export function pinnedIds(): string[] {
  let ids = store.confirmed?.pinnedSessionIds ?? [];
  for (const change of store.queue) ids = projectPins(change, ids);
  return ids;
}

export function sections(): SidebarSection[] {
  let list = store.confirmed?.sections ?? [];
  for (const change of store.queue) list = projectSections(change, list);
  // Pins win over section membership, matching the registry.
  const pins = new Set(pinnedIds());
  return list.map((s) => ({ ...s, sessionIds: s.sessionIds.filter((id) => !pins.has(id)) }));
}

export const preferencesLoaded = () => store.confirmed !== null;

function pump() {
  if (sending) return;
  const next = store.queue[0];
  if (!next) return;
  sending = true;
  const started = epoch;
  rpc
    .call<{ sidebarPreferences?: unknown }>(M.MUTATE, { op: "changeSidebarPin", change: next })
    .then(
      (reply) => {
        if (started !== epoch) return;
        batch(() => {
          accept(reply?.sidebarPreferences);
          setStore("queue", (queue) => queue.slice(1));
        });
      },
      (error: unknown) => {
        if (started !== epoch) return;
        if (error instanceof RpcError && error.outcomeUnknown) {
          // Its order relative to later intents is unknown; drop them rather than guess.
          setStore("queue", []);
          toast("Couldn't confirm sidebar changes. Queued edits were cancelled.", "danger");
        } else {
          setStore("queue", (queue) => queue.slice(1));
          toast(`Couldn't save sidebar changes: ${error instanceof Error ? error.message : String(error)}`, "danger");
        }
      },
    )
    .finally(() => {
      if (started !== epoch) return;
      sending = false;
      pump();
    });
}

export function changePins(change: PinChange) {
  if (!rpc.connected) {
    toast("Engine not connected. Sidebar was not changed.", "danger");
    return;
  }
  setStore("queue", (queue) => [...queue, change]);
  pump();
}

export function setPinned(chatId: string, pinned: boolean) {
  const pins = pinnedIds();
  if (pinned === pins.includes(chatId)) return;
  changePins(pinned ? { action: "pin", sessionId: chatId, after: null, before: pins[0] ?? null } : { action: "unpin", sessionId: chatId });
}

/** Place `chatId` among the pins: before `before`, else after `after`. */
export function placePin(chatId: string, after: string | null, before: string | null) {
  const pinned = pinnedIds().includes(chatId);
  if (after === chatId || before === chatId) return;
  changePins({ action: pinned ? "move" : "pin", sessionId: chatId, after, before });
}

export function changeSection(change: SectionChange) {
  changePins({ action: "section", change });
}
