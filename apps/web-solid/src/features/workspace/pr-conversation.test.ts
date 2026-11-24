import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { appendPrQuestion, prQuestion, prStatus } from "./pr-conversation";
import type { ChangeRequestCheck, ChangeRequestDetail } from "./types";

const check = (changes: Partial<ChangeRequestCheck>): ChangeRequestCheck => ({ name: "", context: "", status: "", conclusion: "", state: "", detailsUrl: "", targetUrl: "", ...changes });

const detail = (changes: Partial<ChangeRequestDetail> = {}): ChangeRequestDetail => ({
  title: "Widget rendering", body: "", url: "https://github.com/acme/widgets/pull/7", number: 7,
  author: { login: "octo" }, baseRefName: "main", headRefName: "widget", headRefOid: "a".repeat(40),
  state: "OPEN", isDraft: false, reviewDecision: "APPROVED", mergeable: "MERGEABLE", additions: 12, deletions: 3,
  comments: [], reviews: [], files: [], statusCheckRollup: [], ...changes,
});

describe("PR conversation status", () => {
  test("does not promise merge readiness when no checks are reported", () => {
    assert.match(prStatus(detail()), /No checks have been reported/);
    assert.doesNotMatch(prStatus(detail()), /Ready to merge/);
  });
  test("reports concurrent blockers rather than hiding them behind one status", () => {
    const status = prStatus(detail({ isDraft: true, mergeable: "CONFLICTING", reviewDecision: "CHANGES_REQUESTED", statusCheckRollup: [
      check({ name: "lint", conclusion: "FAILURE" }), check({ name: "build", status: "IN_PROGRESS" }),
    ], reviewThreads: [{ id: "thread", isResolved: false, isOutdated: true, path: "a", comments: [] }] }));
    for (const text of ["still a draft", "conflicts", "1 check is failing", "1 check is still pending", "Changes were requested", "1 review thread remains unresolved"]) assert.ok(status.includes(text), text);
  });
  test("unknown check results remain pending", () => {
    assert.match(prStatus(detail({ statusCheckRollup: [check({ name: "unknown", conclusion: "MYSTERY" })] })), /still pending/);
  });
  test("successful checks do not fabricate approval", () => {
    const status = prStatus(detail({ reviewDecision: "", statusCheckRollup: [check({ name: "test", conclusion: "SUCCESS" }), check({ name: "skip", conclusion: "SKIPPED" })] }));
    assert.match(status, /finished without failures/);
    assert.match(status, /approval is not confirmed/);
  });
  test("closed and merged lifecycle overrides stale blockers", () => {
    assert.equal(prStatus(detail({ state: "CLOSED", isDraft: true })), "This pull request is closed without merging.");
    assert.equal(prStatus(detail({ state: "MERGED" })), "This pull request has been merged.");
  });
  test("reports review requirements and auto-merge without treating them as success", () => {
    assert.match(prStatus(detail({ reviewDecision: "REVIEW_REQUIRED", autoMergeRequest: { mergeMethod: "SQUASH" } })), /A review is required. Auto-merge is enabled \(squash\)/);
  });
});

describe("PR chat handoff", () => {
  test("includes exact PR context and requests analysis, not mutations", () => {
    for (const intent of ["summary", "blockers"] as const) {
      const prompt = prQuestion(detail().url, intent);
      assert.ok(prompt.includes(detail().url));
      assert.ok(prompt.includes("Read the current diff, checks and reviews"));
      assert.ok(prompt.includes("analysis only; do not merge, enable auto-merge, close"));
    }
  });
  test("preserves existing draft text exactly", () => {
    assert.equal(appendPrQuestion("Already drafting\n ", "Question"), "Already drafting\n \n\nQuestion");
    assert.equal(appendPrQuestion("", "Question"), "Question");
  });
});
