import { render } from "@solidjs/web";
import { createSignal, flush } from "solid-js";
import type { IssueStatus, Zapper } from "@/core/zapper-types";
import { setState } from "@/core/store";
import { ConversationMarkdown } from "@/features/transcript/ConversationMarkdown";
import { Markdown } from "@/features/transcript/Markdown";
import { panelFor } from "@/features/zappers/panel";
import { selectedSubagent } from "@/features/workspace/agents";
import { view } from "@/core/view";
import "@/ui/tokens.css";
import "@/features/transcript/transcript.css";

const owner: Zapper = {
  id: "owner", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: "parent", previousCoordinatorChats: [], workers: [], decisions: [],
  config: { name: "Owner", purpose: "", homeDeviceId: "device", workerDeviceId: "device", repositories: [], coordinatorModel: { harness: "codex", model: null }, maxConcurrentWorkers: 1, maxFollowups: 1 },
  issues: [
    { id: "progress", number: 12, createdAt: 0, updatedAt: 0, details: { title: "Improve conversation references", description: "", status: "inProgress", priority: "normal", assigned: false, dueDate: null, labels: [], relations: [] }, threads: ["linked-thread"], pullRequests: [{ title: "Inline references", url: "https://github.com/owner/repo/pull/34" }], activity: [] },
    { id: "completed", number: 11, createdAt: 0, updatedAt: 0, details: { title: "Preserve existing Markdown", description: "", status: "done", priority: "normal", assigned: false, dueDate: null, labels: [], relations: [] }, threads: [], pullRequests: [], activity: [] },
  ],
};
setState(draft => {
  draft.zappers = [owner];
  for (const id of ["parent", "linked-thread"]) draft.chats[id] = { id, deviceId: "device", title: "Linked implementation thread", archived: false, createdAt: "" };
  draft.transcript = { chatId: "parent", ready: true, messages: [{ id: "entry", deviceId: "device", role: "assistant", createdAt: 0, parts: [{ kind: "tool", id: "agent", call: { kind: "unknown", name: "Agent: Verify" }, subagentRef: "parent--sub--verify" }] }], contextUsage: null, notice: null };
});
const initial = "I’m working on ZAP-12, with ZAP-11 already complete. The implementation lives in linked-thread and owner/repo#34; parent--sub--verify has the supporting conversation. Unknown ZAP-999 and Subagent 1 stay ordinary text.\n\n- Track **ZAP-12** without interrupting the sentence.\n- Keep [ZAP-12](https://example.com) and `ZAP-12` unchanged.\n\n| Work | Pull request |\n| --- | --- |\n| ZAP-11 | #34 |\n\n```text\nZAP-12 #34 parent--sub--verify\n```";
function Fixture() {
  const [text, setText] = createSignal(initial);
  Object.assign(window, { referenceFixture: {
    text: (value: string) => { setText(value); flush(); },
    status: (value: IssueStatus) => { setState(draft => { draft.zappers[0]!.issues[0]!.details.status = value; }); flush(); },
    title: (value: string) => { setState(draft => { draft.zappers[0]!.issues[0]!.details.title = value; }); flush(); },
    destination: () => { const page = panelFor("owner").active; return { page: { kind: page.kind, id: page.kind === "issue" ? page.id : undefined }, agent: selectedSubagent(), chat: view.selectedChatId }; },
  } });
  return <main style={{ "max-width": "700px", margin: "70px auto", padding: "24px", background: "var(--bg)", color: "var(--text)" }}>
    <div class="t-root"><div class="t-entry" data-role="assistant"><ConversationMarkdown text={text()} live={false} chatId="parent" /></div></div>
    <div id="generic" hidden><Markdown text="ZAP-12" live={false} /></div>
  </main>;
}
document.body.style.background = "var(--bg)";
render(() => <Fixture />, document.getElementById("root")!);
