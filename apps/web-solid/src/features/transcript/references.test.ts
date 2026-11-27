import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { Zapper, ZapperIssue } from "@/core/zapper-types";
import { findReferences, openAsks, parsePrUrl, REFERENCE_LIMIT, referenceCatalog, type ReferenceSources } from "./references";

const sources: ReferenceSources = {
  issues: [{ id: "i12", number: 12, title: "Inline references", status: "needsYou" }, { id: "i3", number: 3, title: "Old work", status: "done" }],
  pullRequests: [
    { url: "https://github.com/opcodehq/zuse/pull/378", title: "Pricing" },
    { url: "https://github.com/opcodehq/zuse/pull/378/", title: "", state: "merged" },
    { url: "https://github.com/opcodehq/zuse/pull/12", title: "Draft", state: "open", draft: true },
    { url: "javascript:alert(1)", title: "Unsafe", state: "open" },
  ],
  threads: [{ id: "chat-7f3a", title: "Fix billing" }, { id: "chat-untitled" }],
  agents: [{ ref: "chat-7f3a--sub--verify", title: "Verify", status: "running" }],
};
const catalog = referenceCatalog(sources);
const labels = (text: string, from = catalog) => findReferences(text, from).map(match => text.slice(match.start, match.end));

describe("referenceCatalog", () => {
  test("indexes issues, every PR form, threads and subagents", () => {
    assert.deepEqual(labels("ZAP-12 needs you: merge zuse#378 (opcodehq/zuse#378, #378). See chat-7f3a and chat-7f3a--sub--verify."), ["ZAP-12", "zuse#378", "opcodehq/zuse#378", "#378", "chat-7f3a", "chat-7f3a--sub--verify"]);
  });
  test("carries live status from the data that has it", () => {
    assert.equal(catalog.index.get("ZAP-12")?.status, "needsYou");
    assert.equal(catalog.index.get("zuse#378")?.status, "merged");
    assert.equal(catalog.index.get("zuse#378")?.title, "Pricing");
    assert.equal(catalog.index.get("#12")?.status, "draft");
    assert.equal(catalog.index.get("chat-7f3a--sub--verify")?.status, "running");
    assert.equal(catalog.index.get("chat-7f3a")?.text, "Fix billing");
    assert.equal(catalog.index.get("chat-untitled")?.text, undefined);
  });
  test("unknown codes and unsafe URLs stay plain", () => {
    assert.deepEqual(labels("ZAP-999, zuse#1, other/zuse#378, Subagent 1, chat-unknown"), []);
    assert.equal([...catalog.index.values()].some(ref => ref.target.startsWith("javascript:")), false);
  });
  test("matches only whole tokens", () => {
    assert.deepEqual(labels("SWA-ZAP-12 ZAP-123 ZAP-12x /ZAP-12 ZAP-12/a αZAP-12 #3780 a#378 v.ZAP-12 ZAP-12.5"), []);
    assert.deepEqual(labels("(ZAP-12), **ZAP-3**. Done: ZAP-12!"), ["ZAP-12", "ZAP-3", "ZAP-12"]);
  });
  test("an alias shared by two targets is dropped while qualified forms resolve", () => {
    const mixed = referenceCatalog({ ...sources, pullRequests: [...sources.pullRequests, { url: "https://github.com/other/zuse/pull/378", title: "Fork" }] });
    assert.equal(mixed.index.has("#378"), false);
    assert.equal(mixed.index.has("zuse#378"), false);
    assert.deepEqual(labels("opcodehq/zuse#378 other/zuse#378 #378", mixed), ["opcodehq/zuse#378", "other/zuse#378"]);
  });
  test("a thread id that collides with an issue code is ambiguous", () => {
    const clash = referenceCatalog({ ...sources, threads: [{ id: "ZAP-12", title: "Odd" }] });
    assert.deepEqual(labels("ZAP-12 ZAP-3", clash), ["ZAP-3"]);
  });
  test("is bounded", () => {
    const big = referenceCatalog({ issues: [], pullRequests: [], agents: [], threads: Array.from({ length: REFERENCE_LIMIT + 50 }, (_, i) => ({ id: `thread-${i}` })) });
    assert.equal(big.index.size, REFERENCE_LIMIT);
    assert.equal(findReferences("ZAP-3 ".repeat(20), catalog, 5).length, 5);
    assert.equal(referenceCatalog({ issues: [], pullRequests: [], threads: [], agents: [] }).pattern, null);
  });
  test("parses GitHub PR URLs only", () => {
    assert.deepEqual(parsePrUrl("https://github.com/opcodehq/zuse/pull/378"), { owner: "opcodehq", repo: "zuse", number: 378 });
    assert.equal(parsePrUrl("https://github.com/opcodehq/zuse/issues/378"), null);
    assert.equal(parsePrUrl("https://evil.com/opcodehq/zuse/pull/378"), null);
  });
});

const issue = (id: string, extra: Partial<ZapperIssue["details"]> = {}, threads: string[] = [], activity: ZapperIssue["activity"] = []): ZapperIssue => ({
  id, number: 1, createdAt: 0, updatedAt: 5, threads, pullRequests: [], activity,
  details: { title: `Title ${id}`, description: "", status: "needsYou", priority: "normal", assigned: false, dueDate: null, labels: [], relations: [], ...extra },
});
const zapper: Zapper = {
  id: "z", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: "coord", previousCoordinatorChats: ["old-coord"], workers: [],
  config: { name: "Z", purpose: "", homeDeviceId: "d", workerDeviceId: "d", repositories: [], coordinatorModel: { harness: "codex", model: null }, maxConcurrentWorkers: 1, maxFollowups: 1 },
  decisions: [{ id: "d1", workerChatId: "worker", question: "Use Postgres?", answer: null }, { id: "d2", workerChatId: "worker", question: "Answered", answer: "yes" }],
  issues: [
    issue("asked", {}, ["worker"], [{ id: "q1", createdAt: 1, kind: "question", text: "Old question" }, { id: "c", createdAt: 2, kind: "comment", text: "x" }, { id: "q2", createdAt: 3, kind: "question", text: " Merge zuse#378? " }]),
    issue("bare"),
    issue("agent-owned", { owner: { type: "zapper" } }),
    issue("working", { status: "inProgress" }, ["worker"]),
  ],
};

describe("openAsks", () => {
  test("the coordinator sees every open user ask and pending decision", () => {
    assert.deepEqual(openAsks(zapper, "coord"), [
      { key: "question:q2", text: "Merge zuse#378?", issueId: "asked" },
      { key: "issue:bare:5", text: "Title bare", issueId: "bare" },
      { key: "decision:d1", text: "Use Postgres?" },
    ]);
    assert.equal(openAsks(zapper, "old-coord").length, 3);
  });
  test("a worker sees only asks linked to it", () => {
    assert.deepEqual(openAsks(zapper, "worker").map(ask => ask.key), ["question:q2", "decision:d1"]);
    assert.deepEqual(openAsks(zapper, "elsewhere"), []);
    assert.deepEqual(openAsks(undefined, "coord"), []);
  });
});
