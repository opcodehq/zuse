import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { Chat, SessionMessageEntry } from "@/core/protocol";
import type { Zapper } from "@/core/zapper-types";
import { conversationReferences, referenceIndex, referencePattern, REFERENCE_LIMIT } from "./references";

const zapper: Zapper = {
  id: "owner", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: "parent", previousCoordinatorChats: [], workers: [], decisions: [],
  config: { name: "Owner", purpose: "", homeDeviceId: "device", workerDeviceId: "device", repositories: [], coordinatorModel: { harness: "codex", model: null }, maxConcurrentWorkers: 1, maxFollowups: 1 },
  issues: [{ id: "issue", number: 12, createdAt: 0, updatedAt: 0, details: { title: "Actual issue", description: "", status: "inProgress", priority: "normal", assigned: false, dueDate: null, labels: [], relations: [] }, threads: ["linked-thread"], pullRequests: [{ title: "Linked PR", url: "https://github.com/owner/repo/pull/34" }], activity: [] }],
};
const chats: Record<string, Chat> = Object.fromEntries(["parent", "linked-thread", "unrelated"].map(id => [id, { id, deviceId: "device", archived: false, createdAt: "" }]));
const matches = (text: string) => [...text.matchAll(referencePattern(referenceIndex(conversationReferences(zapper, chats, "parent", [])))!)].map(match => match[0]);

describe("conversation references", () => {
  test("resolves only known local issues, linked threads and PRs", () => {
    assert.deepEqual(matches("ZAP-12, ZAP-999; linked-thread, unrelated, #34 and owner/repo#34"), ["ZAP-12", "linked-thread", "#34", "owner/repo#34"]);
    assert.deepEqual(matches("other-ZAP-12 ZAP-123 /ZAP-12 ZAP-12/suffix αZAP-12"), []);
    assert.deepEqual(conversationReferences(undefined, chats, "parent", []).map(ref => ref.label), ["parent"]);
  });
  test("retains actual issue state without deriving it from PRs or activity", () => {
    for (const status of ["inProgress", "needsYou", "inReview", "done", "canceled", "blocked"] as const) {
      const source = structuredClone(zapper);
      source.issues[0]!.details.status = status;
      assert.equal(conversationReferences(source, chats, "parent", [])[0]!.status, status);
      assert.equal(conversationReferences(source, chats, "parent", []).filter(ref => ref.kind === "pr").every(ref => ref.status === undefined), true);
    }
  });
  test("resolves a normal chat's cached PR and explicit parent without a Zapper", () => {
    const refs = conversationReferences(undefined, { ...chats, child: { ...chats.parent!, id: "child", parentChatId: "parent" } }, "child", [], { title: "Current PR", url: "https://github.com/owner/repo/pull/56" });
    assert.deepEqual(refs.map(ref => ref.label), ["owner/repo#56", "#56", "child", "parent"]);
  });
  test("uses durable subagent refs, never invented display numbering", () => {
    const messages: SessionMessageEntry[] = [{ deviceId: "device", id: "entry", role: "assistant", createdAt: 0, parts: [{ id: "tool", kind: "tool", subagentRef: "parent--sub--actual", call: { kind: "unknown", name: "Agent: Inspect" } }] }];
    const refs = conversationReferences(undefined, {}, "parent", messages);
    assert.deepEqual(refs.map(ref => ref.label), ["parent--sub--actual"]);
    assert.equal(referencePattern(referenceIndex(refs))!.test("Subagent 1"), false);
  });
  test("ambiguous short PR numbers stay plain, qualified numbers still resolve", () => {
    const source = structuredClone(zapper);
    source.issues[0]!.pullRequests.push({ title: "Other repository", url: "https://github.com/other/repo/pull/34" });
    const index = referenceIndex(conversationReferences(source, chats, "parent", []));
    assert.equal(index.has("#34"), false);
    assert.equal(index.has("owner/repo#34"), true);
    assert.equal(index.has("other/repo#34"), true);
  });
  test("bounds catalog size and rejects unsafe PR destinations", () => {
    const ref = { label: "#1", title: "Unsafe", kind: "pr", target: "javascript:alert(1)" } as const;
    assert.equal(referenceIndex([ref]).size, 0);
    assert.equal(referenceIndex(Array.from({ length: 1000 }, (_, i) => ({ ...ref, kind: "thread", label: `thread-${i}` }))).size, REFERENCE_LIMIT);
  });
  test("truncated metadata cannot make a short PR alias falsely unambiguous", () => {
    const source = structuredClone(zapper);
    source.issues[0]!.pullRequests.push(...Array.from({ length: 300 }, (_, i) => ({ title: "Known PR", url: `https://github.com/owner/repo/pull/${100 + i}` })), { title: "Late conflict", url: "https://github.com/other/repo/pull/34" });
    const index = referenceIndex(conversationReferences(source, chats, "parent", []));
    assert.equal(index.has("#34"), false);
    assert.equal(index.has("owner/repo#34"), true);
    assert.equal([...index.keys()].some(label => /^#\d+$/.test(label)), false);
  });
});
