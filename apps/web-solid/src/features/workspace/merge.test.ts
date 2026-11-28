import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { allowedMethods, mergeButton, pickMethod } from "./merge";
import type { ChangeRequestCheck, ChangeRequestDetail } from "./types";

const check = (status: string, conclusion = ""): ChangeRequestCheck => ({ name: "ci", context: "", status, conclusion, state: "", detailsUrl: "", targetUrl: "" });
const detail = (extra: Partial<ChangeRequestDetail> = {}): ChangeRequestDetail => ({
  title: "T", body: "", url: "https://github.com/acme/w/pull/7", number: 7, author: { login: "octo" }, baseRefName: "main", headRefName: "feat", headRefOid: "a",
  state: "OPEN", isDraft: false, reviewDecision: "", mergeable: "MERGEABLE", additions: 1, deletions: 1, comments: [], reviews: [], reviewThreads: [], files: [], statusCheckRollup: [check("COMPLETED", "SUCCESS")], ...extra,
});

describe("mergeButton", () => {
  test("ready reads Merge and is primary", () => {
    assert.deepEqual(mergeButton(detail()), { label: "Merge", action: "merge", primary: true });
  });
  test("pending or failing checks offer auto-merge, quiet but enabled", () => {
    assert.deepEqual(mergeButton(detail({ statusCheckRollup: [check("IN_PROGRESS")] })), { label: "Auto-merge", action: "autoMerge", primary: false });
    assert.deepEqual(mergeButton(detail({ statusCheckRollup: [check("COMPLETED", "FAILURE")] })), { label: "Auto-merge", action: "autoMerge", primary: false });
    assert.deepEqual(mergeButton(detail({ reviewDecision: "REVIEW_REQUIRED" })), { label: "Auto-merge", action: "autoMerge", primary: false });
  });
  test("armed auto-merge reads Auto-merge on and opens the menu to cancel", () => {
    assert.deepEqual(mergeButton(detail({ statusCheckRollup: [check("IN_PROGRESS")], autoMergeRequest: { mergeMethod: "SQUASH" } })), { label: "Auto-merge on", action: "menu", primary: false });
  });
  test("conflicts keep a quiet Merge so GitHub can explain; drafts offer Ready for review", () => {
    assert.deepEqual(mergeButton(detail({ mergeable: "CONFLICTING" })), { label: "Merge", action: "merge", primary: false });
    assert.deepEqual(mergeButton(detail({ isDraft: true })), { label: "Ready for review", action: "markReady", primary: false });
  });
  test("merged and closed pull requests have no merge button", () => {
    assert.equal(mergeButton(detail({ state: "MERGED" })), null);
    assert.equal(mergeButton(detail({ state: "CLOSED" })), null);
  });
});

describe("merge methods", () => {
  test("show only what the repository allows, all three when unknown", () => {
    assert.deepEqual(allowedMethods({ mergeMethods: ["squash", "rebase"] }).map((m) => m.value), ["squash", "rebase"]);
    assert.deepEqual(allowedMethods({}).map((m) => m.value), ["squash", "merge", "rebase"]);
    assert.deepEqual(allowedMethods({ mergeMethods: [] }).map((m) => m.value), ["squash", "merge", "rebase"]);
  });
  test("remember the last pick while it is still allowed", () => {
    const allowed = allowedMethods({ mergeMethods: ["merge", "rebase"] });
    assert.equal(pickMethod(allowed, "rebase"), "rebase");
    assert.equal(pickMethod(allowed, "squash"), "merge");
    assert.equal(pickMethod(allowed, null), "merge");
  });
});
