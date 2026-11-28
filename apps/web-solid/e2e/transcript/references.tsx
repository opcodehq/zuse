// Test-only page: mounts the real Transcript with a Zapper, linked threads, pull requests and a subagent,
// so inline references and the open-ask bar render without an engine. Never bundled into the app.
import { render } from "@solidjs/web";
import "@/ui/tokens.css";
import "@/features/zappers/zappers.css";
import "@/ui/primitives.css";
import "@/shell/shell.css";
import { setState } from "@/core/store";
import { setView, view } from "@/core/view";
import type { SessionMessageEntry } from "@/core/protocol";
import type { IssueStatus, Zapper } from "@/core/zapper-types";
import { selectedSubagent } from "@/features/workspace/agents";
import { pending } from "@/features/workspace/files/model";
import { setWorkspace } from "@/features/workspace/store";
import { Issues } from "@/features/zappers/Issues";
import { panelFor } from "@/features/zappers/panel";
import { flush, Show } from "solid-js";
import { Transcript } from "@/features/transcript";

const CHAT = "c-coordinator";
const IMPL = "c7e1f0a2-9b3d";
const DEVICE = "fixture-device";
document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") ?? "dark";
const now = Date.now();
const ago = (seconds: number) => now - seconds * 1000;
const activity = (id: string, seconds: number, kind: string, text: string, actor: string | null = CHAT) => ({ id, createdAt: ago(seconds), kind, text, actor });
const issue = (id: string, number: number, title: string, status: IssueStatus, extra: Partial<Zapper["issues"][number]> = {}) => ({
  id, number, title, createdAt: 0, updatedAt: 0, threads: [CHAT], pullRequests: [], activity: [],
  details: { title, description: "", status, priority: "normal" as const, assigned: false, dueDate: null, labels: [], relations: [] }, ...extra,
});
const zapper: Zapper = {
  id: "zapper", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: CHAT, previousCoordinatorChats: [], workers: [{ chatId: IMPL, deviceId: DEVICE, spaceId: "s", objective: "Pricing", followups: 0 }], decisions: [],
  config: { name: "Website", purpose: "", homeDeviceId: DEVICE, workerDeviceId: DEVICE, repositories: [], coordinatorModel: { harness: "claude-code", model: null }, maxConcurrentWorkers: 2, maxFollowups: 1 },
  issues: [
    issue("pricing", 18, "Update website pricing", "needsYou", { pullRequests: [{ url: "https://github.com/opcodehq/zuse/pull/378", title: "Update website pricing to the $20 Cloud and $50 Cloud Max plans" }], activity: [
      activity("a-created", 500, "created", "Created the issue", null),
      activity("a-owner", 480, "updated", "Owner: Website · Status changed to In progress"),
      activity("a-pr", 300, "pullRequest", "Linked pull request https://github.com/opcodehq/zuse/pull/378"),
      activity("a-comment", 200, "comment", "Copy is done.", IMPL),
      activity("q-402", 100, "question", "Fix Vercel billing (site 402), then merge zuse#378."),
      activity("a-needs", 99, "updated", "Status changed to Needs you"),
    ] }),
    issue("usage", 17, "Cloud/Tokens toggle on the Usage page", "done", { activity: [activity("a-closed", 90, "closed", "Merged · shipped in zuse#375")] }),
    issue("computers", 21, "Run commands on attached computers", "inProgress", { activity: [activity("a-thread", 400, "thread", "Started thread · In progress")] }),
    issue("waiting", 22, "Vercel billing", "blocked"),
    issue("review", 23, "Release notes dialog", "inReview"),
    issue("todo", 24, "Model picker cleanup", "todo"),
    issue("backlog", 25, "Desktop parity", "backlog"),
    issue("dropped", 26, "Old pricing banner", "canceled"),
  ],
};
const changeRequest = (chatId: string, number: number, title: string, state: "open" | "merged" | "closed") => ({
  checkoutId: chatId, deviceId: DEVICE, cwd: "/repo", branch: "b", updatedAt: "", error: null,
  changeRequest: { provider: "github", number, title, url: `https://github.com/opcodehq/zuse/pull/${number}`, state, baseRef: "main", headRef: "b" },
});
const chat = (id: string, title: string, branch?: string) => ({ id, deviceId: DEVICE, title, branch, archived: false, cwd: "/repo", createdAt: new Date().toISOString(), config: { harness: "claude-code" } });
const messages: SessionMessageEntry[] = [
  { id: "u1", role: "user", createdAt: ago(600), deviceId: DEVICE, status: "complete", parts: [{ kind: "text", id: "u1x", text: "Where are we on the pricing change?\n\nAttached images (local files):\n- /tmp/uploads/1a2b3c4d-pricing-mock.png" }] },
  { id: "a1", role: "assistant", createdAt: ago(120), deviceId: DEVICE, status: "complete", durationMs: 48_000, parts: [
    { kind: "tool", id: "agent", call: { kind: "unknown", name: "Agent: Verify pricing copy" } as never, resolved: true, subagentRef: "c7e1--sub--verify", subagentStatus: "done" },
    { kind: "tool", id: "read", call: { kind: "readFile", path: "/repo/apps/web/src/pricing.tsx" } as never, resolved: true, output: "" },
    { kind: "tool", id: "edit", call: { kind: "editFile", path: "reports/ZUSA-16.md" } as never, resolved: true },
    { kind: "text", id: "a1x", text: [
      "ZAP-18 needs you: please review and merge zuse#378. CI is green at 39d73fe on website-pricing-v2, and it targets main. The copy lives in apps/web/src/pricing.tsx:42 and the write-up is [ZUSA-16.md](zuse-file:reports/ZUSA-16.md).",
      "",
      "The copy was done in " + IMPL + " and double-checked by c7e1--sub--verify. ZAP-17 already shipped as opcodehq/zuse#375 (opcodehq/zuse@e4166aaf), ZAP-21 is still in progress, and #370 was closed in favour of https://github.com/opcodehq/zuse/pull/371. Website matched pricing-mock.png.",
      "",
      "- Unknown things stay plain: ZAP-999, other/zuse#378, src/missing.ts, a1b2c3d and main.",
      "- Code stays code: `ZAP-18`, `apps/web/src/pricing.tsx` and `39d73fe`.",
      "",
      "```text",
      "git log --grep ZAP-18   # zuse#378",
      "```",
    ].join("\n") },
  ] },
];

setState(draft => { Object.assign(draft, {
  localDeviceId: DEVICE,
  connection: { kind: "connecting" },
  chats: { [CHAT]: chat(CHAT, "Website coordinator"), [IMPL]: chat(IMPL, "Pricing page copy", "website-pricing-v2") },
  harnesses: [{ id: "claude-code", name: "Fixture Agent" }],
  zappers: [zapper],
  transcript: { chatId: CHAT, ready: true, messages, contextUsage: null, notice: null },
}); });
setWorkspace(draft => { draft.changeRequests = {
  a: changeRequest("a", 378, "Update website pricing", "open"),
  b: changeRequest("b", 375, "Add Cloud/Tokens toggle", "merged"),
  c: changeRequest("c", 370, "Earlier attempt at remote commands", "closed"),
  d: changeRequest("d", 371, "Let cloud chats run commands on your attached computers", "merged"),
}; });
setView(draft => { Object.assign(draft, { selectedChatId: CHAT, route: { kind: "chat" } }); });
flush();

Object.assign(window, { __refs: {
  status: (number: number, status: IssueStatus) => { setState(draft => { draft.zappers[0]!.issues.find(item => item.number === number)!.details.status = status; }); flush(); },
  destination: () => { const page = panelFor("zapper").active; return { page: page.kind === "issue" ? { kind: "issue", id: page.id } : page.kind === "pr" ? { kind: "pr", url: page.url } : { kind: page.kind }, agent: selectedSubagent(), chat: view.selectedChatId, file: { path: pending.path, line: pending.line }, panel: view.rightPanel, route: view.route.kind }; },
} });
const issuesView = new URLSearchParams(location.search).get("view") === "issues";
render(() => (
  <Show when={issuesView} fallback={<div class="app"><div class="app-main"><main class="app-pane"><div class="app-chat"><Transcript /></div></main></div></div>}>
    <div style={{ width: "460px", padding: "16px", background: "var(--bg)", "min-height": "100vh" }}><Issues zapper={zapper} /></div>
  </Show>
), document.getElementById("root")!);
