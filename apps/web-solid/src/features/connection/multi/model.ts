// Public computer recipes and cached sidebar metadata. Pairing capabilities stay in the existing vault.
import { createSignal } from "solid-js";
import type { Chat, Space, Session } from "@/core/protocol";
export type Recipe = { kind: "owner" } | { kind: "paired"; pairId: string } | { kind: "account"; target: { environmentId: string; expectedSubject: string; label: string }; controller: Recipe };
export interface Snapshot {
  deviceId: string | null;
  label: string;
  connected: boolean;
  synced: boolean;
  selected: string | null;
  spaces: Space[];
  chats: Chat[];
  sessions: Record<string, Session>;
  dirty: boolean;
  catalog?: { subject: string; environments: string[] };
}
export interface Computer { id: string; recipe: Recipe; label: string; snapshot?: Snapshot }
export const FRAME_PARAM = "computer-frame";
export const frameId = () => { try { return new URL(location.href).searchParams.get(FRAME_PARAM); } catch { return null; } };
export const inComputerFrame = () => !!frameId() && typeof window !== "undefined" && window.parent !== window;
const KEY = "zuse.computer-workspaces.v1";
const ACTIVE = "zuse.computer-workspace-active.v1";
const STAGING = "zuse.computer-pairing.v1:";
export function validRecipe(value: unknown, depth = 0): value is Recipe {
  if (!value || typeof value !== "object" || depth > 3) return false;
  const row = value as Record<string, unknown>;
  if (row.kind === "owner") return true;
  if (row.kind === "paired") return typeof row.pairId === "string" && /^[a-f0-9]{32}$/.test(row.pairId);
  if (row.kind !== "account" || !validRecipe(row.controller, depth + 1) || !row.target || typeof row.target !== "object") return false;
  const target = row.target as Record<string, unknown>;
  return [target.environmentId, target.expectedSubject, target.label].every(value => typeof value === "string" && value.length > 0 && value.length <= 256 && !/[\p{Cc}]/u.test(value));
}
export function recipeKey(recipe: Recipe): string {
  return recipe.kind === "owner" ? "owner" : recipe.kind === "paired" ? `paired:${recipe.pairId}` : `account:${recipe.target.expectedSubject}:${recipe.target.environmentId}`;
}
function load(): Computer[] {
  try {
    const values: unknown = JSON.parse(sessionStorage.getItem(KEY + ".pending") ?? localStorage.getItem(KEY) ?? "[]");
    if (!Array.isArray(values) || values.length > 64) return [];
    const seen = new Set<string>();
    return values.filter((value): value is Computer => {
      if (!value || typeof value !== "object" || typeof value.id !== "string" || value.id.length > 1024 || typeof value.label !== "string" || !validRecipe(value.recipe) || seen.has(value.id)) return false;
      seen.add(value.id); return true;
    }).map(value => ({ ...value, snapshot: value.snapshot && Array.isArray(value.snapshot.spaces) && Array.isArray(value.snapshot.chats) ? { ...value.snapshot, connected: false, dirty: false } : undefined }));
  } catch { return []; }
}
const initialComputers = load();
const [computers, setComputers] = createSignal<Computer[]>(initialComputers);
const [active, setActive] = createSignal((() => { try { return sessionStorage.getItem(ACTIVE) ?? "primary"; } catch { return "primary"; } })());
const [primaryRecipe, setPrimaryRecipe] = createSignal<Recipe>(initialComputers.find(value => value.id === "primary")?.recipe ?? { kind: "owner" });
export { computers, active, primaryRecipe };
export const multiMode = () => !inComputerFrame() && computers().some(value => value.id !== "primary");
export function activateComputer(id: string) {
  setActive(id);
  try { sessionStorage.setItem(ACTIVE, id); } catch { /* session only */ }
}
let persistTimer: ReturnType<typeof setTimeout> | undefined;
function persist(values: Computer[], immediate = false) {
  clearTimeout(persistTimer);
  const write = () => {
  try { sessionStorage.setItem(KEY + ".pending", JSON.stringify(values)); } catch { /* memory only */ }
  try { localStorage.setItem(KEY, JSON.stringify(values.filter(value => value.id === "primary" || value.snapshot?.synced).map(value => ({ ...value, snapshot: value.snapshot ? { ...value.snapshot, dirty: false, sessions: {} } : undefined })))); } catch { /* memory only */ }
  };
  if (immediate) write(); else persistTimer = setTimeout(write, 200);
}
export function notePrimaryRecipe(recipe: Recipe) {
  if (inComputerFrame()) return;
  setPrimaryRecipe(recipe);
  setComputers(values => {
    const existing = values.find(value => value.id === "primary");
    const next = [{ ...existing, id: "primary", recipe, label: existing?.label ?? "This computer" }, ...values.filter(value => value.id !== "primary")];
    persist(next, true); return next;
  });
}
export function addRecipe(recipe: Recipe, label: string): string {
  if (!validRecipe(recipe)) throw new Error("Invalid computer connection");
  const id = recipeKey(recipe);
  const existing = computers().find(value => recipeKey(value.recipe) === id);
  if (existing) { activateComputer(existing.id); return existing.id; }
  if (computers().length >= 64) throw new Error("Remove an unused computer before adding another.");
  setComputers(values => { const next = [...values, { id, recipe, label }]; persist(next, true); return next; });
  activateComputer(id);
  return id;
}
export function applyComputerSnapshot(id: string, snapshot: Snapshot) {
  if (!snapshot || !Array.isArray(snapshot.chats) || !Array.isArray(snapshot.spaces) || snapshot.chats.length > 100000 || snapshot.spaces.length > 10000) return;
  setComputers(values => {
    const current = values.find(value => value.id === id);
    if (!current) return values;
    const duplicate = snapshot.deviceId && snapshot.synced ? values.find(value => value.id !== id && value.snapshot?.deviceId === snapshot.deviceId && value.snapshot.synced && (value.id === "primary" || values.indexOf(value) < values.indexOf(current))) : undefined;
    if (duplicate) {
      if (active() === id) activateComputer(duplicate.id);
      const next = values.filter(value => value.id !== id); persist(next, true); return next;
    }
    const next = values.map(value => value.id === id ? { ...value, label: snapshot.label || value.label, snapshot } : value);
    persist(next); return next;
  });
}
export function removeRecipe(id: string) {
  setComputers(values => { const next = values.filter(value => value.id !== id || id === "primary"); persist(next, true); return next; });
  if (active() === id) activateComputer("primary");
}
export function stagedPairing(pairId: string): string | null { try { return sessionStorage.getItem(STAGING + pairId); } catch { return null; } }
export function stagePairing(pairId: string, link: string) { try { sessionStorage.setItem(STAGING + pairId, link); } catch { throw new Error("Browser storage is required to add a computer."); } }
export function savedPrimaryRecipe(): Recipe | null { return computers().find(value => value.id === "primary")?.recipe ?? null; }
export function frameRecipe(id: string): Recipe | null { return computers().find(value => value.id === id)?.recipe ?? null; }
export function projectKey(space: Space, computer: string): string {
  return space.repositoryId ? `repo:${space.repositoryId}` : space.remoteRepository ? `remote:${space.remoteRepository}` : `computer:${computer}:project:${space.id}`;
}
export interface ThreadTarget { computer: string; chat: string }
export interface ProjectGroup { key: string; label: string; parts: { computer: Computer; space: Space | null; chats: Chat[] }[] }
export function projectGroups(values: Computer[], query = ""): ProjectGroup[] {
  const groups = new Map<string, ProjectGroup>();
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  for (const computer of values) {
    const snapshot = computer.snapshot;
    if (!snapshot) continue;
    const spaces = snapshot.spaces.filter(space => !snapshot.deviceId || space.deviceId === snapshot.deviceId);
    const chats = snapshot.chats.filter(chat => !chat.parentChatId && !chat.archived && (!snapshot.deviceId || chat.deviceId === snapshot.deviceId));
    const candidates: (Space | null)[] = [...spaces, ...(chats.some(chat => !chat.spaceId) ? [null] : [])];
    for (const space of candidates) {
      const key = space ? projectKey(space, computer.id) : `home:${computer.id}`;
      const label = space ? space.name || space.path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() || space.path : "No project";
      const matching = chats.filter(chat => (space ? chat.spaceId === space.id : !chat.spaceId) && terms.every(term => `${chat.title ?? "New session"} ${label} ${chat.branch ?? ""} ${computer.label}`.toLowerCase().includes(term))).sort((a, b) => (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt));
      if (terms.length && !matching.length) continue;
      const group = groups.get(key) ?? { key, label, parts: [] };
      group.parts.push({ computer, space, chats: matching }); groups.set(key, group);
    }
  }
  return [...groups.values()];
}
export function nextThread(order: ThreadTarget[], current: ThreadTarget | null, forward: boolean): ThreadTarget | undefined {
  if (!order.length) return;
  const index = current ? order.findIndex(value => value.computer === current.computer && value.chat === current.chat) : -1;
  return order[index < 0 ? forward ? 0 : order.length - 1 : (index + (forward ? 1 : -1) + order.length) % order.length];
}
