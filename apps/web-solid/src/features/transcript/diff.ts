// Line diff for inline tool diffs: Myers over the changed middle, grouped into hunks with 3 lines of context.
export type LineKind = "add" | "del" | "context";

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
  added: boolean;
  hunks: Hunk[];
  additions: number;
  deletions: number;
  truncatedBy: number;
}

const CONTEXT = 3;
const MAX_EDIT_DISTANCE = 2_000;
export const DIFF_DETAIL_MAX_LINES = 600;

const splitLines = (text: string) => {
  if (!text) return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
};

type Op = { kind: LineKind; a: number; b: number };

/** Edit script between `a` and `b`. Falls back to replace-all past MAX_EDIT_DISTANCE. */
function myers(a: string[], b: string[]): Op[] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length, endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) { endA--; endB--; }
  const ops: Op[] = [];
  for (let i = 0; i < start; i++) ops.push({ kind: "context", a: i, b: i });
  const n = endA - start, m = endB - start;
  const middle: Op[] = [];
  const max = n + m;
  const offset = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  let found = n === 0 && m === 0;
  for (let d = 0; d <= Math.min(max, MAX_EDIT_DISTANCE) && !found; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[offset + k - 1]! < v[offset + k + 1]!) ? v[offset + k + 1]! : v[offset + k - 1]! + 1;
      let y = x - k;
      while (x < n && y < m && a[start + x] === b[start + y]) { x++; y++; }
      v[offset + k] = x;
      if (x >= n && y >= m) { found = true; break; }
    }
  }
  if (!found) {
    for (let i = 0; i < n; i++) middle.push({ kind: "del", a: start + i, b: -1 });
    for (let j = 0; j < m; j++) middle.push({ kind: "add", a: -1, b: start + j });
  } else if (n || m) {
    let x = n, y = m;
    for (let d = trace.length - 1; d >= 0 && (x > 0 || y > 0); d--) {
      const vd = trace[d]!;
      const k = x - y;
      const prevK = k === -d || (k !== d && vd[offset + k - 1]! < vd[offset + k + 1]!) ? k + 1 : k - 1;
      const prevX = vd[offset + prevK]!;
      const prevY = prevX - prevK;
      while (x > prevX && y > prevY) { x--; y--; middle.push({ kind: "context", a: start + x, b: start + y }); }
      if (d > 0) {
        if (x === prevX) middle.push({ kind: "add", a: -1, b: start + prevY });
        else middle.push({ kind: "del", a: start + prevX, b: -1 });
      }
      x = prevX; y = prevY;
    }
    middle.reverse();
  }
  ops.push(...middle);
  for (let i = 0; i < a.length - endA; i++) ops.push({ kind: "context", a: endA + i, b: endB + i });
  return ops;
}

/** `diff_to_file` + `truncate_file_lines`. */
export function diffFile(path: string, oldText: string | null | undefined, newText: string): FileDiff {
  const a = splitLines(oldText ?? "");
  const b = splitLines(newText);
  const ops = myers(a, b);
  let additions = 0, deletions = 0;
  const changed: number[] = [];
  ops.forEach((op, ix) => {
    if (op.kind === "add") additions++;
    if (op.kind === "del") deletions++;
    if (op.kind !== "context") changed.push(ix);
  });
  const hunks: Hunk[] = [];
  let ix = 0;
  while (ix < changed.length) {
    const from = Math.max(0, changed[ix]! - CONTEXT);
    let to = Math.min(ops.length - 1, changed[ix]! + CONTEXT);
    while (ix + 1 < changed.length && changed[ix + 1]! - CONTEXT <= to + 1) {
      ix++;
      to = Math.min(ops.length - 1, changed[ix]! + CONTEXT);
    }
    ix++;
    const slice = ops.slice(from, to + 1);
    const oldStart = slice.find((op) => op.a >= 0)?.a ?? 0;
    const newStart = slice.find((op) => op.b >= 0)?.b ?? 0;
    const oldLen = slice.filter((op) => op.kind !== "add").length;
    const newLen = slice.filter((op) => op.kind !== "del").length;
    hunks.push({
      header: `@@ -${oldStart + 1},${oldLen} +${newStart + 1},${newLen} @@`,
      lines: slice.map((op) => ({
        kind: op.kind,
        oldNo: op.kind === "add" ? null : op.a + 1,
        newNo: op.kind === "del" ? null : op.b + 1,
        text: op.kind === "add" ? b[op.b]! : a[op.a]!,
      })),
    });
  }
  let budget = DIFF_DETAIL_MAX_LINES;
  let truncatedBy = 0;
  for (const hunk of hunks) {
    if (hunk.lines.length > budget) {
      truncatedBy += hunk.lines.length - budget;
      hunk.lines = hunk.lines.slice(0, budget);
    }
    budget -= hunk.lines.length;
  }
  return { path, added: oldText === null || oldText === undefined, hunks: hunks.filter((h) => h.lines.length), additions, deletions, truncatedBy };
}
