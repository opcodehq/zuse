import { Icon } from "@/ui/Icon";
import { ZapperSelect } from "./Controls";
import {
  createMemo,
  createEffect,
  createSignal,
  For,
  Show,
  snapshot,
  untrack,
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
const statuses: Record<IssueStatus, string> = {
  backlog: "Backlog",
  todo: "Todo",
  inProgress: "In progress",
  needsYou: "Needs you",
  blocked: "Blocked",
  inReview: "In review",
  done: "Done",
  canceled: "Canceled",
};
const priorities: IssuePriority[] = ["urgent", "high", "normal", "low"];
export function Issues(props: { zapper: Zapper; compact?: boolean }) {
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
          <ZapperSelect label="Issue status filter" value={filter()} onChange={setFilter} options={[{value:"open",label:"Open issues"},{value:"all",label:"All issues"},...Object.entries(statuses).map(([value,label]) => ({value,label}))]} />
        </div>
      </Show>
      <Show when={!props.compact}><div class="zp-list-summary">{filtered().length} {filtered().length === 1 ? "issue" : "issues"}<Show when={query() || filter() !== "open"}><button type="button" class="zp-link" onClick={() => { setQuery(""); setFilter("open"); }}>Clear filters</button></Show></div></Show>
      <div class="zp-issue-list"><For each={props.compact ? filtered().slice(0, 5) : filtered()} fallback={<div class="zp-list-empty"><Icon name="list" size={24} /><strong>{props.zapper.issues.length ? "No matching issues" : "A clear place for the next task"}</strong><p>{props.zapper.issues.length ? "Try another search or status." : "Track tasks, decisions, and progress with your Zapper."}</p><Show when={!props.compact && !props.zapper.issues.length}><Button size="sm" onClick={() => setCreating(true)}>Create an issue</Button></Show></div>}>
        {issue => <button type="button" class="zp-issue-row" onClick={() => { setEditing(false); setSelected(issue.id); }}>
          <span class="zp-issue-status" data-status={issue.details.status} title={statuses[issue.details.status]}><Icon name={issue.details.status === "done" ? "check-circle" : issue.details.status === "blocked" ? "danger-triangle" : "clock-circle"} size={14} /></span>
          <div class="zp-issue-content"><div class="zp-issue-title"><span class="zp-issue-number">#{issue.number}</span><strong>{issue.details.title}</strong></div><div class="zp-issue-meta"><span>{statuses[issue.details.status]}</span><span data-priority={issue.details.priority}>{issue.details.priority}</span><Show when={issue.details.assigned}><span>Assigned</span></Show><Show when={issue.details.dueDate}><span>Due {issue.details.dueDate}</span></Show><For each={issue.details.labels}>{label => <span class="zp-label">{label}</span>}</For></div></div>
          <Icon name="alt-arrow-right" size={12} class="zp-issue-chevron" />
        </button>}
      </For></div>
      <Show when={creating()}>
        <IssueEditor zapper={props.zapper} close={() => setCreating(false)} />
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
function IssueEditor(props: {
  zapper: Zapper;
  issue?: ZapperIssue;
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
            <ZapperSelect label="Issue status" value={draft().status} onChange={value => change("status", value as IssueStatus)} options={Object.entries(statuses).map(([value,label]) => ({value,label}))} />
          </label>
          <label>
            Priority
            <ZapperSelect label="Issue priority" value={draft().priority} onChange={value => change("priority", value as IssuePriority)} options={priorities.map(value => ({value,label:value[0]!.toUpperCase()+value.slice(1)}))} />
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
        <label class="zp-check">
          <input
            type="checkbox"
            checked={draft().owner?.type === "zapper" || (draft().assigned && !draft().owner)}
            onChange={(e) => { const assigned = e.currentTarget.checked; setDraft(d => ({ ...d, assigned, owner: assigned ? { type: "zapper" } : undefined })); }}
          />
          Assign to this Zapper
        </label>
        <label>Done when<textarea rows={2} placeholder="What needs to be true to close this issue?" value={draft().doneCriteria ?? ""} onInput={e => change("doneCriteria",e.currentTarget.value)} /></label>
        <label>Out of scope<textarea rows={2} placeholder="What should this task leave alone?" value={draft().outOfScope ?? ""} onInput={e => change("outOfScope",e.currentTarget.value)} /></label>
        <fieldset>
          <legend>Related issues</legend>
          <For each={props.zapper.issues.filter((i) => i.id !== id)}>
            {(i) => (
              <label class="zp-check">
                <input
                  type="checkbox"
                  checked={draft().relations.includes(i.id)}
                  onChange={(e) =>
                    change(
                      "relations",
                      e.currentTarget.checked
                        ? [...draft().relations, i.id]
                        : draft().relations.filter((r) => r !== i.id),
                    )
                  }
                />
                #{i.number} {i.details.title}
              </label>
            )}
          </For>
        </fieldset>
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
}) {
  const [comment, setComment] = createSignal("");
  const [reply, setReply] = createSignal<string | null>(null);
  const [link, setLink] = createSignal("");
  const [url, setUrl] = createSignal("");
  const [delegating, setDelegating] = createSignal(false);
  return (
    <div class="zp-issue-detail">
      <div class="zp-section-heading">
        <span>
          {statuses[props.issue.details.status]} ·{" "}
          {props.issue.details.priority}
          {props.issue.details.assigned ? " · Assigned" : ""}
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
      <For each={props.issue.details.relations}>
        {(id) => (
          <p class="zp-muted">
            Related:{" "}
            {props.zapper.issues.find((i) => i.id === id)?.details.title ?? id}
          </p>
        )}
      </For>
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
              target="_blank"
              rel="noreferrer"
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
