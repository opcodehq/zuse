// Host path handling for folder browsers, ported from crates/ui/src/pickers.rs.
// POSIX paths and drive-rooted Windows paths (remote hosts) are supported.

const DRIVE = /^([A-Za-z]:)(?:[\\/]|$)/;

const windowsRoot = (path: string) => DRIVE.exec(path)?.[1] ?? null;

export const isWindowsPath = (path: string) => windowsRoot(path) !== null;

function windowsParts(path: string): { root: string; rest: string } | null {
  const root = windowsRoot(path);
  if (!root) return null;
  const rest = path.slice(root.length).replace(/\//g, "\\").replace(/^\\+|\\+$/g, "");
  return { root: `${root.toUpperCase()}\\`, rest };
}

/** Lexical identity of a folder: POSIX spelling kept, Windows drives normalised. */
export function pathKey(path: string): string {
  const windows = windowsParts(path);
  if (windows) return (windows.root + windows.rest).toLowerCase();
  return path.length > 1 ? path.replace(/\/+$/, "") : path;
}

export const samePath = (a: string, b: string) => pathKey(a) === pathKey(b);

export function childPath(base: string, name: string): string {
  if (/[\\/]$/.test(base)) return base + name;
  return isWindowsPath(base) ? `${base}\\${name}` : `${base}/${name}`;
}

/** Parent of an absolute path; null at the filesystem root. */
export function parentPath(path: string): string | null {
  const windows = windowsParts(path);
  if (windows) {
    if (!windows.rest) return null;
    const at = windows.rest.lastIndexOf("\\");
    return windows.root + (at < 0 ? "" : windows.rest.slice(0, at));
  }
  const trimmed = path.replace(/\/+$/, "");
  if (!trimmed) return null;
  const at = trimmed.lastIndexOf("/");
  if (at === 0) return "/";
  return at > 0 ? trimmed.slice(0, at) : null;
}

export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const at = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return (at >= 0 ? trimmed.slice(at + 1) : trimmed) || path;
}

/** `(name, full path)` for every segment, root first. */
export function breadcrumbs(path: string): [string, string][] {
  const windows = windowsParts(path);
  if (windows) {
    const out: [string, string][] = [[windows.root, windows.root]];
    let acc = windows.root;
    for (const segment of windows.rest.split("\\").filter(Boolean)) {
      acc = childPath(acc, segment);
      out.push([segment, acc]);
    }
    return out;
  }
  const out: [string, string][] = [["/", "/"]];
  let acc = "";
  for (const segment of path.split("/").filter(Boolean)) {
    acc += `/${segment}`;
    out.push([segment, acc]);
  }
  return out;
}

/** Segment-aware "is `path` at or under `base`". */
export function pathUnder(path: string, base: string): boolean {
  const p = pathKey(path);
  const b = pathKey(base).replace(/[\\/]+$/, "");
  return !b || p === b || p.startsWith(`${b}/`) || p.startsWith(`${b}\\`);
}

/** A query shaped like a path (absolute, home-relative or drive-rooted) rather than a name filter. */
export const isTypedPath = (query: string) => query.startsWith("/") || query.startsWith("~") || isWindowsPath(query);

/** Resolve a typed path query against the host's home; null when `~` cannot expand yet. */
export function typedPathTarget(query: string, home: string | null): string | null {
  const text = query.trim();
  const windows = windowsParts(text);
  if (windows) return windows.root + windows.rest;
  if (text.startsWith("~")) {
    const rest = text.slice(1);
    if (rest && !rest.startsWith("/") && !rest.startsWith("\\")) return null;
    if (!home) return null;
    const tail = rest.replace(/^[\\/]+|[\\/]+$/g, "");
    if (!tail) return home;
    return isWindowsPath(home) ? childPath(home, tail.replace(/\//g, "\\")) : childPath(home.replace(/\/+$/, ""), tail);
  }
  if (text.startsWith("/")) return text.length > 1 ? text.replace(/\/+$/, "") : "/";
  return null;
}

/** Length of `name`'s prefix matching `query` case-insensitively, or null. */
export function completionPrefixLength(name: string, query: string): number | null {
  if (query.length > name.length) return null;
  return name.slice(0, query.length).toLowerCase() === query.toLowerCase() ? query.length : null;
}

/** The folder a `name/` query descends into: exact, then full-length prefix, then a unique prefix. */
export function segmentTarget(names: string[], query: string): number | null {
  const exact = names.indexOf(query);
  if (exact >= 0) return exact;
  const whole = names.findIndex((n) => completionPrefixLength(n, query) === n.length);
  if (whole >= 0) return whole;
  const hits = names.map((n, i) => (completionPrefixLength(n, query) !== null ? i : -1)).filter((i) => i >= 0);
  return hits.length === 1 ? hits[0]! : null;
}

/** Prefix matches first, then substring matches, stable within each rank. */
export function rankFilter<T>(query: string, items: readonly T[], label: (item: T) => string): T[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...items];
  const ranked: [number, number, T][] = [];
  items.forEach((item, index) => {
    const text = label(item).toLowerCase();
    if (text.startsWith(q)) ranked.push([0, index, item]);
    else if (text.includes(q)) ranked.push([1, index, item]);
  });
  return ranked.sort((a, b) => a[0] - b[0] || a[1] - b[1]).map(([, , item]) => item);
}

/** Every whitespace-separated word appears in the text. */
export function matchesWords(query: string, text: string): boolean {
  const lower = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => lower.includes(word));
}

export function looksLikeGitUrl(text: string): boolean {
  const t = text.trim();
  return !!t && !/\s/.test(t) && (t.includes("://") || (t.startsWith("git@") && t.includes(":")));
}

export function cloneName(url: string): string {
  const parts = url.trim().replace(/\/+$/, "").split(/[/:]/);
  return (parts[parts.length - 1] ?? "").replace(/\.git$/, "");
}

/** `owner/name` from a clone URL. */
export function repositoryLabel(url: string): string {
  const parts = url.replace(/\/+$/, "").replace(/\.git$/, "").split(/[/:]/);
  return parts.slice(-2).join("/");
}
