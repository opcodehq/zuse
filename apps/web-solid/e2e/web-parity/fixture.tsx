import type { Zapper } from "@/core/zapper-types";
import { setTab, setInspectorOpen } from "@/features/zappers/state";
import { App } from "@/shell/App";
import { view, setView } from "@/core/view";
import { catalog, defaults, setCatalog } from "@/features/composer/catalog";
import { ModelSelect } from "@/ui/ModelSelect";
import "@/shell/shell.css";
import "@/shell/theme";
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
let preview = false;
const sampleZapper: Zapper = {
 id:"captain",revision:1,activityRevision:1,status:"active",coordinatorChatId:"parent",previousCoordinatorChats:[],
 config:{name:"Web captain",purpose:"Keep the browser workspace consistent with desktop. Coordinate design, implementation, and review.",homeDeviceId:"remote",workerDeviceId:"remote",repositories:[{deviceId:"remote",spaceId:"project"}],coordinatorModel:{harness:"codex",model:"gpt-5.3-codex"},maxConcurrentWorkers:3,maxFollowups:2},
 workers:[{chatId:"worker",deviceId:"remote",spaceId:"project",objective:"Align the cloud model picker with desktop",followups:0}],decisions:[],
 issues:[
  {id:"picker",number:12,createdAt:0,updatedAt:0,details:{title:"Match the desktop model picker",description:"Use the provider rail, search, and keyboard navigation from desktop.",status:"inProgress",priority:"high",assigned:true,owner:{type:"zapper"},dueDate:null,labels:["design","web"],relations:[],doneCriteria:"Models can be searched and selected with the keyboard.",outOfScope:"Provider authentication"},threads:["worker"],pullRequests:[{url:"https://github.com/opcodehq/zuse/pull/325",title:"Web parity"}],activity:[]},
  {id:"review",number:13,createdAt:0,updatedAt:0,details:{title:"Review the new workspace layout",description:"Check the inspector at narrow widths.",status:"needsYou",priority:"normal",assigned:false,dueDate:null,labels:["review"],relations:[]},threads:[],pullRequests:[],activity:[]},
  {id:"done",number:11,createdAt:0,updatedAt:0,details:{title:"Use shared desktop icons",description:"",status:"done",priority:"low",assigned:false,dueDate:null,labels:[],relations:[]},threads:[],pullRequests:[],activity:[]},
 ]
};
const codexModels = [{id:"gpt-5.3-codex",label:"GPT-5.3 Codex",reasoningLevels:["low","medium","high","xhigh"]},{id:"gpt-5.2",label:"GPT-5.2"}];
const claudeModels = [{id:"claude-opus-4-6",label:"Opus 4.6",reasoningLevels:["low","medium","high"],options:[{id:"contextWindow",label:"Context window",defaultChoice:"200k",choices:[{id:"200k",label:"200K"},{id:"1m",label:"1M"}]},{id:"speed",label:"Fast mode",defaultChoice:"normal",choices:[{id:"normal",label:"Normal"},{id:"fast",label:"Fast"}]}]},{id:"claude-sonnet-4-6",label:"Sonnet 4.6"}];
const requests: { method: string; params: unknown }[] = [];
const message = { id: "reply", role: "assistant", deviceId: "remote", createdAt: 0, status: "complete", parts: [{ kind: "text", id: "text", text: "Subagent streamed reply" }] };
Object.assign(rpc, {
  call: async (method: string, params: unknown) => {
    requests.push({ method, params });
    if (method === "ZotEvents") return [];
    if (method === "ZotFiles") return [{path:"INSTRUCTIONS.md",version:1,content:"Generated instructions"},{path:"notes/plan.md",version:1,content:"# First version"},{path:"notes/plan.md",version:2,content:"# Current plan\n\nShip the desktop-aligned Zapper panel."}];
    if (method === "ZotCommand") {
      const request = params as {action:{type:string;issueId:string;details:Zapper["issues"][number]["details"]}};
      const next = structuredClone(sampleZapper);
      if (request.action.type === "configure") next.config = (request.action as unknown as {config:Zapper["config"]}).config;
      if (request.action.type === "editIssue") next.issues = next.issues.map(issue => issue.id === request.action.issueId ? {...issue,details:request.action.details} : issue);
      next.revision++;
      Object.assign(sampleZapper,next);
      return next;
    }
    if (method === M.LIST_MODELS) return (params as {harness:string}).harness === "claude-code" ? claudeModels : codexModels;
    if (method === M.FETCH_TOOL_BLOB) { if (snapshot) return { text: JSON.stringify([message]) }; throw new Error("live doc"); }
    throw new Error("Fixture has no paid services");
  },
  subscribe: (method: string, params: unknown, onItem: (value: unknown) => void) => {
    requests.push({ method, params });
    if (method === M.WATCH_DOC_MESSAGES) {
      sink = onItem;
      const parentMessages = [
        {id:"request",role:"user",createdAt:Date.now(),deviceId:"remote",parts:[{kind:"text",id:"request-text",text:"Bring the cloud UI in sync with desktop. Start with the model picker and agent panels."}]},
        {...message,parts:[{kind:"text",id:"text",text:"I’m comparing the desktop controls and bringing the same layout, spacing, and interactions to web."},{kind:"tool",id:"spawn",call:{kind:"unknown",name:"Agent: Review model picker",input:{model:"gpt-5.3-codex"}},subagentRef:"parent--sub--worker",subagentStatus:"running"}]},
      ];
      const workerMessages = [{...message,parts:[{kind:"text",id:"text",text:"The desktop picker uses a compact provider rail, grouped models, and a search field. I’m checking the web menu against those dimensions.\n\n- Provider icons and selection states\n- Model row spacing and keyboard navigation\n- Reasoning and fast-mode controls"}]}];
      queueMicrotask(() => onItem({ reset: preview ? (params as {chatId:string}).chatId === "parent" ? parentMessages : workerMessages : [message] }));
    }
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
  const [model, setModel] = createSignal("gpt");
  const [active, setActive] = createSignal(true);
  Object.assign(window, { parity: {
    hosted: () => { setHostedBrowser(true); flush(); },
    local: () => { setHostedBrowser(false); flush(); },
    agents: () => { setMode("agents"); openSubagent("parent", "parent--sub--worker"); flush(); },
    models: () => { setMode("models"); flush(); },
    model: () => model(),
    workspace: () => {
      preview = true; snapshot = false;
      setHostedBrowser(false);
      setView(draft => { draft.route = {kind:"chat"}; draft.selectedChatId = "parent"; draft.rightPanel = "agents"; });
      setState(draft => {
        draft.workspaceSynced = true; draft.localDeviceId = "remote"; draft.zappersReady = true;
        draft.harnesses = [{id:"codex",name:"Codex",installed:true,enabled:true},{id:"claude-code",name:"Claude Code",installed:true,enabled:true}];
        draft.spaces.project = {id:"project",name:"zuse",path:"/workspace/zuse",deviceId:"remote",gitDetected:true};
        draft.chats.parent = {...draft.chats.parent!,title:"Bring web in sync with desktop",cwd:"/workspace/zuse",spaceId:"project",branch:"web-parity",config:{harness:"codex",model:"gpt-5.3-codex",sandbox:"auto"}};
      });
      setMode("app"); flush();
      setCatalog(draft => { draft.models["local|codex"] = {status:"ready",value:[{id:"gpt-5.3-codex",label:"GPT-5.3 Codex",reasoningLevels:["low","medium","high","xhigh"]},{id:"gpt-5.2",label:"GPT-5.2"}]}; draft.models["local|claude-code"] = {status:"ready",value:[{id:"claude-opus-4-6",label:"Opus 4.6",reasoningLevels:["low","medium","high"],options:[{id:"contextWindow",label:"Context window",defaultChoice:"200k",choices:[{id:"200k",label:"200K"},{id:"1m",label:"1M"}]},{id:"speed",label:"Fast mode",defaultChoice:"normal",choices:[{id:"normal",label:"Normal"},{id:"fast",label:"Fast"}]}]},{id:"claude-sonnet-4-6",label:"Sonnet 4.6"}]}; });
      openSubagent("parent", "parent--sub--worker"); flush();
    },
    zapper: () => {
      preview = true;
      setHostedBrowser(false); setMode("app");
      setState(draft => { draft.zappers = [structuredClone(sampleZapper)]; draft.zappersReady = true; draft.zappersSupported = true; draft.chats.worker = {...draft.chats.parent!,id:"worker",title:"Align cloud model picker"}; });
      setView(draft => { draft.route = {kind:"zappers",zapperId:"captain"}; draft.selectedChatId="parent"; draft.rightPanel=null; });
      setInspectorOpen(true); flush(); setTab("issues"); flush();
    },
    queued: () => { setState(draft=>{draft.zappers[0]!.workers[0]!.queuePosition=2;draft.zappers[0]!.issues[0]!.details.owner={type:"thread",chatId:"worker"};delete draft.chats.worker;}); flush(); },
    startQueued: () => {setState(draft=>{delete draft.zappers[0]!.workers[0]!.queuePosition;draft.chats.worker={...draft.chats.parent!,id:"worker",title:"Ready worker"};});flush();},
    openQueued: () => {setView(draft=>{draft.selectedChatId="worker";draft.route={kind:"zappers",zapperId:"captain"};});flush();},
    openAgentPage: () => {openSubagent("parent","parent--sub--worker");flush();},
    reviewChanges: () => {setView(draft=>{draft.rightPanel="changes";});flush();},
    configureZapper: () => {setView(draft=>{draft.route={kind:"newZapper",zapperId:"captain"};});flush();},
    panels: () => { setView(draft => { draft.rightPanel="files"; }); flush(); },
    newChatPicker: () => { setView(draft => { draft.selectedChatId=null; draft.route={kind:"chat"}; draft.draftProjectPath="/workspace/zuse"; draft.rightPanel=null; }); flush(); },
    pickerState: () => ({ draft: catalog.draft, defaults: defaults(), explicit: view.preferences.defaultModel }),
    hide: () => { setActive(false); flush(); },
    reconnect: () => { setState(draft => { draft.epoch++; }); flush(); },
    snapshot: () => { snapshot = true; setActive(true); flush(); },
    append: () => { sink?.({ append: [{ entry: "reply", part: "text", text: " updated", len: 31 }], count: 1 }); flush(); },
    counters: () => ({ cancellations, requests }),
    icons: () => ["git-merge", "menu-dots", "tool-agent", "git-branch", "pull-request"].every(hasIcon),
  } });
  return <Show when={mode() !== "app"} fallback={<App />}><Show when={mode() !== "models"} fallback={<div style={{padding:"100px",height:"100vh",background:"var(--bg)"}}><ModelSelect label="Model" value={model()} onChange={setModel} options={[
    {key:"gpt",label:"GPT-5.3 Codex",provider:"codex",providerLabel:"Codex"},
    {key:"gpt52",label:"GPT-5.2",provider:"codex",providerLabel:"Codex"},
    {key:"opus",label:"Opus 4.6",provider:"claude-code",providerLabel:"Claude Code"},
    {key:"sonnet",label:"Sonnet 4.6",provider:"claude-code",providerLabel:"Claude Code"},
  ]} /></div>}><Show when={mode() === "onboarding"} fallback={<Agents chatId="parent" active={active()} />}><WebOnboarding><div id="workspace">Workspace</div></WebOnboarding></Show></Show></Show>;
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
