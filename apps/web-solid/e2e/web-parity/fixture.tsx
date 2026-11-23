import { render } from "@solidjs/web";
import { createSignal, flush, Show } from "solid-js";
import { setHostedBrowser } from "@/core/auth";
import { setState } from "@/core/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { WebOnboarding } from "@/features/connection/WebOnboarding";
import { Agents } from "@/features/workspace/Agents";
import { openSubagent } from "@/features/workspace/agents";
import { Icon, hasIcon } from "@/ui/Icon";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/features/workspace/workspace.css";

let sink: ((value: unknown) => void) | undefined;
let cancellations = 0;
let snapshot = false;
const requests: { method: string; params: unknown }[] = [];
const message = { id: "reply", role: "assistant", deviceId: "remote", createdAt: 0, status: "complete", parts: [{ kind: "text", id: "text", text: "Subagent streamed reply" }] };
Object.assign(rpc, {
  call: async (method: string, params: unknown) => {
    requests.push({ method, params });
    if (method === M.FETCH_TOOL_BLOB) { if (snapshot) return { text: JSON.stringify([message]) }; throw new Error("live doc"); }
    throw new Error("Fixture has no paid services");
  },
  subscribe: (method: string, params: unknown, onItem: (value: unknown) => void) => {
    requests.push({ method, params });
    if (method === M.WATCH_DOC_MESSAGES) { sink = onItem; queueMicrotask(() => onItem({ reset: [message] })); }
    return { cancel: () => { cancellations++; } };
  },
});
setState(draft => {
  draft.connection = { kind: "connected" };
  draft.account = { user: null };
  draft.chats.parent = { id: "parent", deviceId: "remote", archived: false, createdAt: "", config: { harness: "codex", sandbox: "auto" } };
  draft.transcript = { chatId: "parent", ready: true, messages: [{ ...message, role: "assistant", parts: [{ kind: "tool", id: "spawn", call: { kind: "unknown", name: "Agent: Inspect web" }, subagentRef: "parent--sub--worker", subagentStatus: "running" }] }], contextUsage: null, notice: null };
});
function Fixture() {
  const [mode, setMode] = createSignal("onboarding");
  const [active, setActive] = createSignal(true);
  Object.assign(window, { parity: {
    hosted: () => { setHostedBrowser(true); flush(); },
    local: () => { setHostedBrowser(false); flush(); },
    agents: () => { setMode("agents"); openSubagent("parent", "parent--sub--worker"); flush(); },
    hide: () => { setActive(false); flush(); },
    reconnect: () => { setState(draft => { draft.epoch++; }); flush(); },
    snapshot: () => { snapshot = true; setActive(true); flush(); },
    append: () => { sink?.({ append: [{ entry: "reply", part: "text", text: " updated", len: 31 }], count: 1 }); flush(); },
    counters: () => ({ cancellations, requests }),
    icons: () => ["git-merge", "menu-dots", "tool-agent", "git-branch", "pull-request"].every(hasIcon),
  } });
  return <><Icon name="git-merge" /><Show when={mode() === "onboarding"} fallback={<Agents chatId="parent" active={active()} />}><WebOnboarding><div id="workspace">Workspace</div></WebOnboarding></Show></>;
}
render(() => <Fixture />, document.getElementById("root")!);

// Exercise real RPC framing through a simulated controller socket, including
// cancellation and account revocation. No cloud allocation or credentials.
import { CloudTransport } from "@/features/connection/cloud";
import { OwnerTransport } from "@/core/transport";
Object.assign(window, { checkCloudTransport: async () => {
  const originalOpen = OwnerTransport.prototype.open;
  const originalSend = OwnerTransport.prototype.send;
  const originalClose = OwnerTransport.prototype.close;
  const frames: Record<string, unknown>[] = [];
  let socket: OwnerTransport;
  let accountId = 0;
  let closes = 0;
  OwnerTransport.prototype.open = async function () { socket = this; };
  OwnerTransport.prototype.close = function () { closes++; };
  OwnerTransport.prototype.send = function (text) {
    const frame = JSON.parse(text);
    frames.push(frame);
    if (frame.cancel) return;
    queueMicrotask(() => {
      if (frame.method === M.CLOUD_ACCOUNT_STATUS) { accountId = frame.id; this.onMessage?.(JSON.stringify({ id: frame.id, item: { user: { id: "actor" } } })); }
      if (frame.method === M.CLOUD_ENVIRONMENT) this.onMessage?.(JSON.stringify({ id: frame.id, item: { state: "ready", epoch: 1 } }));
      if (frame.method === M.CLOUD_RPC) this.onMessage?.(JSON.stringify({ id: frame.id, item: { id: frame.params.requestId, ok: { result: "engine reply" } } }));
    });
  };
  const cloud = new CloudTransport({ workspaceId: "workspace", accountId: "actor", organizationId: null });
  const replies: unknown[] = [];
  let revoked = false;
  cloud.onMessage = text => replies.push(JSON.parse(text));
  cloud.onClose = () => { revoked = true; };
  try {
    await cloud.open();
    cloud.send(JSON.stringify({ id: 42, method: "WatchChats", params: {} }));
    await new Promise(resolve => setTimeout(resolve, 0));
    cloud.send(JSON.stringify({ id: 42, cancel: true }));
    cloud.send(JSON.stringify({ id: 43, method: "QueueMessage", params: { chatId: "chat", text: "Build this" } }));
    await new Promise(resolve => setTimeout(resolve, 0));
    socket!.onMessage?.(JSON.stringify({ id: accountId, item: { user: { id: "another-account" } } }));
    return { frames, replies, revoked, closes };
  } finally {
    cloud.close();
    OwnerTransport.prototype.open = originalOpen;
    OwnerTransport.prototype.send = originalSend;
    OwnerTransport.prototype.close = originalClose;
  }
} });
