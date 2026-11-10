// Paired computers this browser has reached. Each link is a device credential: it stays in this
// origin's storage, never in the URL, logs or settings, and the host can revoke it at any time.
// Keys match the earlier web client so saved computers carry over.
import { createStore } from "solid-js/store";

const SAVED = "zuse.paired-computers.v1";
export const SELECTED = "zuse.paired-target.v1";
export const ACCOUNT_SELECTED = "zuse.account-target.v1";
const ROUTES = "zuse.paired-routes.v1:";
/** Set after "Forget pairing" so this tab shows its own engine instead of jumping to another saved computer. */
const STAY = "zuse.paired-stay.v1";
const MAX_SAVED = 16;

export interface SavedComputer {
  id: string;
  origin: string;
  current: boolean;
}

function read(storage: () => Storage, key: string): string | null {
  try {
    return storage().getItem(key);
  } catch {
    return null;
  }
}
function write(storage: () => Storage, key: string, value: string | null) {
  try {
    if (value === null) storage().removeItem(key);
    else storage().setItem(key, value);
  } catch {
    // Storage can be unavailable (private windows); the pairing then lasts for this page only.
  }
}
const local = () => localStorage;
const tab = () => sessionStorage;

/** The public parts of an exported link, read without the WebAssembly module. Null when malformed. */
export function describeLink(link: string): { id: string; origin: string } | null {
  try {
    const url = new URL(link);
    if (!url.hash.startsWith("#pair=") || url.hash.length > 520) return null;
    const encoded = url.hash.slice(6).replace(/-/g, "+").replace(/_/g, "/");
    const wire = JSON.parse(atob(encoded.padEnd(Math.ceil(encoded.length / 4) * 4, "="))) as { v?: unknown; id?: unknown };
    if (wire.v !== 1 || typeof wire.id !== "string" || !/^[0-9a-f]{32}$/.test(wire.id)) return null;
    return { id: wire.id, origin: url.origin };
  } catch {
    return null;
  }
}

/** Newest first. Entries that no longer parse are dropped on the next write. */
function load(): string[] {
  try {
    const list = JSON.parse(read(local, SAVED) ?? "[]") as unknown;
    return Array.isArray(list) ? list.filter((link): link is string => typeof link === "string" && describeLink(link) !== null) : [];
  } catch {
    return [];
  }
}
function save(links: string[]) {
  write(local, SAVED, JSON.stringify(links.slice(0, MAX_SAVED)));
  publish();
}

export const selectedLink = () => read(tab, SELECTED);
export function selectLink(link: string | null) {
  write(tab, SELECTED, link);
  if (link) write(tab, STAY, null);
}

const [list, setList] = createStore<{ computers: SavedComputer[] }>({ computers: [] });
export const savedComputers = () => list.computers;

export function publish() {
  const current = describeLink(selectedLink() ?? "")?.id;
  setList(
    "computers",
    load().map((link) => {
      const described = describeLink(link)!;
      return { ...described, current: described.id === current };
    }),
  );
}

/** Saved only after a connection works, so a bad link never becomes the computer a new tab tries first. */
export function remember(link: string) {
  const id = describeLink(link)?.id;
  if (!id) return;
  save([link, ...load().filter((saved) => describeLink(saved)?.id !== id)]);
}

export function forget(id: string) {
  save(load().filter((saved) => describeLink(saved)?.id !== id));
  write(local, ROUTES + id, null);
}

/** The most recent computer, for a tab with no link or owner session. */
export function latest(): string | null {
  if (read(tab, STAY)) return null;
  return load()[0] ?? null;
}

export function linkFor(id: string): string | null {
  return load().find((link) => describeLink(link)?.id === id) ?? null;
}

/** Learned routes hold no secrets; losing them only costs connection speed. */
export const savedRoutes = (id: string) => read(local, ROUTES + id);
export const saveRoutes = (id: string, routes: string) => write(local, ROUTES + id, routes);

/** Reload this tab onto a saved or pasted computer. */
export function openInThisTab(link: string) {
  selectLink(link);
  write(tab, ACCOUNT_SELECTED, null);
  location.replace(location.pathname + location.search);
}

/** Forget the computer this tab uses and return to the page's own engine. */
export function forgetCurrent() {
  const id = describeLink(selectedLink() ?? "")?.id;
  if (id) forget(id);
  selectLink(null);
  write(tab, STAY, "1");
  location.replace(location.pathname + location.search);
}

export function readAccountTarget(): string | null {
  return read(tab, ACCOUNT_SELECTED);
}
export function writeAccountTarget(value: string | null) {
  write(tab, ACCOUNT_SELECTED, value);
}

publish();
