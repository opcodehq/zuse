// The Zapper's files as a tree, mirroring crates/engine/src/zappers/files.rs: folders are the
// parents of live files plus the folders people created; reserved root files always have a row.
import type { ZapperFile, ZapperFileVersion } from "@/core/zapper-types";

/** Root files with a fixed meaning: editable and restorable, never moved or deleted. */
export const RESERVED: Record<string, string> = {
  "AGENTS.md": "Your standing rules for the coordinator. It follows them from its next turn; only people edit this file.",
  "WORKERS.md": "Rules every thread gets with its prompt, from its next turn.",
  "NOTES.md": "Notes the coordinator keeps between turns. You can edit them; it reads them as context, never as instructions.",
  "INSTRUCTIONS.md": "The coordinator's purpose and working rules, in its own words.",
  "DIGEST.md": "The coordinator's short status summary for you.",
};
export const isReserved = (path: string) => Object.hasOwn(RESERVED, path);

/** A reserved file's role, shown beside its name so a look-alike can't pass for it. Exact match. */
export function reservedRole(path: string): string | null {
  const roles: Record<string, string> = {
    "AGENTS.md": "Coordinator rules",
    "WORKERS.md": "Thread rules",
    "NOTES.md": "Notes",
    "INSTRUCTIONS.md": "Purpose",
    "DIGEST.md": "Digest",
  };
  return Object.hasOwn(roles, path) ? roles[path]! : null;
}

/** A file an older engine wrote at a folder's path is `conflict`: both rows show. */
export type FileNode = { kind: "file"; name: string; path: string; file: ZapperFile | null; conflict?: boolean };
export type FolderNode = { kind: "folder"; name: string; path: string; children: TreeNode[] };
export type TreeNode = FileNode | FolderNode;
export type TreeRow = { node: TreeNode; depth: number; expanded: boolean };
/** Identifies a row: a conflicting file and its folder share a path. */
export const rowKey = (node: TreeNode) => (node.kind === "folder" ? "d:" : "f:") + node.path;

const byName = (a: TreeNode, b: TreeNode) =>
  a.kind !== b.kind ? (a.kind === "folder" ? -1 : 1) : a.name.localeCompare(b.name, undefined, { sensitivity: "base" });

/** Newest version per path, leaving out deleted files. `files` may be a whole history. */
export function latestFiles(files: readonly ZapperFile[]): ZapperFile[] {
  const latest = new Map<string, ZapperFile>();
  for (const file of files) if ((latest.get(file.path)?.version ?? 0) < file.version) latest.set(file.path, file);
  return [...latest.values()].filter((file) => !file.deleted);
}

export function buildTree(files: readonly ZapperFile[], folders: readonly string[] = []): TreeNode[] {
  const root: FolderNode = { kind: "folder", name: "", path: "", children: [] };
  const folderAt = new Map<string, FolderNode>([["", root]]);
  const ensure = (path: string): FolderNode => {
    const found = folderAt.get(path);
    if (found) return found;
    const cut = path.lastIndexOf("/");
    const parent = ensure(cut < 0 ? "" : path.slice(0, cut));
    const node: FolderNode = { kind: "folder", name: path.slice(cut + 1), path, children: [] };
    parent.children.push(node);
    folderAt.set(path, node);
    return node;
  };
  for (const folder of folders) ensure(folder);
  const live = latestFiles(files);
  const placed: FileNode[] = [];
  for (const file of live) {
    const cut = file.path.lastIndexOf("/");
    const node: FileNode = { kind: "file", name: file.path.slice(cut + 1), path: file.path, file };
    ensure(cut < 0 ? "" : file.path.slice(0, cut)).children.push(node);
    placed.push(node);
  }
  for (const node of placed) if (folderAt.has(node.path)) node.conflict = true;
  for (const path of Object.keys(RESERVED))
    if (!live.some((file) => file.path === path)) root.children.push({ kind: "file", name: path, path, file: null });
  const sort = (node: FolderNode) => {
    node.children.sort(byName);
    for (const child of node.children) if (child.kind === "folder") sort(child);
  };
  sort(root);
  return root.children;
}

/** The rows a tree shows with `expanded` folders open. */
export function visibleRows(nodes: readonly TreeNode[], expanded: ReadonlySet<string>, depth = 0): TreeRow[] {
  const rows: TreeRow[] = [];
  for (const node of nodes) {
    const open = node.kind === "folder" && expanded.has(node.path);
    rows.push({ node, depth, expanded: open });
    if (node.kind === "folder" && open) rows.push(...visibleRows(node.children, expanded, depth + 1));
  }
  return rows;
}

/** Every live file under `folder` at its newest version: what a folder move or delete acts on. */
export function filesUnder(files: readonly ZapperFile[], folder: string): ZapperFileVersion[] {
  return latestFiles(files)
    .filter((file) => file.path.startsWith(folder + "/"))
    .map((file) => ({ path: file.path, version: file.version }))
    .sort((a, b) => a.path.localeCompare(b.path));
}

/** Created folders strictly under `folder`. */
export const foldersUnder = (folders: readonly string[], folder: string) => folders.filter((path) => path.startsWith(folder + "/"));

export const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");

/** The folders to open so `path` shows. */
export const ancestors = (path: string) => {
  const out: string[] = [];
  for (let i = path.indexOf("/"); i >= 0; i = path.indexOf("/", i + 1)) out.push(path.slice(0, i));
  return out;
};

/** A quick check before sending; the engine has the final word. */
export function pathProblem(path: string): string | null {
  if (!path) return "Enter a name.";
  if (path.length > 256) return "Paths are at most 256 characters.";
  if (path.startsWith("/") || path.endsWith("/")) return "Paths can't start or end with /.";
  for (const part of path.split("/")) {
    if (!part) return "Paths can't contain empty folder names.";
    if (part === "." || part === "..") return "Paths can't use . or .. folders.";
    if (part.includes("\\")) return "Use / between folders.";
    if (part.trim() !== part) return "Names can't start or end with spaces.";
    if (/\p{Cc}/u.test(part)) return "Names can't contain control characters.";
  }
  if (path === "guide" || path.startsWith("guide/")) return "guide/ holds the coordinator's read-only guides.";
  return null;
}
