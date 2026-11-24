import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { rpc } from "@/core/connection";
import { setState } from "@/core/store";
import { setView } from "@/core/view";
import { beginQueueEdit, cancelQueueEdit } from "@/features/composer/queueEdit";
import { PullRequest } from "@/features/workspace/PullRequest";
import { draftFor, setDraft } from "@/features/composer/drafts";
import type { ChangeRequestDetail } from "@/features/workspace/types";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/features/workspace/workspace.css";

const detail: ChangeRequestDetail = {
  title: "Render the widget list faster", body: "Speeds up **rendering**.\n\n- Virtualized rows\n- [Design notes](https://example.com/docs)",
  url: "https://github.com/acme/widgets/pull/7", number: 7, author: { login: "octo" },
  baseRefName: "main", headRefName: "feature/widgets", headRefOid: "a".repeat(40), baseRefOid: "b".repeat(40),
  state: "OPEN", isDraft: false, reviewDecision: "REVIEW_REQUIRED", mergeable: "MERGEABLE", additions: 12, deletions: 3,
  comments: [{ id: "comment", author: { login: "reviewer" }, body: "Please add a regression test.", state: "", createdAt: "2026-11-24T10:00:00Z", submittedAt: "", url: "", diffHunk: "" }], reviews: [],
  reviewThreads: [{ id: "review", isResolved: false, isOutdated: false, path: "src/list.ts", comments: [] }],
  files: [{ path: "src/list.ts", additions: 12, deletions: 3 }],
  statusCheckRollup: [{ name: "build", conclusion: "SUCCESS" }, { name: "lint", conclusion: "FAILURE" }, { name: "e2e", status: "IN_PROGRESS" }],
};
const requests: { method: string; params: unknown }[] = [];
let sink: ((value: unknown) => void) | undefined;
let heldDiff: (() => void) | undefined;
let holdDiff = false;
let patchLine = "virtual row";
const publish = () => { sink?.({ snapshot: structuredClone(detail), refreshing: false }); flush(); };
Object.assign(rpc, {
  call: async (method: string, params: unknown) => {
    requests.push({ method, params });
    if (method === "GetChangeRequest") return structuredClone(detail);
    if (method === "BeginQueuedMessageEdit") return { outcome: "acquired", leaseId: "lease", baseTextHash: "hash", text: "Queued text", attachments: [] };
    if (method === "FinishQueuedMessageEdit") return { outcome: "cancelled" };
    if (method === "GetChangeRequestDiff") {
      const patch = `diff --git a/src/list.ts b/src/list.ts\n--- a/src/list.ts\n+++ b/src/list.ts\n@@ -1 +1 @@\n-old row\n+${patchLine}\n`;
      if (holdDiff) { holdDiff = false; return new Promise(resolve => { heldDiff = () => resolve(patch); }); }
      return patch;
    }
    if (method === "UpdateChangeRequestAtHead") return { refusal: "Merging is blocked by a failing required check." };
    if (method === "UpdateChangeRequest") return {};
    if (method === "PostChangeRequestComment") return {};
    return [];
  },
  subscribe: (method: string, _params: unknown, onItem: (value: unknown) => void) => {
    if (method === "WatchChangeRequest") { sink = onItem; queueMicrotask(publish); }
    return { cancel() { sink = undefined; } };
  },
});
setState(draft => { draft.connection = { kind: "connected" }; draft.queue.chat = [{ id: "queued", text: "Queued text" }]; });
setView(draft => { draft.selectedChatId = "chat"; });
setDraft("chat", { text: "My existing draft" });
document.documentElement.dataset.theme = "dark";
const target = { chatId: "chat", deviceId: "local", targetDeviceId: null, cwd: "/fixture", archived: false, space: null, git: true, local: true, branch: "feature/widgets", repoPath: "/fixture" };
render(() => <main style={{ display: "grid", "grid-template-columns": "minmax(0, 1fr) minmax(340px, 440px)", height: "100vh", color: "var(--text)", background: "var(--bg)", "font-family": "var(--font-sans)" }}>
  <section style={{ padding: "32px", "min-width": 0 }}><h2>Conversation</h2><p>Review the pull request, then continue with an editable question.</p><label for="draft">Chat draft</label><textarea id="draft" class="field cmp-input" style={{ width: "100%", "min-height": "220px", "margin-top": "12px" }} value={draftFor("chat").text} onInput={event => setDraft("chat", { text: event.currentTarget.value })} /></section>
  <PullRequest target={target} active url={detail.url} />
</main>, document.getElementById("root")!);
Object.assign(window, { prFixture: { requests, draft: () => draftFor("chat"), beginEdit: () => beginQueueEdit("chat", { id: "queued", text: "Queued text" }), cancelEdit: cancelQueueEdit, reset: () => { sink?.({ reset: true, snapshot: structuredClone(detail), refreshing: false }); flush(); }, holdDiff: () => { holdDiff = true; }, releaseDiff: () => heldDiff?.(), changeBase: (base: string, line: string) => { detail.baseRefOid = base; patchLine = line; publish(); }, changeHead: () => { detail.headRefOid = "c".repeat(40); publish(); }, reconnect: (connected: boolean) => { setState(draft => { draft.connection = { kind: connected ? "connected" : "reconnecting" }; }); flush(); } } });
flush();
