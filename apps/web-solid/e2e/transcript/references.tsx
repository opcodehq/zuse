// Test-only page: mounts the real Transcript with a Zapper, linked threads, pull requests and a subagent,
// so inline references and the open-ask bar render without an engine. Never bundled into the app.
import { render } from "@solidjs/web";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/shell/shell.css";
import { setState } from "@/core/store";
import { setView, view } from "@/core/view";
import type { SessionMessageEntry } from "@/core/protocol";
import type { IssueStatus, Zapper } from "@/core/zapper-types";
import { selectedSubagent } from "@/features/workspace/agents";
import { setWorkspace } from "@/features/workspace/store";
import { panelFor } from "@/features/zappers/panel";
import { flush } from "solid-js";
import { Transcript } from "@/features/transcript";

const CHAT = "c-coordinator";
const IMPL = "c7e1f0a2-9b3d";
const DEVICE = "fixture-device";
document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") ?? "dark";
const issue = (id: string, number: number, title: string, status: IssueStatus, extra: Partial<Zapper["issues"][number]> = {}) => ({
  id, number, title, createdAt: 0, updatedAt: 0, threads: [CHAT], pullRequests: [], activity: [],
  details: { title, description: "", status, priority: "normal" as const, assigned: false, dueDate: null, labels: [], relations: [] }, ...extra,
});
const zapper: Zapper = {
  id: "zapper", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: CHAT, previousCoordinatorChats: [], workers: [{ chatId: IMPL, deviceId: DEVICE, spaceId: "s", objective: "Pricing", followups: 0 }], decisions: [],
  config: { name: "Website", purpose: "", homeDeviceId: DEVICE, workerDeviceId: DEVICE, repositories: [], coordinatorModel: { harness: "claude-code", model: null }, maxConcurrentWorkers: 2, maxFollowups: 1 },
  issues: [
    issue("pricing", 18, "Update website pricing", "needsYou", { pullRequests: [{ url: "https://github.com/opcodehq/zuse/pull/378", title: "Update website pricing to the $20 Cloud and $50 Cloud Max plans" }], activity: [{ id: "q-402", createdAt: 1, kind: "question", text: "Fix Vercel billing (site 402), then merge zuse#378." }] }),
    issue("usage", 17, "Cloud/Tokens toggle on the Usage page", "done"),
    issue("computers", 21, "Run commands on attached computers", "inProgress"),
  ],
};
const changeRequest = (chatId: string, number: number, title: string, state: "open" | "merged" | "closed") => ({
  checkoutId: chatId, deviceId: DEVICE, cwd: "/repo", branch: "b", updatedAt: "", error: null,
  changeRequest: { provider: "github", number, title, url: `https://github.com/opcodehq/zuse/pull/${number}`, state, baseRef: "main", headRef: "b" },
});
const chat = (id: string, title: string) => ({ id, deviceId: DEVICE, title, archived: false, cwd: "/repo", createdAt: new Date().toISOString(), config: { harness: "claude-code" } });
const now = Date.now();
const messages: SessionMessageEntry[] = [
  { id: "u1", role: "user", createdAt: now - 120_000, deviceId: DEVICE, status: "complete", parts: [{ kind: "text", id: "u1x", text: "Where are we on the pricing change?" }] },
  { id: "a1", role: "assistant", createdAt: now - 60_000, deviceId: DEVICE, status: "complete", durationMs: 48_000, parts: [
    { kind: "tool", id: "agent", call: { kind: "unknown", name: "Agent: Verify pricing copy" } as never, resolved: true, subagentRef: "c7e1--sub--verify", subagentStatus: "done" },
    { kind: "text", id: "a1x", text: [
      "ZAP-18 needs you: please review and merge zuse#378. It's the pricing fix. I checked it myself: CI is green at `39d73fe`, it doesn't conflict with main, and it targets main. Say merge and I'll land it.",
      "",
      "The copy was done in " + IMPL + " and double-checked by c7e1--sub--verify. ZAP-17 already shipped as opcodehq/zuse#375, ZAP-21 is still in progress, and #370 was closed in favour of https://github.com/opcodehq/zuse/pull/371.",
      "",
      "- Unknown codes such as ZAP-999 or other/zuse#378 stay plain.",
      "- Code stays code: `ZAP-18` and `zuse#378`.",
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
  chats: { [CHAT]: chat(CHAT, "Website coordinator"), [IMPL]: chat(IMPL, "Pricing page copy") },
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
  destination: () => { const page = panelFor("zapper").active; return { page: page.kind === "issue" ? { kind: "issue", id: page.id } : page.kind === "pr" ? { kind: "pr", url: page.url } : { kind: page.kind }, agent: selectedSubagent(), chat: view.selectedChatId }; },
} });
render(() => (
  <div class="app"><div class="app-main"><main class="app-pane"><div class="app-chat"><Transcript /></div></main></div></div>
), document.getElementById("root")!);
