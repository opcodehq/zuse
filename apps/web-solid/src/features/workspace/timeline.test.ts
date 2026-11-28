import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { statusLine } from "./feedback";
import { buildTimeline, eventParts, groupSummary, mergeReason } from "./timeline";
import type { ChangeRequestDetail, TimelineItem } from "./types";

const at = (m: number) => new Date(Date.UTC(2026, 10, 28, 12, m)).toISOString();
const actor = (login: string) => ({ login });
const detail = (timeline: TimelineItem[] | undefined, extra: Partial<ChangeRequestDetail> = {}): ChangeRequestDetail => ({
  title: "T", body: "", url: "https://github.com/acme/w/pull/7", number: 7, author: { login: "octo" }, baseRefName: "main", headRefName: "feat", headRefOid: "a",
  state: "OPEN", isDraft: false, reviewDecision: "", mergeable: "MERGEABLE", additions: 1, deletions: 1, comments: [], reviews: [], reviewThreads: [], files: [],
  statusCheckRollup: [], createdAt: at(0), timeline, ...extra,
});
const commit = (m: number, oid: string, state?: string): TimelineItem => ({ kind: "commit", actor: actor("octo"), createdAt: at(m), state, commit: { oid, message: `commit ${oid}` } });

describe("buildTimeline", () => {
  test("falls back when the engine sent no timeline", () => {
    assert.equal(buildTimeline(detail(undefined)), null);
  });
  test("starts with the opening, folds consecutive commits and keeps the last push's checks", () => {
    const rows = buildTimeline(detail([commit(1, "aaaaaaa1", "FAILURE"), commit(2, "bbbbbbb2", "SUCCESS"), { kind: "comment", actor: actor("maya"), createdAt: at(3), url: "u1" }, commit(4, "ccccccc3", "PENDING")], { comments: [{ id: "1", url: "u1", author: actor("maya"), body: "Looks good", state: "", createdAt: at(3), submittedAt: "", diffHunk: "" }] }))!;
    assert.deepEqual(rows.map((r) => r.type === "commits" ? ["commits", r.commits.length, r.checks] : r.type === "event" ? [r.item.kind, r.body] : ["group"]), [
      ["opened", ""], ["commits", 2, "passed"], ["comment", "Looks good"], ["commits", 1, "running"],
    ]);
  });
  test("folds runs of three or more minor events under a summary, leaves shorter runs inline", () => {
    const rows = buildTimeline(detail([
      { kind: "labeled", actor: actor("octo"), createdAt: at(1), name: "ui", color: "00ff00" },
      { kind: "labeled", actor: actor("octo"), createdAt: at(2), name: "web" },
      { kind: "reviewRequested", actor: actor("octo"), createdAt: at(3), name: "maya" },
      { kind: "review", actor: actor("maya"), createdAt: at(4), state: "APPROVED" },
      { kind: "autoMergeEnabled", actor: actor("octo"), createdAt: at(5) },
      { kind: "merged", actor: actor("octo"), createdAt: at(6), to: "0123456789" },
    ]))!;
    assert.deepEqual(rows.map((r) => (r.type === "group" ? `group: ${r.summary}` : r.type === "event" ? r.item.kind : r.type)), ["opened", "group: 3 events: 2 label changes, 1 review request", "review", "autoMergeEnabled", "merged"]);
  });
  test("finds a review's body by author and time when its URL differs", () => {
    const rows = buildTimeline(detail([{ kind: "review", actor: actor("maya"), createdAt: at(4), state: "APPROVED", url: "https://github.com/acme/w/pull/7#pullrequestreview-9" }], { reviews: [{ id: "r", url: "", author: actor("maya"), body: "Ship it", state: "APPROVED", createdAt: at(4), submittedAt: at(4), diffHunk: "" }] }))!;
    assert.equal(rows[1]!.type === "event" && rows[1]!.body, "Ship it");
  });
  test("sorts by time even when the engine's order differs", () => {
    const rows = buildTimeline(detail([{ kind: "closed", actor: actor("octo"), createdAt: at(9) }, { kind: "comment", actor: actor("octo"), createdAt: at(5) }]))!;
    assert.deepEqual(rows.map((r) => (r.type === "event" ? r.item.kind : r.type)), ["opened", "comment", "closed"]);
  });
});

describe("eventParts", () => {
  test("reads each event as a short sentence with chips", () => {
    const parts = (item: Partial<TimelineItem>) => eventParts({ actor: null, createdAt: at(0), kind: "", ...item }, "main").map((p) => (typeof p === "string" ? p : `[${p.kind}:${p.chip}]`)).join(" ");
    assert.equal(parts({ kind: "forcePush", from: "aaaaaaaaaa", to: "bbbbbbbbbb" }), "force-pushed [sha:aaaaaaa] to [sha:bbbbbbb]");
    assert.equal(parts({ kind: "review", state: "CHANGES_REQUESTED" }), "requested changes");
    assert.equal(parts({ kind: "review", state: "APPROVED" }), "approved these changes");
    assert.equal(parts({ kind: "unlabeled", name: "bug" }), "removed the [label:bug] label");
    assert.equal(parts({ kind: "baseRefChanged", from: "dev", to: "main" }), "changed the base branch from [ref:dev] to [ref:main]");
    assert.equal(parts({ kind: "merged", to: "0123456789" }), "merged commit [sha:0123456] into [ref:main]");
    assert.equal(parts({ kind: "connected", subject: { number: 12, title: "Bug", url: "u", state: "OPEN", isPullRequest: false } }), "linked [subject:#12]");
    assert.equal(parts({ kind: "convertedToDraft" }), "converted this to a draft");
  });
  test("summarises mixed groups", () => {
    assert.equal(groupSummary([{ item: { kind: "connected", createdAt: "" } }, { item: { kind: "crossReferenced", createdAt: "" } }, { item: { kind: "crossReferenced", createdAt: "" } }]), "3 events: 1 linked issue, 2 mentions");
  });
});

describe("mergeability reason", () => {
  test("adds GitHub's reason when nothing else explains the state", () => {
    assert.equal(mergeReason({ mergeStateStatus: "BEHIND", baseRefName: "main" }), "Behind main");
    assert.equal(mergeReason({ mergeStateStatus: "CLEAN", baseRefName: "main" }), null);
    assert.equal(statusLine(detail([], { mergeStateStatus: "BLOCKED" })).detail, "Blocked by branch rules");
    assert.equal(statusLine(detail([], { mergeStateStatus: "BEHIND" })).ready, false);
    assert.equal(statusLine(detail([], { mergeStateStatus: "CLEAN" })).detail, "Ready to merge");
  });
});
