import { test } from "node:test";
import assert from "node:assert/strict";
import { issueRelations } from "./issue-relations";
import type { Zapper } from "@/core/zapper-types";

const issue = (id: string, number: number, blocks: string[] = [], relations: string[] = []) => ({ id, number, details: { title: id, status: "todo", blocks, relations } });
const zapper = (issues: ReturnType<typeof issue>[]) => ({ issues } as unknown as Zapper);
const view = (z: Zapper, id: string) => issueRelations(z, id).map(r => [r.kind, r.issue.id]);

test("blocking reads as blocked by from the other side and related is symmetric", () => {
  const z = zapper([issue("a", 1, ["b"]), issue("b", 2, [], ["c"]), issue("c", 3)]);
  assert.deepEqual(view(z, "a"), [["blocks", "b"]]);
  assert.deepEqual(view(z, "b"), [["blockedBy", "a"], ["related", "c"]]);
  assert.deepEqual(view(z, "c"), [["related", "b"]]);
});

test("missing targets, self links and issues saved before blocks are ignored", () => {
  const z = zapper([issue("a", 1, ["a", "gone"], ["gone"]), { id: "old", number: 2, details: { title: "old", status: "todo", relations: ["a"] } } as never]);
  assert.deepEqual(view(z, "a"), [["related", "old"]]);
  assert.deepEqual(view(z, "missing"), []);
});
