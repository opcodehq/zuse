import { targetForChat, workspace, type Target } from "@/features/workspace/store";
import { panelFor, pageKey, showPage, closePage } from "./panel";
import { workerStatus } from "./threads-model";
import { createEffect, createMemo, createSignal, For, Show, lazy } from "solid-js";
import { state } from "@/core/store";
import { navigate, openChat, setView, view } from "@/core/view";
import type { Zapper, ZapperDecision, ZapperAction } from "@/core/zapper-types";
import { Composer } from "@/features/composer";
import { Transcript } from "@/features/transcript";
import { TopBar } from "@/features/workspace";
import { Button, Dialog, Menu } from "@/ui/primitives";
import { Icon } from "@/ui/Icon";
import { Avatar, avatarOf, moodOf } from "./Avatar";
import { now } from "@/features/sidebar/util";
import { Issues, IssuePage } from "./Issues";
import { Threads } from "./Threads";
import { Drive, FileDialogs } from "./Drive";
import {
  act,
  command,
  commandState,
  inspectorOpen,
  loadResources,
  openZapper,
  selectedZapper,
  setInspectorOpen,
  setTab,
  statusLabel,
  tab,
  type Tab,
} from "./state";
import { CommandAlert } from "./Alert";
import "./zappers.css";
const Agents=lazy(()=>import("@/features/workspace/Agents").then(module=>({default:module.Agents})));
const Changes=lazy(()=>import("@/features/workspace/Changes").then(module=>({default:module.Changes})));
const PullRequest=lazy(()=>import("@/features/workspace/PullRequest").then(module=>({default:module.PullRequest})));

export function ZapperPage() {
  createEffect(()=>[selectedZapper()?.id,view.selectedChatId,view.rightPanel] as const,([id,chatId,panel])=>{
    if(!id || !chatId) return;
    const url=workspace.changeRequests[chatId]?.changeRequest?.url;
    if(panel === "changes" || (panel === "pullRequest" && url)) {
      showPage(id,panel === "changes" ? {kind:"diff",chatId} : {kind:"pr",chatId,url:url!});
      setInspectorOpen(true);setView(draft=>{draft.rightPanel=null;});
    }
  });
  createEffect(
    () => {
      const z = selectedZapper();
      return z
        ? `${z.id}:${z.revision}:${z.activityRevision}:${state.epoch}`
        : "";
    },
    () => {
      const z = selectedZapper();
      if (z) void loadResources(z);
    },
  );
  return (
    <>
      <CommandAlert />
      <Show when={selectedZapper()} fallback={<Directory />}>
        {(z) => (
          <div class="zp-page" data-inspector={inspectorOpen() && !view.rightPanel ? "true" : undefined}>
            <div class="zp-conversation">
              <div class="zp-context">
                <button class="zp-context-name" onClick={() => openZapper(z())}>
                  <Avatar avatar={avatarOf(z())} image={z().config.avatarImage} size={24} mood={moodOf(z(), now())} />
                  <strong>{z().config.name}</strong>
                  <span class="zp-muted">
                    {view.selectedChatId === z().coordinatorChatId
                      ? "Coordinator"
                      : "Thread"}
                  </span>
                </button>
                <Button
                  size="sm"
                  onClick={() => { if (view.rightPanel) { setView(draft => { draft.rightPanel = null; }); setInspectorOpen(true); } else setInspectorOpen(!inspectorOpen()); }}
                >
                  {inspectorOpen() && !view.rightPanel ? "Hide Zapper" : "Show Zapper"}
                </Button>
              </div>
              <TopBar />
              <div class="app-chat"><Show when={z().workers.find(worker=>worker.chatId===view.selectedChatId && (worker.queuePosition !== undefined || !state.chats[worker.chatId]))} fallback={<><Transcript /><Composer /></>}>{worker => <div class="zp-thread-placeholder"><Icon name="clock-circle" size={28} /><h2>{workerStatus(worker())}</h2><p>{worker().objective}</p><span>{worker().queuePosition !== undefined ? "This thread will start automatically when a worker slot is available." : "Preparing this thread’s conversation…"}</span></div>}</Show></div>
            </div>
            <Show when={inspectorOpen() && !view.rightPanel}>
              <Inspector zapper={z()} />
            </Show>
          </div>
        )}
      </Show>
    </>
  );
}
function Directory() {
  const [archived, setArchived] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const visible = createMemo(() => state.zappers.filter(z => (archived() || z.status !== "archived") && `${z.config.name} ${z.config.purpose}`.toLowerCase().includes(query().toLowerCase())));
  return (
    <div class="zp-directory">
      <div class="zp-section-heading">
        <h1>Zappers</h1>
        <Button
          variant="primary"
          onClick={() => navigate({ kind: "newZapper" })}
        >
          New Zapper
        </Button>
      </div>
      <p class="zp-muted">
        Captains for your projects, with their own conversations, threads,
        issues and shared notes.
      </p>
      <div class="zp-directory-tools"><label class="zp-search"><Icon name="palette-search" size={14} /><input aria-label="Search Zappers" placeholder="Find a Zapper…" value={query()} onInput={e => setQuery(e.currentTarget.value)} /></label>
      <label class="zp-check">
        <input
          type="checkbox"
          checked={archived()}
          onChange={(e) => setArchived(e.currentTarget.checked)}
        />
        Show archived
      </label></div>
      <Show when={!state.zappersReady}>
        <p>Loading Zappers…</p>
      </Show>
      <Show when={state.zappersError}>
        <p role="alert">{state.zappersError}</p>
      </Show>
      <div class="zp-directory-grid">
        <For
          each={visible()}
        >
          {(z) => (
            <button
              class="zp-card zp-directory-card"
              onClick={() => openZapper(z)}
            >
              <Avatar avatar={avatarOf(z)} image={z.config.avatarImage} size={42} />
              <h2>{z.config.name}</h2>
              <p>{z.config.purpose || "Your project captain"}</p>
              <span class="zp-muted">
                {statusLabel(z.status)} · {z.workers.length} threads ·{" "}
                {z.issues.filter(issue => !["done","canceled"].includes(issue.details.status)).length} open issues
              </span>
            </button>
          )}
        </For>
      </div>
      <Show when={state.zappersReady && !state.zappersError && !visible().length}>
        <div class="zp-empty">
          <Icon name="tool-agent" size={28} /><strong>{state.zappers.length ? "No matching Zappers" : "A captain for your next project"}</strong><p>{state.zappers.length ? "Try another search or include archived Zappers." : "Create a Zapper to coordinate tasks, threads, and shared notes."}</p>
        </div>
      </Show>
    </div>
  );
}
function Inspector(props: { zapper: Zapper }) {
  const [confirm, setConfirm] = createSignal<"delete" | "archive" | null>(null);
  const tabs = (): Tab[] => ["overview", "issues", "threads", "drive"];
  const panel = () => panelFor(props.zapper.id);
  const label = (page: import("./panel").PanelPage) => page.kind === "issue" ? `ZAP-${props.zapper.issues.find(issue=>issue.id===page.id)?.number ?? "?"}` : page.kind === "file" ? page.path.split("/").at(-1)! : page.kind === "pr" ? `#${page.url.split("/").at(-1)}` : page.kind === "agent" ? page.title ?? "Agent" : page.kind === "diff" ? "Changes" : "History";
  const target = (chatId:string): Target => targetForChat(chatId) ?? {chatId,deviceId:props.zapper.config.homeDeviceId,targetDeviceId:props.zapper.config.homeDeviceId,cwd:null,archived:false,space:null,git:false,local:false,branch:null,repoPath:null};
  const openIssue = (id:string) => showPage(props.zapper.id,{kind:"issue",id});
  const fresh = async () => {
    try {
      const z = await command(props.zapper.id, { type: "freshConversation" });
      if (z) {
        await Promise.resolve();
        openZapper(z);
      }
    } catch {
      /* shared alert */
    }
  };
  const finish = async () => {
    const action: ZapperAction =
      confirm() === "delete"
        ? { type: "delete" }
        : { type: "setStatus", status: "archived" };
    try {
      await command(props.zapper.id, action);
      setConfirm(null);
      if (action.type === "delete") navigate({ kind: "zappers", zapperId: "" });
    } catch {
      /* shared alert */
    }
  };
  return (
    <aside class="zp-inspector" aria-label="Zapper inspector">
      <header class="zp-inspector-header">
        <Menu
          trigger={(attrs) => (
            <Button {...attrs} aria-label="Zapper actions">
              <Icon name="more-horizontal" size={14} />
            </Button>
          )}
          items={[
            {
              label: "Zapper settings",
              onSelect: () =>
                navigate({ kind: "newZapper", zapperId: props.zapper.id }),
            },
            { label: "Fresh conversation", onSelect: () => void fresh() },
            { label: "Version history", icon:"history", onSelect:()=>showPage(props.zapper.id,{kind:"history"}) },
            { label: "Drive", icon:"folder-open", onSelect:()=>setTab("drive") },
            {
              label: props.zapper.status === "active" ? "Pause" : "Resume",
              onSelect: () =>
                act(props.zapper.id, {
                  type: "setStatus",
                  status: props.zapper.status === "active" ? "paused" : "active",
                }),
            },
            {
              label: "Stop Zapper",
              onSelect: () =>
                act(props.zapper.id, { type: "setStatus", status: "stopped" }),
            },
            { label: "Archive Zapper", onSelect: () => setConfirm("archive") },
            {
              label: "Delete Zapper",
              danger: true,
              onSelect: () => setConfirm("delete"),
            },
          ]}
        />
        <Button size="sm" icon="close" aria-label="Close Zapper inspector" onClick={() => setInspectorOpen(false)} />
      </header>
      <nav class="zp-tabs" aria-label="Zapper sections">
        <For each={tabs()}>
          {(name) => (
            <button
              aria-label={name[0]!.toUpperCase() + name.slice(1)}
              title={name[0]!.toUpperCase() + name.slice(1)}
              aria-current={tab() === name ? "page" : undefined}
              onClick={() => setTab(name)}
            >
              <Icon
                name={
                  {
                    overview: "widget",
                    issues: "checklist",
                    threads: "chat-round-line",
                    drive: "folder-open",
                    history: "history",
                  }[name]
                }
                size={13}
              />
              <span class="zp-tab-label">
                <span>{name[0]!.toUpperCase() + name.slice(1)}</span>
              </span>
            </button>
          )}
        </For>
        <For each={panel().open}>{page => <div class="zp-page-tab" data-active={pageKey(panel().active)===pageKey(page)?"true":undefined}><button type="button" class="zp-page-tab-title" title={page.kind === "issue" ? props.zapper.issues.find(issue=>issue.id===page.id)?.details.title : page.kind === "file" ? page.path : label(page)} onClick={()=>showPage(props.zapper.id,page)}>{label(page)}</button><button type="button" class="zp-page-tab-close" aria-label={`Close ${label(page)}`} onClick={()=>closePage(props.zapper.id,page)}><Icon name="close" size={11} /></button></div>}</For>
      </nav>
      <div class="zp-inspector-body" data-workspace={["agent","diff","pr"].includes(panel().active.kind)?"true":undefined}>
        <Show when={tab() === "overview"}>
          <Overview zapper={props.zapper} onOpenIssue={openIssue} />
        </Show>
        <Show when={tab() === "issues"}>
          <Issues zapper={props.zapper} onOpen={openIssue} />
        </Show>
        <Show when={tab() === "threads"}>
          <Threads zapper={props.zapper} />
        </Show>
        <Show when={tab() === "drive"}><Drive zapper={props.zapper} history={false} onOpen={path=>showPage(props.zapper.id,{kind:"file",path})} /></Show>
        <For each={panel().open}>{page=><section class="zp-page-surface" hidden={pageKey(panel().active)!==pageKey(page)}>
          <Show when={page.kind === "issue"}><IssuePage zapper={props.zapper} id={page.kind === "issue" ? page.id : ""} onOpen={openIssue} onOpenPr={(url,chatId)=>showPage(props.zapper.id,{kind:"pr",url,chatId})} /></Show>
          <Show when={page.kind === "file"}><Drive zapper={props.zapper} history={false} path={page.kind === "file" ? page.path : ""} /></Show>
          <Show when={page.kind === "history"}><Drive zapper={props.zapper} history /></Show>
          <Show when={page.kind === "agent"}>{page.kind === "agent" && <Agents chatId={page.chatId} docId={page.docId} active={pageKey(panel().active)===pageKey(page)} />}</Show>
          <Show when={page.kind === "diff" && pageKey(panel().active)===pageKey(page)}>{page.kind === "diff" && <Changes target={target(page.chatId)} />}</Show>
          <Show when={page.kind === "pr" && pageKey(panel().active)===pageKey(page)}>{page.kind === "pr" && <PullRequest target={target(page.chatId)} url={page.url} active onBack={()=>closePage(props.zapper.id,page)} />}</Show>
        </section>}</For>
      </div>
      <FileDialogs zapper={props.zapper} />
      <Dialog
        open={!!confirm()}
        onOpenChange={(open) => {
          if (!open) setConfirm(null);
        }}
        title={confirm() === "delete" ? "Delete Zapper?" : "Archive Zapper?"}
        description={
          confirm() === "delete"
            ? "Its conversations and files remain available. The Zapper will no longer coordinate work."
            : "The Zapper and all its worker threads will be archived."
        }
        footer={
          <>
            <Button onClick={() => setConfirm(null)}>Cancel</Button>
            <Button
              variant="danger"
              disabled={commandState.busy}
              onClick={() => void finish()}
            >
              Confirm
            </Button>
          </>
        }
      />
    </aside>
  );
}
function Overview(props: { zapper: Zapper; onOpenIssue: (id:string)=>void }) {
  const [decision,setDecision]=createSignal<string|null>(null);
  const working=()=>props.zapper.workers.filter(worker=>worker.queuePosition === undefined && state.sessions[worker.chatId]?.status === "working").length;
  const asks=()=>props.zapper.issues.filter(issue=>issue.details.status === "needsYou" && (!issue.details.owner || issue.details.owner.type === "user"));
  const decisions=()=>props.zapper.decisions.filter(decision=>decision.answer===null);
  return <>
    <div class="zp-overview-hero"><Avatar avatar={avatarOf(props.zapper)} image={props.zapper.config.avatarImage} size={40} mood={moodOf(props.zapper, now())} /><div><h2>{props.zapper.config.name}</h2><span class="zp-overview-status" data-status={props.zapper.status}><i />{props.zapper.status === "active" && working() ? `${working()} ${working()===1?"thread":"threads"} working` : statusLabel(props.zapper.status)}</span></div><Show when={["active","paused"].includes(props.zapper.status)}><Button size="sm" onClick={()=>act(props.zapper.id,{type:"setStatus",status:props.zapper.status === "active" ? "paused":"active"})}>{props.zapper.status === "active" ? "Pause":"Resume"}</Button></Show></div>
    <Show when={props.zapper.config.purpose.trim()}><p class="zp-purpose">{props.zapper.config.purpose}</p></Show>
    <Show when={asks().length || decisions().length}><div class="zp-needs-you"><h3>Needs you</h3><For each={asks()}>{issue=><button type="button" onClick={()=>props.onOpenIssue(issue.id)}><Icon name="status-question" size={14} /><div><strong>{issue.details.title}</strong><span>{[...issue.activity].reverse().find(event=>event.kind === "question")?.text ?? `ZAP-${issue.number}`}</span></div><span>Answer</span></button>}</For><For each={decisions()}>{item=><button type="button" onClick={()=>setDecision(item.id)}><Icon name="info-circle" size={14} /><div><strong>{item.question}</strong></div><span>Answer</span></button>}</For></div></Show>
    <div class="zp-section-heading"><h3>Work</h3><Button size="sm" onClick={()=>setTab("issues")}>All issues</Button></div><Issues zapper={props.zapper} compact onOpen={props.onOpenIssue} />
    <h3>Repositories</h3><div class="zp-repository-chips"><For each={props.zapper.config.repositories}>{repo=><span><Icon name="folder" size={13} />{state.spaces[repo.spaceId]?.name ?? state.spaces[repo.spaceId]?.path ?? repo.spaceId}</span>}</For></div>
    <Show when={props.zapper.decisions.find(item=>item.id===decision() && item.answer===null)}>{item=><Dialog open onOpenChange={open=>{if(!open)setDecision(null);}} title="Your decision"><Decision zapper={props.zapper} decision={item()} onDone={()=>setDecision(null)} /></Dialog>}</Show>
  </>;
}
function Decision(props: { zapper: Zapper; decision: ZapperDecision; onDone?:()=>void }) {
  const [answer, setAnswer] = createSignal("");
  return (
    <form
      class="zp-card zp-form"
      onSubmit={async e => {
        e.preventDefault();
        if(!answer().trim()) return;
        try {await command(props.zapper.id,{type:"decide",decision:{...props.decision,answer:answer().trim()}});props.onDone?.();}catch{/* shared command alert */}
      }}
    >
      <p>{props.decision.question}</p>
      <Show when={props.decision.workerChatId}>
        <button
          type="button"
          onClick={() => openChat(props.decision.workerChatId)}
        >
          Open related thread
        </button>
      </Show>
      <label>
        Your answer
        <textarea
          required
          value={answer()}
          onInput={(e) => setAnswer(e.currentTarget.value)}
        />
      </label>
      <Button type="submit" disabled={commandState.busy}>
        Send decision
      </Button>
    </form>
  );
}
