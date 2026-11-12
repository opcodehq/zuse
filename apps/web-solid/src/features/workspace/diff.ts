// Unified-diff parsing, ported from crates/ui/src/changes.rs (parse_patch, file_notices).

export type LineKind = "add" | "del" | "context" | "meta";

export interface DiffLine {
  kind: LineKind;
  oldNo: number | null;
  newNo: number | null;
  text: string;
}

export interface Hunk {
  header: string;
  lines: DiffLine[];
}

export interface FileDiff {
  path: string;
  oldPath: string | null;
  status: "modified" | "added" | "deleted" | "renamed";
  binary: boolean;
  hunks: Hunk[];
  additions: number;
  deletions: number;
  notices: string[];
}

function stripPrefix(path: string) {
  return path.startsWith("a/") || path.startsWith("b/") ? path.slice(2) : path;
}

function unquote(path: string) {
  if (!path.startsWith('"')) return path;
  try {
    return JSON.parse(path) as string;
  } catch {
    return path.slice(1, -1);
  }
}

function parseGitPaths(rest: string): [string, string] {
  const quoted = rest.match(/^("(?:[^"\\]|\\.)*") ("(?:[^"\\]|\\.)*")$/);
  if (quoted) return [stripPrefix(unquote(quoted[1]!)), stripPrefix(unquote(quoted[2]!))];
  // Unquoted paths: "a/x b/x" — split at the " b/" that makes both halves equal when possible.
  const half = rest.indexOf(" b/");
  if (half >= 0) return [stripPrefix(rest.slice(0, half)), stripPrefix(rest.slice(half + 1))];
  const parts = rest.split(" ");
  return [stripPrefix(parts[0] ?? rest), stripPrefix(parts[1] ?? rest)];
}

function parseHunkHeader(raw: string): [number, number] | null {
  const match = raw.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
  return match ? [Number(match[1]), Number(match[2])] : null;
}

export function parsePatch(patch: string): FileDiff[] {
  const files: FileDiff[] = [];
  let inHunk = false;
  let oldNo = 0;
  let newNo = 0;
  for (const raw of patch.split("\n")) {
    if (raw.startsWith("diff --git ")) {
      const [old, next] = parseGitPaths(raw.slice(11));
      files.push({ path: next, oldPath: old !== next ? old : null, status: old !== next ? "renamed" : "modified", binary: false, hunks: [], additions: 0, deletions: 0, notices: [] });
      inHunk = false;
      continue;
    }
    const file = files[files.length - 1];
    if (!file) continue;
    if (raw.startsWith("@@")) {
      const parsed = parseHunkHeader(raw);
      if (parsed) {
        [oldNo, newNo] = parsed;
        file.hunks.push({ header: raw, lines: [] });
        inHunk = true;
      }
      continue;
    }
    if (inHunk) {
      const hunk = file.hunks[file.hunks.length - 1]!;
      const marker = raw[0];
      const body = raw.slice(1);
      if (marker === "+") {
        file.additions++;
        hunk.lines.push({ kind: "add", oldNo: null, newNo: newNo++, text: body });
        continue;
      }
      if (marker === "-") {
        file.deletions++;
        hunk.lines.push({ kind: "del", oldNo: oldNo++, newNo: null, text: body });
        continue;
      }
      if (marker === " " || raw === "") {
        hunk.lines.push({ kind: "context", oldNo: oldNo++, newNo: newNo++, text: body });
        continue;
      }
      if (marker === "\\") {
        hunk.lines.push({ kind: "meta", oldNo: null, newNo: null, text: raw.replace(/^\\\s*/, "") });
        continue;
      }
      inHunk = false;
    }
    if (raw.startsWith("new file mode")) file.status = "added";
    else if (raw.startsWith("deleted file mode")) file.status = "deleted";
    else if (raw.startsWith("rename from ")) {
      file.status = "renamed";
      file.oldPath = raw.slice(12).trim();
    } else if (raw.startsWith("rename to ")) {
      file.status = "renamed";
      file.path = raw.slice(10).trim();
    } else if (raw.startsWith("Binary files") || raw.startsWith("GIT binary patch")) file.binary = true;
    else if (raw.startsWith("new mode ")) file.notices.push(`Mode changed to ${raw.slice(9).trim()}`);
    else if (raw.startsWith("+++ ")) {
      const next = raw.slice(4).trim();
      if (next === "/dev/null") file.status = "deleted";
      else if (!file.oldPath) file.path = stripPrefix(unquote(next));
    } else if (raw.startsWith("--- ") && raw.slice(4).trim() === "/dev/null") file.status = "added";
  }
  // A trailing newline in the patch leaves an empty context line at the end of the last hunk.
  for (const file of files) {
    const last = file.hunks[file.hunks.length - 1];
    const tail = last?.lines[last.lines.length - 1];
    if (tail && tail.kind === "context" && tail.text === "") last!.lines.pop();
  }
  return files;
}

export function fileNotices(file: FileDiff) {
  const notices: string[] = [];
  if (file.status === "added") notices.push("New file");
  else if (file.status === "deleted") notices.push("Deleted file");
  else if (file.status === "renamed") notices.push(`Renamed from ${file.oldPath ?? "?"}`);
  if (file.binary) notices.push("Binary file — contents not shown");
  return [...notices, ...file.notices];
}

export type DiffRow =
  | { kind: "file"; file: number }
  | { kind: "notice"; file: number; text: string }
  | { kind: "hunk"; file: number; text: string }
  | { kind: "line"; file: number; line: DiffLine };

/** Flatten files into virtual rows, skipping the bodies of collapsed files. */
export function diffRows(files: FileDiff[], collapsed: ReadonlySet<string>): DiffRow[] {
  const rows: DiffRow[] = [];
  files.forEach((file, index) => {
    rows.push({ kind: "file", file: index });
    if (collapsed.has(file.path)) return;
    for (const text of fileNotices(file)) rows.push({ kind: "notice", file: index, text });
    for (const hunk of file.hunks) {
      rows.push({ kind: "hunk", file: index, text: hunk.header });
      for (const line of hunk.lines) rows.push({ kind: "line", file: index, line });
    }
  });
  return rows;
}
