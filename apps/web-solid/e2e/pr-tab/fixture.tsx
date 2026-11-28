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
const detail: ChangeRequestDetail = {
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
  reviews: [comment("r1", "coderabbitai[bot]", "", 9, { state: "COMMENTED", submittedAt: iso(9) })],
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
  autoMergeRequest: null, viewerLogin: "swarajbachu",
};

const calls: { method: string; params: unknown }[] = [];
Object.assign(rpc, {
  call: async (method: string, params: unknown) => {
    calls.push({ method, params });
    if (method === "GetChangeRequest") return structuredClone(detail);
    return null;
  },
  subscribe: () => ({ cancel() {} }),
});
setState(draft => { Object.assign(draft, {
  localDeviceId: DEVICE,
  connection: { kind: "connected" },
  chats: { [CHAT]: { id: CHAT, deviceId: DEVICE, title: "Quieter chat errors", archived: false, cwd: "/repo", createdAt: new Date().toISOString(), config: { harness: "claude-code" } } },
  harnesses: [{ id: "claude-code", name: "Fixture Agent" }],
}); });
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
