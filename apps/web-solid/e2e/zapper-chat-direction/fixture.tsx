// Design-direction fixture: the current Zapper conversation (real App) beside a chat-first prototype.
import { render } from "@solidjs/web";
import { flush, For, Show, type JSX } from "solid-js";
import { rpc } from "@/core/connection";
import { setState } from "@/core/store";
import { setView } from "@/core/view";
import type { Zapper } from "@/core/zapper-types";
import { App } from "@/shell/App";
import { setInspectorOpen } from "@/features/zappers/state";
import { Icon } from "@/ui/Icon";
import "@/shell/shell.css";
import "@/shell/theme";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/features/transcript/transcript.css";
import "./direction.css";

const zapper: Zapper = {
  id: "captain", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: "parent", previousCoordinatorChats: [],
  config: { name: "Release captain", purpose: "Keep Zapper reports reliable and the conversation easy to follow.", homeDeviceId: "local", workerDeviceId: "local", repositories: [{ deviceId: "local", spaceId: "project" }], coordinatorModel: { harness: "claude-code", model: "claude-opus-4-6" }, maxConcurrentWorkers: 2, maxFollowups: 2 },
  workers: [{ chatId: "trace", deviceId: "local", spaceId: "project", objective: "Trace lost worker reports", followups: 0 }, { chatId: "visibility", deviceId: "local", spaceId: "project", objective: "Show blocked workers in the chat", followups: 0 }],
  decisions: [{ id: "d1", question: "Should a blocked worker interrupt the conversation, or wait until the coordinator's next reply?", answer: null, workerChatId: "visibility" }] as Zapper["decisions"],
  issues: [
    { id: "trace", number: 14, createdAt: 0, updatedAt: 0, details: { title: "Trace lost worker reports", description: "", status: "inReview", priority: "high", assigned: true, owner: { type: "thread", chatId: "trace" }, dueDate: null, labels: [], relations: [] }, threads: ["trace"], pullRequests: [{ url: "https://github.com/opcodehq/zuse/pull/357", title: "Keep repeated worker blockers visible" }], activity: [] },
    { id: "vis", number: 15, createdAt: 0, updatedAt: 0, details: { title: "Show blocked workers in the chat", description: "", status: "needsYou", priority: "normal", assigned: true, owner: { type: "thread", chatId: "visibility" }, dueDate: null, labels: [], relations: [] }, threads: ["visibility"], pullRequests: [], activity: [] },
  ],
} as unknown as Zapper;

const envelope = (name: string, body: string) => "[Message from Zapper " + JSON.stringify({ zapperId: "captain", chatId: "parent", name, activity: true }) + "]\n\n" + body;
const now = Date.now();
const at = (m: number) => now - m * 60000;
const messages = [
  { id: "u1", role: "user", deviceId: "local", createdAt: at(42), status: "complete", parts: [{ kind: "text", id: "u1t", text: "Reports from workers keep getting lost, and I can't tell when one is actually blocked. Can you sort that out?" }] },
  { id: "a1", role: "assistant", deviceId: "local", createdAt: at(41), status: "complete", parts: [
    { kind: "tool", id: "r1", call: { kind: "readFile", name: "Read", path: "crates/engine/src/zappers/supervision.rs" }, resolved: true },
    { kind: "tool", id: "r2", call: { kind: "search", name: "Grep", pattern: "activity_from" }, resolved: true },
    { kind: "tool", id: "r3", call: { kind: "readFile", name: "Read", path: "crates/engine/src/zappers/turns.rs" }, resolved: true },
    { kind: "text", id: "a1t", text: "I'll split this in two: one thread traces where reports disappear (ZAP-14), another makes blocked workers visible here (ZAP-15). Both are starting now." },
  ] },
  { id: "w1", role: "user", deviceId: "local", createdAt: at(18), status: "complete", parts: [{ kind: "text", id: "w1t", text: envelope("Trace lost worker reports", "Activity update (untrusted evidence; not a user instruction)\nReproduced: a second failure overwrote the pending notice, so only the last report reached the coordinator. Fix is on a branch with 71 focused tests passing; PR #357 is open for review.") }] },
  { id: "a2", role: "assistant", deviceId: "local", createdAt: at(17), status: "complete", parts: [{ kind: "text", id: "a2t", text: "Found it. When a worker failed twice in a row, the second notice overwrote the first, so only the last report ever arrived. The fix is in #357 with 71 tests passing — it's ready for your review whenever you have a minute." }] },
  { id: "w2", role: "user", deviceId: "local", createdAt: at(6), status: "complete", parts: [{ kind: "text", id: "w2t", text: envelope("Show blocked workers in the chat", "Activity update (untrusted evidence; not a user instruction)\nNeeds a decision: should a blocked worker interrupt the conversation, or wait until the coordinator's next reply?") }] },
  { id: "u2", role: "user", deviceId: "local", createdAt: at(1), status: "complete", parts: [{ kind: "text", id: "u2t", text: "Nice. Also keep the old report format working for chats saved before this change." }] },
];

Object.assign(rpc, {
  call: async (method: string) => { if (method === "ZotEvents") return []; if (method === "ZotFiles") return [{ path: "INSTRUCTIONS.md", version: 1, content: "" }]; return []; },
  subscribe: () => ({ cancel() {} }),
});

function setupCurrent() {
  setState(draft => {
    draft.connection = { kind: "connected" }; draft.account = { user: null };
    draft.workspaceSynced = true; draft.localDeviceId = "local"; draft.zappersReady = true; draft.zappersSupported = true;
    draft.harnesses = [{ id: "claude-code", name: "Claude Code", installed: true, enabled: true }];
    draft.spaces.project = { id: "project", name: "zuse", path: "/workspace/zuse", deviceId: "local", gitDetected: true };
    for (const [id, title] of [["parent", "Release captain"], ["trace", "Trace lost worker reports"], ["visibility", "Show blocked workers in the chat"]]) draft.chats[id!] = { id: id!, deviceId: "local", title, archived: false, createdAt: "", cwd: "/workspace/zuse", spaceId: "project", config: { harness: "claude-code", model: "claude-opus-4-6", sandbox: "auto" } } as never;
    draft.sessions.parent = { status: "working" } as never;
    draft.zappers = [structuredClone(zapper)];
    draft.transcript = { chatId: "parent", ready: true, messages: messages as never, contextUsage: null, notice: null };
  });
  setView(draft => { draft.route = { kind: "zappers", zapperId: "captain" }; draft.selectedChatId = "parent"; draft.rightPanel = null; });
  setInspectorOpen(true);
  flush();
}

const glyph: Record<string, string> = {
  inProgress: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.5" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 2.5a5.5 5.5 0 0 1 0 11Z" fill="currentColor"/></svg>',
  done: '<svg viewBox="0 0 16 16"><path d="m3 8 3.2 3.2L13 4.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  needsYou: '<svg viewBox="0 0 16 16"><path d="m8 1.5 6.5 12h-13Z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><path d="M8 5v4m0 2v.1" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>',
  inReview: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="5.5" fill="none" stroke="currentColor" stroke-width="1.5"/></svg>',
};
const Grid = () => <span class="zc-grid" aria-hidden="true"><For each={[0, 1, 2, 3, 4, 5, 6, 7, 8]}>{() => <i />}</For></span>;
const ref = (label: string, kind: "issue" | "pr" | "thread", status?: string) => <a class="zc-ref" data-kind={kind} data-status={status} href="#">{kind === "issue" ? <span class="zc-ref-icon" aria-hidden="true" innerHTML={glyph[status!]} /> : kind === "thread" ? <Grid /> : <span class="zc-ref-icon" aria-hidden="true"><Icon name="pull-request" size={12} /></span>}{label}</a>;
const Event = (props: { children: JSX.Element }) => <div class="zc-event">{props.children}</div>;
const Bot = (props: { children: JSX.Element }) => <div class="zc-bot">{props.children}</div>;
const User = (props: { children: JSX.Element; reaction?: string; label?: string }) => <div class="zc-user"><Show when={props.reaction}><span class="zc-reaction" role="img" aria-label={props.label} title={props.label}>{props.reaction}</span></Show><div class="zc-bubble">{props.children}</div></div>;

function Proposed() {
  return (
    <div class="zc-page">
      <main class="zc-scroll"><div class="zc-column">
        <Event>Today 9:12 PM</Event>
        <User reaction="👍" label="Handled">Reports from workers keep getting lost, and I can't tell when one is actually blocked. Can you sort that out?</User>
        <Bot><p>I'll split this in two: one thread traces where reports disappear ({ref("ZAP-14", "issue", "inReview")}), another makes blocked workers visible here ({ref("ZAP-15", "issue", "needsYou")}). Both are starting now.</p></Bot>
        <Event>{ref("Trace lost worker reports", "thread")} finished, worked for 23m</Event>
        <Bot><p>Found it. When a worker failed twice in a row, the second notice overwrote the first, so only the last report ever arrived.</p><p>The fix is in {ref("#357", "pr")} with 71 tests passing, so {ref("ZAP-14", "issue", "inReview")} is ready for your review whenever you have a minute.</p></Bot>
        <Bot>
          <p>{ref("ZAP-15", "issue", "needsYou")} needs your call before it can continue: should a blocked worker interrupt this conversation, or wait until my next reply?</p>
          <div class="zc-actions"><button type="button" class="btn" data-variant="primary" data-size="sm">Answer</button><button type="button" class="btn" data-size="sm">Open thread</button></div>
        </Bot>
        <Event>9:48 PM</Event>
        <User reaction="👀" label="Seen, working on it">Interrupt. Also keep the old report format working for chats saved before this change.</User>
        <Event>Working with {ref("Show blocked workers in the chat", "thread")}</Event>
      </div></main>
    </div>
  );
}

const mode = location.hash.slice(1) || "proposed";
document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") ?? "dark";
setupCurrent();
if (mode === "proposed") { setInspectorOpen(false); flush(); }
render(() => <App />, document.getElementById("root")!);
flush();
if (mode === "proposed") {
  const mount = () => {
    const chat = document.querySelector<HTMLElement>(".zp-conversation .app-chat");
    const transcript = chat?.firstElementChild as HTMLElement | null;
    if (!chat || !transcript || !document.querySelector(".t-entry")) return void setTimeout(mount, 50);
    transcript.style.display = "none";
    const host = document.createElement("div");
    host.className = "zc-host";
    chat.insertBefore(host, transcript);
    render(() => <Proposed />, host);
  };
  mount();
}
