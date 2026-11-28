import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { aiPrompt, checksSummary, cleanBody, feedbackSummary, fixAllPrompt, fixPrompt, groupFeedback, sortChecks, statusLine } from "./feedback";
import type { ChangeRequestCheck, ChangeRequestComment, ChangeRequestDetail } from "./types";

const check = (name: string, status: string, conclusion = ""): ChangeRequestCheck => ({ name, context: "", status, conclusion, state: "", detailsUrl: "", targetUrl: "" });
const comment = (id: string, login: string, body: string, minutes: number, extra: Partial<ChangeRequestComment> = {}): ChangeRequestComment => ({
  id, url: `https://github.com/acme/w/pull/7#${id}`, author: { login }, body, state: "", createdAt: new Date(Date.UTC(2026, 10, 28, 12, minutes)).toISOString(), submittedAt: "", diffHunk: "", ...extra,
});
const detail = (extra: Partial<ChangeRequestDetail> = {}): ChangeRequestDetail => ({
  title: "Quieter errors", body: "", url: "https://github.com/acme/w/pull/7", number: 7, author: { login: "octo" }, baseRefName: "main", headRefName: "feat", headRefOid: "a",
  state: "OPEN", isDraft: false, reviewDecision: "", mergeable: "MERGEABLE", additions: 1, deletions: 1, comments: [], reviews: [], reviewThreads: [], files: [], statusCheckRollup: [], ...extra,
});

const RABBIT = [
  "_⚠️ Potential issue_ | _🟡 Minor_",
  "",
  "**Open the Git panel after switching to the branch's chat.**",
  "",
  "If the branch belongs to another chat, `openChat(target)` changes only the chat.",
  "",
  "<details>",
  "<summary>🤖 Prompt for AI Agents</summary>",
  "",
  "```",
  "In @apps/web-solid/src/ReferenceMarkdown.tsx around lines 267 - 270, open the target chat, then call openPanel(\"git\").",
  "```",
  "",
  "</details>",
  "",
  "<!-- fingerprinting:phantom:triton -->",
].join("\n");

describe("checks", () => {
  const list = [check("platforms", "COMPLETED", "SUCCESS"), check("rust", "IN_PROGRESS"), check("lint", "COMPLETED", "FAILURE"), check("deploy", "COMPLETED", "SKIPPED"), check("native", "QUEUED"), check("browser", "COMPLETED", "SUCCESS")];
  test("sort running, then failed, then passed and skipped, keeping order within a group", () => {
    assert.deepEqual(sortChecks(list).map((c) => c.name), ["rust", "native", "lint", "platforms", "browser", "deploy"]);
  });
  test("summarise the non-zero counts", () => {
    assert.equal(checksSummary(list), "2 running, 1 failed, 2 passed, 1 skipped");
    assert.equal(checksSummary([check("a", "IN_PROGRESS"), ...Array.from({ length: 7 }, (_, i) => check(`p${i}`, "COMPLETED", "SUCCESS")), check("b", "PENDING")]), "2 running, 7 passed");
    assert.equal(checksSummary([]), "");
  });
});

describe("statusLine", () => {
  test("reads as one line with the next step", () => {
    assert.deepEqual(statusLine(detail({ statusCheckRollup: [check("rust", "IN_PROGRESS")] })), { state: "Open", tone: "success", detail: "Waiting on checks", ready: false });
    assert.deepEqual(statusLine(detail({ statusCheckRollup: [check("rust", "COMPLETED", "SUCCESS")] })), { state: "Open", tone: "success", detail: "Ready to merge", ready: true });
    assert.equal(statusLine(detail({ statusCheckRollup: [check("rust", "COMPLETED", "FAILURE"), check("x", "QUEUED")] })).detail, "Checks failing");
    assert.equal(statusLine(detail({ mergeable: "CONFLICTING" })).detail, "Has conflicts");
    assert.equal(statusLine(detail({ reviewDecision: "CHANGES_REQUESTED" })).detail, "Changes requested");
    assert.equal(statusLine(detail({ reviewDecision: "REVIEW_REQUIRED" })).ready, false);
    assert.deepEqual(statusLine(detail({ state: "MERGED" })), { state: "Merged", tone: "merged", detail: null, ready: false });
    assert.deepEqual(statusLine(detail({ isDraft: true })), { state: "Draft", tone: "muted", detail: null, ready: false });
  });
});

describe("feedback parsing", () => {
  test("finds severity and the bold title", () => {
    assert.deepEqual(feedbackSummary(RABBIT), { severity: "Minor", title: "Open the Git panel after switching to the branch's chat." });
    assert.deepEqual(feedbackSummary("Minor · Functional Correctness · Quick win\n\n**Count distinct file paths.**\n\nBody"), { severity: "Minor", title: "Count distinct file paths." });
    assert.deepEqual(feedbackSummary("Nit: rename this."), { severity: "Nit", title: "Nit: rename this." });
    assert.deepEqual(feedbackSummary("## Deploying with Cloudflare Workers\n\nLatest commit: abc"), { severity: null, title: "Deploying with Cloudflare Workers" });
  });
  test("extracts the AI prompt block and keeps it out of the body", () => {
    assert.equal(aiPrompt(RABBIT), "In @apps/web-solid/src/ReferenceMarkdown.tsx around lines 267 - 270, open the target chat, then call openPanel(\"git\").");
    assert.equal(aiPrompt("Plain comment"), null);
    const body = cleanBody(RABBIT);
    assert.ok(!body.includes("Prompt for AI"));
    assert.ok(!body.includes("fingerprinting"));
    assert.ok(body.includes("openChat(target)"));
    assert.equal(cleanBody("A\n<details>\n<summary>📝 Committable suggestion</summary>\n\nfix\n</details>"), "A\n\n**📝 Committable suggestion**\n\nfix");
  });
});

describe("groupFeedback", () => {
  const d = detail({
    comments: [comment("c1", "octo", "My own note", 1), comment("c2", "cloudflare-workers-and-pages[bot]", "## Deploying\n\nDone", 2)],
    reviews: [comment("r1", "coderabbitai[bot]", "", 3, { state: "COMMENTED" }), comment("r2", "maya", "Looks close, see inline.", 4, { state: "CHANGES_REQUESTED" })],
    reviewThreads: [
      { id: "t1", isResolved: false, isOutdated: false, path: "apps/web-solid/src/ReferenceMarkdown.tsx", line: 268, comments: [comment("x1", "coderabbitai[bot]", RABBIT, 5), comment("x2", "octo", "on it", 6)] },
      { id: "t2", isResolved: true, isOutdated: false, path: "src/b.ts", line: 3, comments: [comment("x3", "maya", "Rename this.", 0)] },
      { id: "t3", isResolved: false, isOutdated: true, path: "src/c.ts", line: null, comments: [comment("x4", "maya", "Old.", 7)] },
      { id: "t4", isResolved: false, isOutdated: false, path: "src/d.ts", line: 9, comments: [] },
    ],
  });
  const { open, closed } = groupFeedback(d);
  test("keeps actionable items open, oldest first, and leaves out the author's own comments and empty reviews", () => {
    assert.deepEqual(open.map((i) => [i.id, i.kind]), [["c2", "comment"], ["r2", "review"], ["t1", "thread"]]);
    assert.equal(open[2]!.replies, 1);
    assert.equal(open[2]!.path, "apps/web-solid/src/ReferenceMarkdown.tsx");
  });
  test("puts resolved and outdated threads apart", () => {
    assert.deepEqual(closed.map((i) => [i.id, i.resolved, i.outdated]), [["t2", true, false], ["t3", false, true]]);
  });
  test("builds a fix prompt from the bot's prompt, else from file, line and comment", () => {
    assert.equal(fixPrompt(open[2]!), aiPrompt(RABBIT));
    assert.equal(fixPrompt(closed[0]!), "Address this review comment from maya on src/b.ts:3:\n\nRename this.");
    assert.equal(fixPrompt(open[1]!), "Address this pull request comment from maya:\n\nLooks close, see inline.");
    assert.equal(fixPrompt({ ...open[2]!, path: "src/other.ts", line: 4 }), `In src/other.ts:4: ${aiPrompt(RABBIT)}`);
  });
  test("numbers every item in one prompt", () => {
    const all = fixAllPrompt([closed[0]!, open[1]!], d);
    assert.equal(all, "Fix these 2 review comments on #7 (Quieter errors):\n\n1. Address this review comment from maya on src/b.ts:3:\n   \n   Rename this.\n\n2. Address this pull request comment from maya:\n   \n   Looks close, see inline.");
    assert.equal(fixAllPrompt([closed[0]!], d), fixPrompt(closed[0]!));
  });
});
