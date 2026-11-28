// Test-only page: mounts the real pull request surface against a fixture pull request (no engine, no GitHub),
// with the linked chat's composer draft shown beside it. Never bundled into the app.
// Query: theme=dark|light, state=waiting|ready|merged.
import { render } from "@solidjs/web";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/shell/shell.css";
import "@/features/workspace/workspace.css";
import { rpc } from "@/core/connection";
import { setState } from "@/core/store";
import { setView } from "@/core/view";
import { draftFor } from "@/features/composer/drafts";
import { PullRequest } from "@/features/workspace/PullRequest";
import { setWorkspace } from "@/features/workspace/store";
import type { ChangeRequestCheck, ChangeRequestComment, ChangeRequestDetail } from "@/features/workspace/types";
import { flush } from "solid-js";

const query = new URLSearchParams(location.search);
const theme = query.get("theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;
const variant = query.get("state") ?? "waiting";
const CHAT = "c-quiet-errors";
const DEVICE = "fixture-device";
const URL_ = "https://github.com/opcodehq/zuse/pull/425";
const now = Date.now();
const iso = (minutes: number) => new Date(now - minutes * 60000).toISOString();
const check = (name: string, status: string, conclusion = "", link = true): ChangeRequestCheck => ({ name, context: "", status, conclusion, state: "", detailsUrl: link ? `https://example.com/checks/${encodeURIComponent(name)}` : "", targetUrl: "" });
const comment = (id: string, login: string, body: string, minutes: number, extra: Partial<ChangeRequestComment> = {}): ChangeRequestComment => ({ id, url: `${URL_}#discussion_r${id}`, author: { login }, body, state: "", createdAt: iso(minutes), submittedAt: "", diffHunk: "", ...extra });
const rabbit = (severity: string, title: string, text: string, prompt: string) => [
  `_⚠️ Potential issue_ | _${severity}_`, "", `**${title}**`, "", text, "",
  "<details>", "<summary>🤖 Prompt for AI Agents</summary>", "", "```", prompt, "```", "", "</details>", "", "<!-- fingerprinting:phantom:medusa:ocelot -->",
].join("\n");
const running = variant === "waiting";
let detail: ChangeRequestDetail = {
  title: "Quieter chat errors, and resume the chat once sign-in lands",
  body: [
    "A failed turn now shows as a quiet line instead of a red card. A chat stopped by an expired sign-in now carries on by itself once you sign back in.",
    "", "## What changed", "",
    "- **Error line:** the red \"Error\" card in the transcript is now one muted line with a Retry action.",
    "- **Resume after sign-in:** the chat records why it stopped and resumes once the account is valid again.",
    "- **Banners:** connection banners no longer stack; the newest replaces the old one.",
    "", "## Verification", "", "- Unit tests for the stop reason and resume.", "- Screenshots in light and dark.",
  ].join("\n"),
  url: URL_, number: 425, author: { login: "swarajbachu" }, baseRefName: "main", baseRefOid: "b".repeat(40), headRefName: "zuse/quiet-errors-resume-after-sign-in", headRefOid: "a".repeat(40),
  state: variant === "merged" ? "MERGED" : "OPEN", isDraft: false, reviewDecision: "", mergeable: "MERGEABLE", additions: 176, deletions: 131,
  comments: [
    comment("c1", "swarajbachu", "Pushed the retry fix.", 30),
    comment("c2", "cloudflare-workers-and-pages[bot]", "## Deploying with Cloudflare Workers\n\nThe latest updates on your project.\n\n| Status | Name | Preview |\n| --- | --- | --- |\n| ✅ Deployment successful! | zuse-website | [Visit preview](https://example.com) |", 10),
  ],
  reviews: [
    comment("r1", "coderabbitai[bot]", "**Actionable comments posted: 2**\n\n<details>\n<summary>📜 Review details</summary>\n\nConfiguration used: CodeRabbit UI\n</details>\n\n## Walkthrough\n\nThe transcript shows failed turns as a quiet line.", 9, { state: "COMMENTED", submittedAt: iso(9) }),
    comment("r2", "maya", "Keep the banner change in its own pull request; it touches the shell layout and needs its own review.", 7, { state: "CHANGES_REQUESTED", submittedAt: iso(7) }),
  ],
  reviewThreads: [
    { id: "t1", isResolved: false, isOutdated: false, path: "apps/web-solid/src/features/transcript/ReferenceMarkdown.tsx", line: 270, comments: [comment("101", "coderabbitai[bot]", rabbit("🟡 Minor", "Open the Git panel after switching to the branch's chat.", "If the branch belongs to another chat, `openChat(target)` changes only the chat and route. The right panel keeps its previous view, so the branch chip does not open the Git view.\n\nOpen the target chat, then call `openPanel(\"git\")`.", "In @apps/web-solid/src/features/transcript/ReferenceMarkdown.tsx around lines 267 - 270, when the branch belongs to another chat, open that chat and then call openPanel(\"git\") so the Git view shows; keep the current-chat path unchanged."), 8, { diffHunk: "@@ -264,6 +264,10 @@\n   }\n   if (kind === \"branch\") {\n+    if (target === chatId) return openPanel(\"git\");\n+    if (state.chats[target]) return openChat(target);" })] },
    { id: "t2", isResolved: false, isOutdated: false, path: "apps/web-solid/src/features/workspace/files/Files.tsx", line: 188, comments: [comment("102", "maya", "Keep the pending line until the editor shows that file's text. `revealWantedLine` checks the store snapshot, not the editor content.", 6, { diffHunk: "@@ -185,4 +185,6 @@\n     const current = open();\n+    if (!editor || !wantLine || current?.path !== wantLine.path) return;\n+    editor.revealLine(wantLine.line);" }), comment("103", "swarajbachu", "Good catch, fixing.", 5)] },
    { id: "t3", isResolved: true, isOutdated: false, path: "apps/web-solid/src/features/transcript/references.ts", line: 66, comments: [comment("104", "coderabbitai[bot]", rabbit("🟡 Minor", "Count distinct file paths when checking basename ambiguity.", "Deduplicate normalized paths before marking a basename ambiguous.", "In @apps/web-solid/src/features/transcript/references.ts around line 66, deduplicate normalized paths before marking a basename ambiguous."), 20)] },
    { id: "t4", isResolved: false, isOutdated: true, path: "apps/web-solid/src/ui/markdown.ts", line: null, comments: [comment("105", "maya", "Nit: this regex also matches inside code spans.", 25)] },
  ],
  files: Array.from({ length: 6 }, (_, i) => ({ path: `src/file${i}.ts`, additions: 10, deletions: 5 })),
  statusCheckRollup: [
    check("platforms", "COMPLETED", "SUCCESS"), check("checks", "COMPLETED", "SUCCESS"), check("rust", running ? "IN_PROGRESS" : "COMPLETED", running ? "" : "SUCCESS"),
    check("harness-evaluation", "COMPLETED", "SUCCESS"), check("browser", "COMPLETED", "SUCCESS"), check("native (ubuntu-24.04)", running ? "QUEUED" : "COMPLETED", running ? "" : "SUCCESS"),
    check("native (macos-latest)", "COMPLETED", "SUCCESS"), check("CodeRabbit", "COMPLETED", "SUCCESS", false), check("Workers Builds: zuse-website", "COMPLETED", "SUCCESS"),
  ],
  autoMergeRequest: null, viewerLogin: "swarajbachu", mergeMethods: ["squash", "rebase"],
  createdAt: iso(40),
  commits: [{ oid: "1a2b3c4d5e", messageHeadline: "Show failed turns as one quiet line" }, { oid: "2b3c4d5e6f", messageHeadline: "Resume a chat once sign-in lands" }, { oid: "3c4d5e6f70", messageHeadline: "Replace stacked banners" }],
  mergeStateStatus: running ? "BLOCKED" : "CLEAN",
  requestedReviewers: ["alice"],
  linkedIssues: [{ number: 430, title: "Errors are too loud in the transcript", url: "https://github.com/opcodehq/zuse/issues/430", state: "OPEN", isPullRequest: false }],
};
const swaraj = { login: "swarajbachu" };
detail.timeline = [
  { kind: "commit", actor: swaraj, createdAt: iso(39), state: "SUCCESS", commit: { oid: "1a2b3c4d5e", message: "Show failed turns as one quiet line" } },
  { kind: "commit", actor: swaraj, createdAt: iso(38), state: "SUCCESS", commit: { oid: "2b3c4d5e6f", message: "Resume a chat once sign-in lands" } },
  { kind: "labeled", actor: swaraj, createdAt: iso(37), name: "ui", color: "1d76db" },
  { kind: "labeled", actor: swaraj, createdAt: iso(37), name: "web", color: "0e8a16" },
  { kind: "reviewRequested", actor: swaraj, createdAt: iso(36), name: "maya" },
  { kind: "comment", actor: swaraj, createdAt: iso(30), url: `${URL_}#discussion_rc1` },
  { kind: "connected", actor: swaraj, createdAt: iso(14), subject: detail.linkedIssues![0] },
  { kind: "forcePush", actor: swaraj, createdAt: iso(12), from: "2b3c4d5e6f", to: "9f8e7d6c5b" },
  { kind: "commit", actor: swaraj, createdAt: iso(11), state: running ? "PENDING" : "SUCCESS", commit: { oid: "3c4d5e6f70", message: "Replace stacked banners" } },
  { kind: "comment", actor: { login: "cloudflare-workers-and-pages[bot]" }, createdAt: iso(10), url: `${URL_}#discussion_rc2` },
  { kind: "review", actor: { login: "coderabbitai[bot]" }, createdAt: iso(9), state: "COMMENTED", url: `${URL_}#discussion_rr1` },
  { kind: "review", actor: { login: "maya" }, createdAt: iso(7), state: "CHANGES_REQUESTED", url: `${URL_}#discussion_rr2` },
];
detail.timelineTotal = detail.timeline.length + 5;

// A stand-in for the GitHub-backed methods: updates change the fixture, comments and replies append to it.
type Params = { update?: { kind: string; method?: string }; body?: string; threadId?: string };
const calls: { method: string; params: Params }[] = [];
const posted = (body: string) => ({ id: `c${calls.length}`, url: `${URL_}#issuecomment-${calls.length}`, author: { login: "swarajbachu" }, body, state: "", createdAt: new Date().toISOString(), submittedAt: "", diffHunk: "" });
Object.assign(rpc, {
  call: async (method: string, params: Params) => {
    calls.push({ method, params: structuredClone(params) });
    if (method === "GetChangeRequest") return structuredClone(detail);
    if (method === "PostChangeRequestComment") return (detail = { ...detail, comments: [...detail.comments, posted(params.body!)] }), posted(params.body!);
    if (method === "ReplyChangeRequestThread") {
      detail = { ...detail, reviewThreads: detail.reviewThreads!.map((t) => (t.id === params.threadId ? { ...t, comments: [...t.comments, posted(params.body!)] } : t)) };
      return posted(params.body!);
    }
    const update = params.update!;
    if (update.kind === "merge" && detail.statusCheckRollup.some((c) => c.status !== "COMPLETED")) return { refusal: "Required status checks are still running." };
    if (update.kind === "merge") detail = { ...detail, state: "MERGED" };
    if (update.kind === "enableAutoMerge") detail = { ...detail, autoMergeRequest: { mergeMethod: update.method!.toUpperCase() } };
    if (update.kind === "disableAutoMerge") detail = { ...detail, autoMergeRequest: null };
    if (update.kind === "close") detail = { ...detail, state: "CLOSED" };
    if (update.kind === "reopen") detail = { ...detail, state: "OPEN" };
    if (update.kind === "markDraft") detail = { ...detail, isDraft: true };
    if (update.kind === "markReady") detail = { ...detail, isDraft: false };
    return { refusal: null };
  },
  subscribe: () => ({ cancel() {} }),
});
setState(draft => { Object.assign(draft, {
  localDeviceId: DEVICE,
  connection: { kind: "connected" },
  chats: { [CHAT]: { id: CHAT, deviceId: DEVICE, title: "Quieter chat errors", archived: false, cwd: "/repo", createdAt: new Date().toISOString(), config: { harness: "claude-code" } } },
  harnesses: [{ id: "claude-code", name: "Fixture Agent" }],
}); });
setWorkspace(draft => { draft.changeRequests = { [CHAT]: { checkoutId: CHAT, deviceId: DEVICE, cwd: "/repo", branch: detail.headRefName, updatedAt: "", error: null, changeRequest: { provider: "github", number: 425, title: detail.title, url: URL_, state: "open", baseRef: "main", headRef: detail.headRefName } } }; });
setView(draft => { Object.assign(draft, { selectedChatId: CHAT, route: { kind: "chat" } }); draft.preferences.theme = theme; });
flush();

const target = { chatId: CHAT, deviceId: DEVICE, targetDeviceId: null, cwd: "/repo", archived: false, space: null, git: true, local: true, branch: detail.headRefName, repoPath: "/repo" };
Object.assign(window, { __pr: { calls, draft: () => draftFor(CHAT).text } });
render(() => (
  <div class="pr-fixture" style={{ display: "flex", height: "100vh", background: "var(--bg)", color: "var(--text)" }}>
    <section class="ws-surface" id="ws-surface-pullRequest" style={{ width: "480px", flex: "none", "border-right": "1px solid var(--border)" }}>
      <PullRequest target={target} url={URL_} active />
    </section>
    <div style={{ flex: 1, padding: "12px", "min-width": 0 }}>
      <div style={{ "font-size": "var(--text-xs)", color: "var(--text-faint)", "margin-bottom": "6px" }}>Composer draft</div>
      <textarea class="cmp-input" readOnly value={draftFor(CHAT).text} style={{ width: "100%", height: "70vh", background: "transparent", color: "var(--text)", border: "1px solid var(--border)", "border-radius": "8px", padding: "8px", font: "12px/1.5 var(--font-mono)" }} />
    </div>
  </div>
), document.getElementById("root")!);
