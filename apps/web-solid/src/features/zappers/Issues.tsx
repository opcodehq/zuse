import { Icon } from "@/ui/Icon";
import { state } from "@/core/store";
import { IssuePriorityIcon, IssueStatusIcon, IssueWorking, issueStatuses } from "./IssueMetadata";
import { issueAgentWorking } from "./issue-activity";
import { IssueRelations } from "./IssueRelations";
import { deviceOnline } from "@/features/projects/hosts";
import { effectiveIndicator } from "@/features/extras/util";
import { ZapperSelect } from "./Controls";
import {
  createMemo,
  createEffect,
  createSignal,
  For,
  Show,
  snapshot,
  untrack,
  onCleanup,
} from "solid-js";
import { openChat } from "@/core/view";
import { uuid } from "@/core/util";
import type {
  Zapper,
  ZapperIssue,
  IssueDetails,
  IssueStatus,
  IssuePriority,
} from "@/core/zapper-types";
import { Button, Dialog } from "@/ui/primitives";
import { Markdown } from "@/features/transcript/Markdown";
import { WorkerDialog } from "./Threads";
import { act, command, commandState, threadTitle } from "./state";
const statuses = Object.fromEntries(Object.entries(issueStatuses).map(([key, value]) => [key, value.label])) as Record<IssueStatus, string>;
const priorities: IssuePriority[] = ["urgent", "high", "normal", "low"];
export function Issues(props: { zapper: Zapper; compact?: boolean; onOpen?: (id:string)=>void }) {
  const [activityAt, setActivityAt] = createSignal(Date.now());
  const activityTimer = setInterval(() => setActivityAt(Date.now()), 15_000);
  onCleanup(() => clearInterval(activityTimer));
  const [filter, setFilter] = createSignal("open");
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal<string | null>(null);
  const [editing, setEditing] = createSignal(false);
  const [creating, setCreating] = createSignal(false);
  createEffect(() => props.zapper.id, () => { setSelected(null); setEditing(false); setCreating(false); setQuery(""); });
  const issue = () => props.zapper.issues.find((i) => i.id === selected());
  const filtered = createMemo(() =>
    props.zapper.issues
      .filter(
        (i) =>
          (filter() === "all" ||
            (filter() === "open" &&
              !["done", "canceled"].includes(i.details.status)) ||
            filter() === i.details.status) &&
          `${i.number} ${i.details.title} ${i.details.description} ${i.details.labels.join(" ")}`
            .toLowerCase()
            .includes(query().toLowerCase()),
      )
      .sort(
        (a, b) =>
          priorities.indexOf(a.details.priority) -
          priorities.indexOf(b.details.priority),
      ),
  );
  return (
    <>
      <Show when={!props.compact}>
        <div class="zp-section-heading">
          <h3>Issues</h3>
          <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
            New issue
          </Button>
        </div>
        <div class="zp-filters">
          <label class="zp-search"><Icon name="palette-search" size={14} /><input
            aria-label="Search Zapper issues"
            placeholder="Search issues…"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          /></label>
          <ZapperSelect label="Issue status filter" value={filter()} onChange={setFilter} options={[{value:"open",label:"Open issues"},{value:"all",label:"All issues"},...Object.entries(statuses).map(([value,label]) => ({value,label,leading:<IssueStatusIcon status={value as IssueStatus} />}))]} />
        </div>
      </Show>
      <Show when={!props.compact}><div class="zp-list-summary">{filtered().length} {filtered().length === 1 ? "issue" : "issues"}<Show when={query() || filter() !== "open"}><button type="button" class="zp-link" onClick={() => { setQuery(""); setFilter("open"); }}>Clear filters</button></Show></div></Show>
      <div class="zp-issue-list"><For each={props.compact ? filtered().slice(0, 5) : filtered()} fallback={<div class="zp-list-empty"><Icon name="list" size={24} /><strong>{props.zapper.issues.length ? "No matching issues" : "A clear place for the next task"}</strong><p>{props.zapper.issues.length ? "Try another search or status." : "Track tasks, decisions, and progress with your Zapper."}</p><Show when={!props.compact && !props.zapper.issues.length}><Button size="sm" onClick={() => setCreating(true)}>Create an issue</Button></Show></div>}>
        {issue => <button type="button" class="zp-issue-row" onClick={() => { if(props.onOpen) props.onOpen(issue.id); else {setEditing(false);setSelected(issue.id);} }}>
          <span class="zp-issue-status" title={issueStatus(props.zapper,issue)}><IssueStatusIcon status={issueStatus(props.zapper, issue) === "Queued" ? "todo" : issue.details.status} /></span>
          <div class="zp-issue-content"><div class="zp-issue-title"><span class="zp-issue-number">ZAP-{issue.number}</span><strong>{issue.details.title}</strong></div><div class="zp-issue-meta"><span>{issueStatus(props.zapper,issue)}</span><span class="issue-metadata" data-priority={issue.details.priority}><IssuePriorityIcon priority={issue.details.priority} />{issue.details.priority}</span><Show when={issue.details.owner || issue.details.assigned}><span class="issue-metadata"><Show when={issue.details.owner?.type === "thread"}><IssueWorking working={issueAgentWorking(issue.details, props.zapper, state.connection.kind === "connected", id => effectiveIndicator(id, activityAt()), deviceOnline)} /></Show>{ownerLabel(props.zapper,issue)}</span></Show><Show when={issue.details.dueDate}><span>Due {issue.details.dueDate}</span></Show><For each={issue.details.labels}>{label => <span class="zp-label">{label}</span>}</For></div></div>
          <Icon name="alt-arrow-right" size={12} class="zp-issue-chevron" />
        </button>}
      </For></div>
      <Show when={creating()}>
        <IssueEditor zapper={props.zapper} close={() => setCreating(false)} onSave={props.onOpen} />
      </Show>
      <Show when={issue()}>
        {(value) => (
          <Dialog
            open
            onOpenChange={(open) => {
              if (!open) { setSelected(null); setEditing(false); }
            }}
            title={editing() ? "Edit issue" : `#${value().number} ${value().details.title}`}
          >
            <Show when={editing()} fallback={<IssueDetail zapper={props.zapper} issue={value()} edit={() => setEditing(true)} />}>
              <IssueEditor zapper={props.zapper} issue={value()} embedded close={() => setEditing(false)} />
            </Show>
          </Dialog>
        )}
      </Show>

    </>
  );
}
export function IssuePage(props: { zapper: Zapper; id: string; onOpen: (id:string)=>void; onOpenPr?: (url:string,chatId:string)=>void }) {
  const [editing,setEditing]=createSignal(false);
  createEffect(()=>props.id,()=>{ setEditing(false); });
  const issue=()=>props.zapper.issues.find(issue=>issue.id===props.id);
  return <Show when={issue()} fallback={<p class="zp-muted">This issue is no longer available.</p>}>{issue=><div class="zp-issue-page"><div class="zp-issue-page-title"><span>ZAP-{issue().number}</span><h2>{issue().details.title}</h2></div><Show when={editing()} fallback={<IssueDetail zapper={props.zapper} issue={issue()} edit={()=>setEditing(true)} onOpen={props.onOpen} onOpenPr={props.onOpenPr} />}><IssueEditor zapper={props.zapper} issue={issue()} embedded close={()=>setEditing(false)} /></Show></div>}</Show>;
}
function ownerValue(details: IssueDetails) { const owner=details.owner;return owner?.type === "thread" ? `thread:${owner.chatId}` : owner?.type ?? (details.assigned ? "zapper" : "none"); }
function ownerLabel(zapper: Zapper, issue: ZapperIssue) {
  const owner=issue.details.owner;
  return owner?.type === "user" ? "You" : owner?.type === "thread" ? threadTitle(owner.chatId,"Thread") : owner?.type === "zapper" || issue.details.assigned ? zapper.config.name : "Unassigned";
}
function issueStatus(zapper: Zapper, issue: ZapperIssue) {
  const owner=issue.details.owner;
  const worker=owner?.type === "thread" ? zapper.workers.find(worker=>worker.chatId===owner.chatId) : undefined;
  return worker?.queuePosition !== undefined ? "Queued" : statuses[issue.details.status];
}
function IssueEditor(props: {
  zapper: Zapper;
  issue?: ZapperIssue;
  onSave?: (id:string)=>void;
  embedded?: boolean;
  close: () => void;
}) {
  const id = untrack(() => props.issue?.id) ?? uuid();
  const [draft, setDraft] = createSignal<IssueDetails>(() =>
    untrack(() =>
      props.issue
        ? structuredClone(snapshot(props.issue.details))
        : ({
            title: "",
            description: "",
            status: "backlog",
            priority: "normal",
            assigned: false,
            dueDate: null,
            labels: [],
            relations: [],
          } as IssueDetails),
    ),
  );
  const change = <K extends keyof IssueDetails>(
    key: K,
    value: IssueDetails[K],
  ) => setDraft((d) => ({ ...d, [key]: value }));
  const form = () => (
      <form
        class="zp-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!draft().title.trim()) return;
          try {
            await command(props.zapper.id, {
              type: props.issue ? "editIssue" : "createIssue",
              issueId: id,
              details: { ...draft(), title: draft().title.trim() },
            });
            props.onSave?.(id);
            props.close();
          } catch {
            /* shared alert */
          }
        }}
      >
        <label>
          Title
          <input
            required
            value={draft().title}
            onInput={(e) => change("title", e.currentTarget.value)}
          />
        </label>
        <label>
          Description
          <textarea
            rows={6}
            value={draft().description}
            onInput={(e) => change("description", e.currentTarget.value)}
          />
        </label>
        <div class="zp-form-grid">
          <label>
            Status
            <ZapperSelect label="Issue status" value={draft().status} onChange={value => change("status", value as IssueStatus)} options={Object.entries(statuses).map(([value,label]) => ({value,label,leading:<IssueStatusIcon status={value as IssueStatus} />}))} />
          </label>
          <label>
            Priority
            <ZapperSelect label="Issue priority" value={draft().priority} onChange={value => change("priority", value as IssuePriority)} options={priorities.map(value => ({value,label:value[0]!.toUpperCase()+value.slice(1),leading:<IssuePriorityIcon priority={value} />}))} />
          </label>
          <label>
            Due date
            <input
              type="date"
              value={draft().dueDate ?? ""}
              onInput={(e) => change("dueDate", e.currentTarget.value || null)}
            />
          </label>
          <label>
            Labels
            <input
              value={draft().labels.join(", ")}
              onChange={(e) =>
                change(
                  "labels",
                  e.currentTarget.value
                    .split(",")
                    .map((s) => s.trim())
                    .filter(Boolean),
                )
              }
              placeholder="bug, release"
            />
          </label>
        </div>
        <label>Owner<ZapperSelect label="Issue owner" value={ownerValue(draft())} options={[{value:"none",label:"Unassigned"},{value:"user",label:"You"},{value:"zapper",label:props.zapper.config.name},...props.zapper.workers.map(worker=>({value:`thread:${worker.chatId}`,label:threadTitle(worker.chatId,worker.objective)}))]} onChange={value => setDraft(draft=>({...draft,owner:value === "none" ? undefined : value.startsWith("thread:") ? {type:"thread",chatId:value.slice(7)} : {type:value as "user"|"zapper"},assigned:value === "zapper" || value.startsWith("thread:")}))} /></label>
        <label>Parent issue<ZapperSelect label="Parent issue" value={draft().parent ?? ""} options={[{value:"",label:"No parent"},...props.zapper.issues.filter(issue=>issue.id!==id).map(issue=>({value:issue.id,label:`ZAP-${issue.number} · ${issue.details.title}`}))]} onChange={value=>change("parent",value || undefined)} /></label>
        <label>Done when<textarea rows={2} placeholder="What needs to be true to close this issue?" value={draft().doneCriteria ?? ""} onInput={e => change("doneCriteria",e.currentTarget.value)} /></label>
        <label>Out of scope<textarea rows={2} placeholder="What should this task leave alone?" value={draft().outOfScope ?? ""} onInput={e => change("outOfScope",e.currentTarget.value)} /></label>
        <Show when={commandState.error}>
          <p class="zp-error" role="alert">
            {commandState.error}
          </p>
        </Show>
        <div class="zp-form-actions">
          <Button onClick={props.close}>Cancel</Button>
          <Button type="submit" variant="primary" disabled={commandState.busy || !draft().title.trim()}>
            Save issue
          </Button>
        </div>
      </form>
  );
  return <Show when={!props.embedded} fallback={form()}><Dialog open onOpenChange={open => { if (!open) props.close(); }} title={props.issue ? "Edit issue" : "New issue"}>{form()}</Dialog></Show>;
}

function IssueDetail(props: {
  zapper: Zapper;
  issue: ZapperIssue;
  edit: () => void;
  onOpen?: (id:string)=>void;
  onOpenPr?: (url:string,chatId:string)=>void;
}) {
  const [comment, setComment] = createSignal("");
  const [reply, setReply] = createSignal<string | null>(null);
  const [link, setLink] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [delegating, setDelegating] = createSignal(false);
  const [action,setAction]=createSignal<"ask"|"close"|null>(null);
  const [note,setNote]=createSignal("");
  const [recommendation,setRecommendation]=createSignal("");
  const [outcome,setOutcome]=createSignal<"merged"|"dropped">("merged");
  return (
    <div class="zp-issue-detail">
      <div class="zp-section-heading">
        <span>
          <span class="issue-metadata"><IssueStatusIcon status={issueStatus(props.zapper, props.issue) === "Queued" ? "todo" : props.issue.details.status} />{issueStatus(props.zapper,props.issue)}</span> ·{" "}
          <span class="issue-metadata"><IssuePriorityIcon priority={props.issue.details.priority} />{props.issue.details.priority}</span>
          {` · ${ownerLabel(props.zapper,props.issue)}`}
        </span>
        <Button size="sm" onClick={props.edit}>
          Edit issue
        </Button>
      </div>
      <Markdown text={props.issue.details.description} live={false} />
      <Show when={props.issue.details.doneCriteria}><h3>Done when</h3><Markdown text={props.issue.details.doneCriteria!} live={false} /></Show>
      <Show when={props.issue.details.outOfScope}><h3>Out of scope</h3><Markdown text={props.issue.details.outOfScope!} live={false} /></Show>
      <Show when={props.issue.details.dueDate}>
        <p>Due {props.issue.details.dueDate}</p>
      </Show>
      <Show when={props.issue.details.labels.length}>
        <p>{props.issue.details.labels.join(" · ")}</p>
      </Show>
      <IssueRelations zapper={props.zapper} issue={props.issue} onOpen={props.onOpen} />
      <Show when={props.issue.details.parent}><button type="button" class="zp-link" onClick={()=>props.onOpen?.(props.issue.details.parent!)}>Parent: {props.zapper.issues.find(issue=>issue.id===props.issue.details.parent)?.details.title ?? props.issue.details.parent}</button></Show>
      <Show when={!["done","canceled"].includes(props.issue.details.status)}><div class="zp-issue-actions"><Button size="sm" onClick={()=>{setNote("");setRecommendation("");setAction("ask");}}>Ask you</Button><Button size="sm" onClick={()=>{setNote("");setAction("close");}}>Close issue</Button></div></Show>
      <h3>Threads</h3>
      <For each={props.issue.threads}>
        {(id) => (
          <div class="zp-section-heading">
            <button class="zp-link" onClick={() => openChat(id)}>
              {threadTitle(id)}
            </button>
            <Button
              size="sm"
              onClick={() =>
                act(props.zapper.id, {
                  type: "unlinkIssueThread",
                  issueId: props.issue.id,
                  chatId: id,
                })
              }
            >
              Unlink
            </Button>
          </div>
        )}
      </For>
      <div class="zp-filters">
        <ZapperSelect label="Link issue thread" value={link()} onChange={setLink} options={[{value:"",label:"Choose a worker thread"},...props.zapper.workers.filter(worker => !props.issue.threads.includes(worker.chatId)).map(worker => ({value:worker.chatId,label:threadTitle(worker.chatId,worker.objective),icon:"chat-round-line"}))]} />
        <Button
          disabled={!link() || commandState.busy}
          onClick={() =>
            act(props.zapper.id, {
              type: "linkIssueThread",
              issueId: props.issue.id,
              chatId: link(),
            })
          }
        >
          Link
        </Button>
        <Button
          disabled={!props.issue.details.assigned}
          onClick={() => setDelegating(true)}
        >
          Start task
        </Button>
      </div>
      <h3>Pull requests</h3>
      <For each={props.issue.pullRequests}>
        {(pr) => (
          <div class="zp-section-heading">
            <a
              href={/^https?:\/\//.test(pr.url) ? pr.url : undefined}
              target={props.onOpenPr ? undefined : "_blank"}
              rel="noreferrer"
              onClick={event=>{if(props.onOpenPr && /^https?:\/\//.test(pr.url)){event.preventDefault();props.onOpenPr(pr.url,props.issue.threads[0] ?? props.zapper.coordinatorChatId);}}}
            >
              {pr.title || pr.url}
            </a>
            <Button
              size="sm"
              onClick={() =>
                act(props.zapper.id, {
                  type: "unlinkIssuePullRequest",
                  issueId: props.issue.id,
                  url: pr.url,
                })
              }
            >
              Unlink
            </Button>
          </div>
        )}
      </For>
      <form
        class="zp-filters"
        onSubmit={(e) => {
          e.preventDefault();
          if (/^https?:\/\//.test(url()))
            act(props.zapper.id, {
              type: "linkIssuePullRequest",
              issueId: props.issue.id,
              url: url(),
              title: "",
            });
        }}
      >
        <input
          type="url"
          required
          aria-label="Pull request URL"
          placeholder="Pull request URL"
          value={url()}
          onInput={(e) => setUrl(e.currentTarget.value)}
        />
        <Button type="submit" disabled={commandState.busy}>
          Link PR
        </Button>
      </form>
      <h3>Activity</h3>
      <For each={props.issue.activity}>
        {(entry) => (
          <div
            class="zp-activity"
            data-reply={entry.replyTo ? "true" : undefined}
          >
            <small class="zp-muted">
              {entry.actor || "Zapper"} · {entry.kind}
              {entry.replyTo ? " · Reply" : ""}
            </small>
            <Markdown text={entry.text} live={false} />
            <button class="zp-link" onClick={() => setReply(entry.id)}>
              Reply
            </button>
          </div>
        )}
      </For>
      <form
        class="zp-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!comment().trim()) return;
          try {
            await command(props.zapper.id, {
              type: "commentIssue",
              issueId: props.issue.id,
              text: comment().trim(),
              replyTo: reply(),
            });
            setComment("");
            setReply(null);
          } catch {
            /* shared alert */
          }
        }}
      >
        <Show when={reply()}>
          <span>
            Replying to a comment{" "}
            <button type="button" onClick={() => setReply(null)}>
              Cancel reply
            </button>
          </span>
        </Show>
        <label>
          Comment
          <textarea
            required
            value={comment()}
            onInput={(e) => setComment(e.currentTarget.value)}
          />
        </label>
        <Show when={commandState.error}>
          <p role="alert" class="zp-error">
            {commandState.error}
          </p>
        </Show>
        <Button type="submit" disabled={commandState.busy || !comment().trim()}>
          Post comment
        </Button>
      </form>
      <Dialog open={!!action()} onOpenChange={open=>{if(!open)setAction(null);}} title={action() === "ask" ? "Ask you" : "Close issue"}>
        <form class="zp-form" onSubmit={async event=>{event.preventDefault();if(action() === "ask" && !note().trim())return;try {await command(props.zapper.id,action() === "ask" ? {type:"askUser",issueId:props.issue.id,question:note().trim(),recommendation:recommendation().trim() || undefined} : {type:"closeIssue",issueId:props.issue.id,outcome:outcome(),note:note().trim()});setAction(null);}catch{/* shared command alert */}}}>
          <Show when={action() === "close"}><label>Outcome<ZapperSelect label="Close outcome" value={outcome()} onChange={value=>setOutcome(value as "merged"|"dropped")} options={[{value:"merged",label:"Completed / merged"},{value:"dropped",label:"Dropped"}]} /></label></Show>
          <label>{action() === "ask" ? "Question" : "Closing note"}<textarea required={action() === "ask"} value={note()} onInput={event=>setNote(event.currentTarget.value)} /></label>
          <Show when={action() === "ask"}><label>Recommendation<textarea value={recommendation()} onInput={event=>setRecommendation(event.currentTarget.value)} /></label></Show>
          <div class="zp-form-actions"><Button onClick={()=>setAction(null)}>Cancel</Button><Button type="submit" variant="primary" disabled={commandState.busy || (action() === "ask" && !note().trim())}>{action() === "ask" ? "Send question" : "Close issue"}</Button></div>
        </form>
      </Dialog>
      <Show when={delegating()}>
        <WorkerDialog
          zapper={props.zapper}
          mode="delegate"
          issueId={props.issue.id}
          close={() => setDelegating(false)}
        />
      </Show>
    </div>
  );
}
