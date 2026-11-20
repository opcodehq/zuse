import {
  createMemo,
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
export function Issues(props: { zot: Zapper; compact?: boolean }) {
  const [filter, setFilter] = createSignal("open");
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal<string | null>(null);
  const [editing, setEditing] = createSignal(false);
  const [creating, setCreating] = createSignal(false);
  const issue = () => props.zot.issues.find((i) => i.id === selected());
  const filtered = createMemo(() =>
    props.zot.issues
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
          <input
            aria-label="Search Zapper issues"
            placeholder="Search issues…"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
          <select
            aria-label="Issue status filter"
            value={filter()}
            onChange={(e) => setFilter(e.currentTarget.value)}
          >
            <option value="open">Open issues</option>
            <option value="all">All issues</option>
            <For each={Object.entries(statuses)}>
              {([value, label]) => <option value={value}>{label}</option>}
            </For>
          </select>
        </div>
      </Show>
      <For
        each={props.compact ? filtered().slice(0, 5) : filtered()}
        fallback={<p class="zp-muted">No matching issues.</p>}
      >
        {(i) => (
          <button class="zp-issue-row" onClick={() => setSelected(i.id)}>
            <span class="zp-muted">#{i.number}</span>
            <div>
              <strong>{i.details.title}</strong>
              <small>
                {statuses[i.details.status]} · {i.details.priority}
                {i.details.assigned ? " · Assigned" : ""}
                {i.details.dueDate ? ` · Due ${i.details.dueDate}` : ""}
              </small>
              <Show when={i.details.labels.length}>
                <small>{i.details.labels.join(" · ")}</small>
              </Show>
            </div>
          </button>
        )}
      </For>
      <Show when={creating()}>
        <IssueEditor zot={props.zot} close={() => setCreating(false)} />
      </Show>
      <Show when={issue()}>
        {(value) => (
          <Dialog
            open
            onOpenChange={(open) => {
              if (!open) setSelected(null);
            }}
            title={`#${value().number} ${value().details.title}`}
          >
            <IssueDetail
              zot={props.zot}
              issue={value()}
              edit={() => setEditing(true)}
            />
          </Dialog>
        )}
      </Show>
      <Show when={editing() && issue()}>
        {(value) => (
          <IssueEditor
            zot={props.zot}
            issue={value()}
            close={() => setEditing(false)}
          />
        )}
      </Show>
    </>
  );
}
function IssueEditor(props: {
  zot: Zapper;
  issue?: ZapperIssue;
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
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) props.close();
      }}
      title={props.issue ? "Edit issue" : "New issue"}
    >
      <form
        class="zp-form"
        onSubmit={async (e) => {
          e.preventDefault();
          if (!draft().title.trim()) return;
          try {
            await command(props.zot.id, {
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
            <select
              value={draft().status}
              onChange={(e) =>
                change("status", e.currentTarget.value as IssueStatus)
              }
            >
              <For each={Object.entries(statuses)}>
                {([value, label]) => <option value={value}>{label}</option>}
              </For>
            </select>
          </label>
          <label>
            Priority
            <select
              value={draft().priority}
              onChange={(e) =>
                change("priority", e.currentTarget.value as IssuePriority)
              }
            >
              <For each={priorities}>
                {(p) => (
                  <option value={p}>{p[0]!.toUpperCase() + p.slice(1)}</option>
                )}
              </For>
            </select>
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
            checked={draft().assigned}
            onChange={(e) => { const assigned = e.currentTarget.checked; setDraft(d => ({ ...d, assigned, owner: assigned ? { type: "zapper" } : undefined })); }}
          />
          Assign to this Zapper
        </label>
        <fieldset>
          <legend>Related issues</legend>
          <For each={props.zot.issues.filter((i) => i.id !== id)}>
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
          <Button type="submit" variant="primary" disabled={commandState.busy}>
            Save issue
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
function IssueDetail(props: {
  zot: Zapper;
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
            {props.zot.issues.find((i) => i.id === id)?.details.title ?? id}
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
                act(props.zot.id, {
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
        <select
          aria-label="Link issue thread"
          value={link()}
          onChange={(e) => setLink(e.currentTarget.value)}
        >
          <option value="">Choose a worker thread</option>
          <For
            each={props.zot.workers.filter(
              (w) => !props.issue.threads.includes(w.chatId),
            )}
          >
            {(w) => (
              <option value={w.chatId}>
                {threadTitle(w.chatId, w.objective)}
              </option>
            )}
          </For>
        </select>
        <Button
          disabled={!link() || commandState.busy}
          onClick={() =>
            act(props.zot.id, {
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
                act(props.zot.id, {
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
            act(props.zot.id, {
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
            await command(props.zot.id, {
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
        <Button type="submit" disabled={commandState.busy}>
          Post comment
        </Button>
      </form>
      <Show when={delegating()}>
        <WorkerDialog
          zot={props.zot}
          mode="delegate"
          issueId={props.issue.id}
          close={() => setDelegating(false)}
        />
      </Show>
    </div>
  );
}
