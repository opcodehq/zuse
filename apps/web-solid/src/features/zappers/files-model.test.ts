import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { ancestors, buildTree, filesUnder, isReserved, pathProblem, RESERVED, reservedRole, rowKey, visibleRows } from "./files-model";

const RESERVED_PATHS = Object.keys(RESERVED);
import type { ZapperFile } from "@/core/zapper-types";

const file = (path: string, version = 1, deleted = false): ZapperFile => ({ path, version, content: path, ...(deleted ? { deleted } : {}) });

describe("Zapper file tree", () => {
  test("folders come first, nested paths become folders, created empty folders stay", () => {
    const tree = buildTree(
      [file("handoffs/conversational-pr-research.md"), file("handoffs/design-references/a.md"), file("NOTES.md"), file("AGENTS.md"), file("WORKERS.md"), file("INSTRUCTIONS.md"), file("DIGEST.md")],
      ["renders", "reports", "handoffs/empty"],
    );
    const rows = visibleRows(tree, new Set(["handoffs"])).map((row) => "  ".repeat(row.depth) + row.node.name);
    assert.deepEqual(rows, [
      "handoffs",
      "  design-references",
      "  empty",
      "  conversational-pr-research.md",
      "renders",
      "reports",
      "AGENTS.md",
      "DIGEST.md",
      "INSTRUCTIONS.md",
      "NOTES.md",
      "WORKERS.md",
    ]);
  });

  test("deleted files leave the tree, reserved files missing on an older Zapper still get a row", () => {
    const tree = buildTree([file("gone.md", 1), file("gone.md", 2, true), file("NOTES.md")]);
    assert.ok(!tree.some((node) => node.name === "gone.md"));
    assert.deepEqual(
      tree.find((node) => node.path === "AGENTS.md"),
      { kind: "file", name: "AGENTS.md", path: "AGENTS.md", file: null },
    );
  });

  test("a file an older engine wrote over a folder shows beside it with its own key", () => {
    const tree = buildTree([file("notes/day.md"), file("notes"), file("handoffs")], ["handoffs"]);
    const rows = visibleRows(tree, new Set(["notes"]))
      .filter((row) => !RESERVED_PATHS.includes(row.node.path))
      .map((row) => [rowKey(row.node), row.node.kind === "file" && !!row.node.conflict]);
    assert.deepEqual(rows, [
      ["d:handoffs", false],
      ["d:notes", false],
      ["f:notes/day.md", false],
      ["f:handoffs", true],
      ["f:notes", true],
    ]);
  });

  test("only exact reserved names get a role", () => {
    assert.equal(reservedRole("AGENTS.md"), "Coordinator rules");
    for (const lookalike of ["AGENTS.md ", "agents.md", "\u0410GENTS.md", "docs/AGENTS.md", "toString", "__proto__"]) {
      assert.equal(reservedRole(lookalike), null, lookalike);
      assert.equal(isReserved(lookalike), false, lookalike);
    }
  });

  test("a folder action names every live file under it, however deep, and nothing beside it", () => {
    const files = [file("a/x.md", 3), file("a/x.md", 4), file("a/deep/y.md"), file("ab.md"), file("a/old.md", 1), file("a/old.md", 2, true)];
    assert.deepEqual(filesUnder(files, "a"), [
      { path: "a/deep/y.md", version: 1 },
      { path: "a/x.md", version: 4 },
    ]);
  });

  test("hostile paths are caught before they are sent", () => {
    for (const bad of ["", "/abs", "a/", "a//b", "../x", "a/./b", "a\\b", " a", "a /b", "bell\u0007", "guide/x.md", "x".repeat(257)])
      assert.notEqual(pathProblem(bad), null, bad);
    assert.equal(pathProblem("handoffs/design references/plan.md"), null);
  });

  test("opening a nested file opens each folder above it", () => {
    assert.deepEqual(ancestors("a/b/c.md"), ["a", "a/b"]);
    assert.deepEqual(ancestors("c.md"), []);
  });
});
