// Pull requests for the chat's repository: the branch's own pull request when it has one, the
// repository's list otherwise, and a detail view with checks, discussion, files, diff and status
// actions. Reads follow crates/ui/src/pull_request_detail.rs; updates are explicit and never resent.
import { batch, createEffect, createMemo, createSignal, For, Match, on, onCleanup, Show, Switch } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { TerminalSpinner } from "@/ui/loaders";
import { providerMarkdownHtml } from "@/ui/markdown";
import { Button, IconButton, Menu, Spinner, Tabs, type MenuItem } from "@/ui/primitives";
import { parsePatch } from "./diff";
import { DiffView } from "./DiffView";
import { Banner, DiffStat, Empty, PrStateIcon } from "./parts";
import { errorText, liveSubscribe, readErrorText, workspace, type Target } from "./store";
import type { ChangeRequestCheck, ChangeRequestComment, ChangeRequestDetail, ChangeRequestListItem, ChangeRequestPage, ChangeRequestUpdate, SourceControlFrame } from "./types";
import { copyText, plural, timeAgo } from "./util";

type Filter = "authored" | "all" | "reviewing";
const FILTERS: { value: Filter; label: string }[] = [
  { value: "authored", label: "Mine" },
  { value: "reviewing", label: "Reviewing" },
  { value: "all", label: "All" },
];
type Method = "squash" | "merge" | "rebase";
const METHODS: { value: Method; label: string }[] = [
  { value: "squash", label: "Squash and merge" },
  { value: "merge", label: "Create a merge commit" },
  { value: "rebase", label: "Rebase and merge" },
];

const openExternal = (url: string) => window.open(url, "_blank", "noopener,noreferrer");

export function PullRequest(props: { target: Target; active: boolean }) {
  const own = () => workspace.changeRequests[props.target.chatId]?.changeRequest ?? null;
  const [picked, setPicked] = createSignal<string | null>(null);
  // The branch's pull request shows by default; the list is one step back.
  const [browsing, setBrowsing] = createSignal(false);
  const url = () => picked() ?? (browsing() ? null : own()?.url ?? null);

  return (
    <div class="ws-surface-col ws-pr">
      <Show
        when={url()}
        keyed
        fallback={
          <PullRequestList
            target={props.target}
            active={props.active}
            ownUrl={own()?.url ?? null}
            onOpen={(next) => {
              setPicked(next === own()?.url ? null : next);
              setBrowsing(false);
            }}
          />
        }
      >
        {(current) => (
          <PullRequestDetail
            target={props.target}
            url={current}
            own={current === own()?.url}
            onBack={() => {
              setPicked(null);
              setBrowsing(true);
            }}
          />
        )}
      </Show>
    </div>
  );
}

function PullRequestList(props: { target: Target; active: boolean; ownUrl: string | null; onOpen: (url: string) => void }) {
  const [repository, setRepository] = createSignal<string | null>(null);
  const [filter, setFilter] = createSignal<Filter>("authored");
  const [items, setItems] = createSignal<ChangeRequestListItem[]>([]);
  const [next, setNext] = createSignal<string | null>(null);
  const [total, setTotal] = createSignal<number | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [loaded, setLoaded] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let seq = 0;
  const connected = () => state.connection.kind === "connected";
  const cwd = () => (props.target.archived ? props.target.space?.path ?? props.target.cwd : props.target.cwd);

  const page = (append: boolean) => {
    const repo = repository();
    if (!repo) return;
    const id = ++seq;
    setLoading(true);
    rpc
      .call<ChangeRequestPage>(M.LIST_CHANGE_REQUEST_PAGE, {
        repository: repo,
        filter: filter(),
        after: append ? next() : null,
        refresh: true,
        targetDeviceId: props.target.deviceId,
      })
      .then(
        (result) => {
          if (id !== seq) return;
          batch(() => {
            const seen = new Set(append ? items().map((i) => i.url) : []);
            setItems([...(append ? items() : []), ...result.items.filter((i) => !seen.has(i.url))]);
            setNext(result.nextCursor ?? null);
            setTotal(result.totalCount ?? null);
            setError(null);
            setLoaded(true);
          });
        },
        (e) => id === seq && setError(readErrorText(e, "Couldn't list pull requests.", true)),
      )
      .finally(() => id === seq && setLoading(false));
  };

  const refresh = () => {
    const path = cwd();
    if (!connected()) return setError("Reconnect to read pull requests.");
    if (!path) return setError("Select a project to browse its pull requests.");
    if (repository()) return page(false);
    const id = ++seq;
    setLoading(true);
    rpc.call<string | null>(M.GET_CHANGE_REQUEST_REPOSITORY, { cwd: path, targetDeviceId: props.target.deviceId }).then(
      (repo) => {
        if (id !== seq) return;
        setLoading(false);
        if (!repo) {
          setLoaded(true);
          setError(null);
          return;
        }
        setRepository(repo);
        page(false);
      },
      (e) => {
        if (id !== seq) return;
        setLoading(false);
        setError(readErrorText(e, "Couldn't find this project's repository.", true));
      },
    );
  };

  createEffect(on([() => props.active, connected, () => state.epoch], ([active, live]) => active && live && refresh()));
  createEffect(on(filter, () => page(false), { defer: true }));

  return (
    <>
      <div class="ws-panel-header">
        <Icon name="pull-request" size={14} class="ws-muted" />
        <h2 class="ws-panel-title ws-truncate" title={repository() ?? undefined}>{repository() ?? "Pull requests"}</h2>
        <Show when={total() !== null}>
          <span class="ws-faint ws-small">{total()}</span>
        </Show>
        <div class="ws-grow" />
        <Show when={loading()} fallback={<IconButton icon="refresh" label="Refresh pull requests" size="sm" iconSize={14} onClick={refresh} />}>
          <span class="ws-icon-slot"><Spinner label="Loading pull requests" /></span>
        </Show>
      </div>
      <Show when={repository()}>
        <div class="ws-subbar">
          <Tabs label="Filter pull requests" value={filter()} items={FILTERS} onChange={setFilter} />
        </div>
      </Show>
      <Show when={error()}>
        <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={refresh}>Retry</Button>}>{error()}</Banner>
      </Show>
      <Show when={loaded() && !repository() && !error()}>
        <Empty icon="pull-request" title="No GitHub repository" detail="This project has no GitHub remote, so it has no pull requests to show." />
      </Show>
      <Show when={repository()}>
        <div class="ws-pr-list" role="list" aria-label="Pull requests">
          <For
            each={items()}
            fallback={
              <Show when={loaded() && !loading()}>
                <Empty icon="pull-request" title="No pull requests" detail={filter() === "authored" ? "You have no open pull requests here." : filter() === "reviewing" ? "Nothing is waiting for your review." : "This repository has no open pull requests."} />
              </Show>
            }
          >
            {(item) => (
              <div role="listitem">
                <button type="button" class="ws-pr-row" onClick={() => props.onOpen(item.url)}>
                  <PrStateIcon state={item.state} draft={item.isDraft} />
                  <div class="ws-pr-row-main">
                    <div class="ws-pr-row-title ws-truncate">
                      {item.title}
                      <Show when={item.url === props.ownUrl}>
                        <span class="ws-chip">This branch</span>
                      </Show>
                    </div>
                    <div class="ws-faint ws-small ws-truncate">
                      #{item.number} · {item.author.login} · updated {timeAgo(item.updatedAt)}
                      <Show when={item.ci && item.ci.state !== "unknown" && item.ci.state !== "noChecks"}> · <CiLabel state={item.ci!.state} /></Show>
                    </div>
                  </div>
                  <DiffStat additions={item.additions} deletions={item.deletions} />
                </button>
              </div>
            )}
          </For>
          <Show when={next()}>
            <div class="ws-pr-more">
              <Button size="sm" variant="ghost" disabled={loading()} onClick={() => page(true)}>{loading() ? "Loading…" : "Load more"}</Button>
            </div>
          </Show>
        </div>
      </Show>
    </>
  );
}

function CiLabel(props: { state: string }) {
  const text = () =>
    ({ pending: "CI pending", passed: "CI passed", skipped: "CI skipped", failed: "CI failing", noChecks: "No checks reported" })[props.state] ?? "CI unavailable";
  return <span data-tone={props.state === "failed" ? "danger" : props.state === "passed" ? "success" : undefined} class="ws-ci">{text()}</span>;
}

type DetailTab = "overview" | "activity" | "files";

/** Comment drafts survive switching between pull requests in this tab. */
const drafts = new Map<string, string>();
/** Pull requests whose last comment post lost its receipt. */
const uncertain = new Set<string>();

function PullRequestDetail(props: { target: Target; url: string; own: boolean; onBack: () => void }) {
  const [detail, setDetail] = createSignal<ChangeRequestDetail | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  // Keyed by URL in the parent, so these seeds are read once per pull request.
  // eslint-disable-next-line solid/reactivity
  const [notice, setNotice] = createSignal<string | null>(uncertain.has(props.url) ? "Comment delivery was uncertain. Check the discussion before posting again." : null);
  const [loading, setLoading] = createSignal(false);
  const [tab, setTab] = createSignal<DetailTab>("overview");
  const [updating, setUpdating] = createSignal<string | null>(null);
  const [patch, setPatch] = createSignal<string | null>(null);
  const [patchError, setPatchError] = createSignal<string | null>(null);
  // eslint-disable-next-line solid/reactivity
  const [comment, setComment] = createSignal(drafts.get(props.url) ?? "");
  const [posting, setPosting] = createSignal(false);
  const connected = () => state.connection.kind === "connected";
  let seq = 0;

  const host = (params: Record<string, unknown>) => ({ ...params, targetDeviceId: props.target.deviceId });

  const read = (refresh = true) => {
    if (!connected()) return setError("Reconnect to read this pull request.");
    const id = ++seq;
    setLoading(true);
    rpc
      .call<ChangeRequestDetail>(M.GET_CHANGE_REQUEST, host({ url: props.url, refresh }))
      .then(
        (value) => {
          if (id !== seq) return;
          setDetail(value);
          setError(null);
        },
        (e) => id === seq && setError(readErrorText(e, "Couldn't read this pull request.", true)),
      )
      .finally(() => id === seq && setLoading(false));
  };

  // A live watch keeps the detail current; a first read seeds it quickly.
  createEffect(
    on([connected, () => state.epoch], ([live]) => {
      if (!live) return;
      read(false);
      // Watch frames carry the summary, which has no inline review threads (desktop's
      // pull_request_detail.rs): the first frame only seeds an empty view, and threads carry over.
      let seen: string | null = null;
      let seeded = false;
      const sub = liveSubscribe<SourceControlFrame>(
        M.WATCH_CHANGE_REQUEST,
        host({ url: props.url }),
        (frame) => {
          if (frame.reset) batch(() => (setDetail(null), setPatch(null)));
          if (frame.error) setError(frame.error);
          const snapshot = frame.snapshot as unknown as ChangeRequestDetail | null | undefined;
          if (frame.refreshing || !snapshot || typeof snapshot !== "object" || !("url" in snapshot)) return;
          const key = JSON.stringify(snapshot);
          if (key === seen) return void setError(null);
          if (!seeded) {
            seeded = true;
            if (detail()) return;
          }
          seen = key;
          const old = detail();
          setDetail(old && old.headRefOid === snapshot.headRefOid ? { ...snapshot, reviewThreads: old.reviewThreads } : snapshot);
          setError(null);
        },
      );
      onCleanup(() => sub.cancel());
    }),
  );

  const loadPatch = () => {
    const d = detail();
    if (!d) return;
    setPatchError(null);
    rpc.call<string | { patch?: string }>(M.GET_CHANGE_REQUEST_DIFF, host({ url: d.url, headRefOid: d.headRefOid, baseRefOid: d.baseRefOid })).then(
      (value) => {
        if (detail()?.headRefOid !== d.headRefOid) return;
        setPatch(typeof value === "string" ? value : value?.patch ?? "");
      },
      (e) => setPatchError(readErrorText(e, "Couldn't read the diff.")),
    );
  };
  createEffect(on([tab, () => detail()?.headRefOid], ([t, head]) => t === "files" && head && patch() === null && loadPatch()));
  createEffect(on(() => detail()?.headRefOid, () => setPatch(null), { defer: true }));

  const update = (change: ChangeRequestUpdate, label: string) => {
    const d = detail();
    if (!d || updating()) return;
    const merging = change.kind === "merge" || change.kind === "enableAutoMerge";
    setUpdating(label);
    setNotice(null);
    rpc
      .call<{ refusal?: string | null }>(merging ? M.UPDATE_CHANGE_REQUEST_AT_HEAD : M.UPDATE_CHANGE_REQUEST, host({ url: d.url, update: change, headRefOid: d.headRefOid }))
      .then(
        (result) => {
          if (result?.refusal) setNotice(result.refusal);
          read();
        },
        (e) => {
          if (e instanceof RpcError && e.outcomeUnknown) setNotice("The connection dropped. Refresh the pull request before trying again; the change may have been made.");
          else setNotice(errorText(e, "GitHub refused the change."));
        },
      )
      .finally(() => setUpdating(null));
  };

  const post = () => {
    const body = comment();
    if (!body.trim() || posting()) return;
    if (new TextEncoder().encode(body).length > 60000) return setNotice("Enter a comment of at most 60,000 bytes.");
    setPosting(true);
    setNotice(null);
    rpc
      .call(M.POST_CHANGE_REQUEST_COMMENT, host({ url: props.url, body }))
      .then(
        () => {
          uncertain.delete(props.url);
          drafts.delete(props.url);
          setComment("");
          read();
        },
        (e) => {
          if (e instanceof RpcError && e.outcomeUnknown) {
            uncertain.add(props.url);
            setNotice("Comment delivery is uncertain. Refresh the discussion before posting again.");
          } else setNotice(errorText(e, "Couldn't post the comment."));
        },
      )
      .finally(() => setPosting(false));
  };

  createEffect(() => {
    const text = comment();
    if (text) drafts.set(props.url, text);
    else drafts.delete(props.url);
  });

  const open = () => detail()?.state.toUpperCase() === "OPEN";
  const merged = () => detail()?.state.toUpperCase() === "MERGED";
  const activity = createMemo(() => {
    const d = detail();
    if (!d) return [];
    const all = [...d.comments, ...d.reviews, ...(d.reviewThreads ?? []).flatMap((t) => t.comments)].filter((c) => c.body.trim() || c.state);
    const seen = new Set<string>();
    return all
      .filter((c) => (c.id ? !seen.has(c.id) && (seen.add(c.id), true) : true))
      .sort((a, b) => Date.parse(a.submittedAt || a.createdAt) - Date.parse(b.submittedAt || b.createdAt));
  });
  const checks = () => detail()?.statusCheckRollup ?? [];

  const statusItems = (): MenuItem[] => {
    const d = detail();
    if (!d) return [];
    const items: MenuItem[] = [];
    if (open()) {
      items.push(d.isDraft ? { label: "Mark ready for review", icon: "eye", onSelect: () => update({ kind: "markReady" }, "Marking ready") } : { label: "Convert to draft", icon: "git-state-draft", onSelect: () => update({ kind: "markDraft" }, "Converting to draft") });
      if (d.autoMergeRequest) items.push({ label: "Disable auto-merge", icon: "close", onSelect: () => update({ kind: "disableAutoMerge" }, "Disabling auto-merge") });
      else for (const m of METHODS) items.push({ label: `Auto-merge: ${m.label.toLowerCase()}`, icon: "clock-circle", onSelect: () => update({ kind: "enableAutoMerge", method: m.value }, "Enabling auto-merge") });
      items.push({ label: "", separator: true }, { label: "Close pull request", icon: "git-state-closed", danger: true, onSelect: () => update({ kind: "close" }, "Closing") });
    } else if (!merged()) items.push({ label: "Reopen pull request", icon: "git-state-open", onSelect: () => update({ kind: "reopen" }, "Reopening") });
    items.push({ label: "", separator: true }, { label: "Copy link", icon: "copy", onSelect: () => void copyText(d.url, "Link copied") }, { label: "Open on GitHub", icon: "arrow-up-right", onSelect: () => openExternal(d.url) });
    return items;
  };

  return (
    <>
      <div class="ws-panel-header">
        <IconButton icon="alt-arrow-left" label="All pull requests" size="sm" iconSize={14} onClick={() => props.onBack()} />
        <Show when={detail()} fallback={<h2 class="ws-panel-title ws-truncate">Pull request</h2>}>
          {(d) => (
            <>
              <PrStateIcon state={d().state} draft={d().isDraft} />
              <h2 class="ws-panel-title ws-truncate" title={d().title}>#{d().number}</h2>
            </>
          )}
        </Show>
        <div class="ws-grow" />
        <Show when={updating()}>
          <span class="ws-muted ws-small">{updating()}…</span>
        </Show>
        <Show when={loading()} fallback={<IconButton icon="refresh" label="Refresh pull request" size="sm" iconSize={14} onClick={() => read()} />}>
          <span class="ws-icon-slot"><Spinner label="Loading pull request" /></span>
        </Show>
        <Show when={detail()}>
          <Menu
            placement="bottom-end"
            trigger={(p) => (
              <button {...p} type="button" class="icon-btn" data-size="sm" aria-label="Pull request actions" title="Pull request actions" disabled={!!updating()}>
                <Icon name="more-horizontal" size={14} />
              </button>
            )}
            items={statusItems()}
          />
        </Show>
      </div>
      <Show when={error()}>
        <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={() => read()}>Retry</Button>}>{error()}</Banner>
      </Show>
      <Show when={notice()}>
        <Banner tone="warning">{notice()}</Banner>
      </Show>
      <Show when={detail()} fallback={<Show when={!error()}><div class="ws-empty"><Spinner label="Loading pull request" /></div></Show>}>
        {(d) => (
          <div class="ws-pr-detail">
            <div class="ws-pr-head">
              <h3 class="ws-pr-title">{d().title}</h3>
              <div class="ws-pr-meta ws-small ws-muted">
                <span>{d().author.login}</span>
                <span class="ws-faint">wants to merge into</span>
                <span class="ws-chip ws-mono">{d().baseRefName}</span>
                <span class="ws-faint">from</span>
                <span class="ws-chip ws-mono" title={d().headRefName}>{d().headRefName}</span>
                <Show when={props.own}>
                  <span class="ws-chip">This branch</span>
                </Show>
              </div>
              <div class="ws-pr-meta ws-small">
                <DiffStat additions={d().additions} deletions={d().deletions} />
                <span class="ws-faint">· {plural(d().files.length, "file")}</span>
                <Show when={d().reviewDecision}>
                  <span class="ws-faint">· {reviewLabel(d().reviewDecision)}</span>
                </Show>
              </div>
            </div>
            <div class="ws-subbar">
              <Tabs
                label="Pull request sections"
                value={tab()}
                onChange={setTab}
                items={[
                  { value: "overview", label: "Overview" },
                  { value: "activity", label: activity().length ? `Activity ${activity().length}` : "Activity" },
                  { value: "files", label: `Files ${d().files.length}` },
                ]}
              />
            </div>
            <Switch>
              <Match when={tab() === "overview"}>
                <div class="ws-pr-scroll">
                  <Show when={open()}>
                    <MergeBox detail={d()} busy={!!updating()} onMerge={(method) => update({ kind: "merge", method }, "Merging")} onReady={() => update({ kind: "markReady" }, "Marking ready")} />
                  </Show>
                  <Show when={checks().length}>
                    <section class="ws-pr-section" aria-label="Checks">
                      <div class="ws-section-label">Checks</div>
                      <For each={checks()}>{(check) => <CheckRow check={check} />}</For>
                    </section>
                  </Show>
                  <section class="ws-pr-section" aria-label="Description">
                    <div class="ws-section-label">Description</div>
                    <Show when={d().body.trim()} fallback={<div class="ws-faint ws-small">No description provided.</div>}>
                      <MarkdownBody text={d().body} />
                    </Show>
                  </section>
                </div>
              </Match>
              <Match when={tab() === "activity"}>
                <div class="ws-pr-scroll">
                  <For each={activity()} fallback={<Empty icon="chat-round-line" title="No discussion yet" />}>
                    {(c) => <CommentView comment={c} />}
                  </For>
                </div>
                <form
                  class="ws-pr-compose"
                  onSubmit={(e) => {
                    e.preventDefault();
                    post();
                  }}
                >
                  <textarea
                    class="field ws-textarea"
                    rows={3}
                    placeholder="Write a comment…"
                    aria-label="Comment"
                    value={comment()}
                    onInput={(e) => setComment(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                        e.preventDefault();
                        post();
                      }
                    }}
                  />
                  <div class="ws-form-row">
                    <span class="ws-faint ws-small ws-grow">Posts to GitHub as you.</span>
                    <Button size="sm" variant="primary" type="submit" disabled={!comment().trim() || posting() || !connected()}>{posting() ? "Posting…" : "Comment"}</Button>
                  </div>
                </form>
              </Match>
              <Match when={tab() === "files"}>
                <Show when={patchError()}>
                  <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={loadPatch}>Retry</Button>}>{patchError()}</Banner>
                </Show>
                <Show when={patch() !== null} fallback={<Show when={!patchError()}><div class="ws-empty"><Spinner label="Loading diff" /></div></Show>}>
                  <PatchView patch={patch() ?? ""} label={`${plural(d().files.length, "changed file")}`} />
                </Show>
              </Match>
            </Switch>
          </div>
        )}
      </Show>
    </>
  );
}

function PatchView(props: { patch: string; label: string }) {
  const files = createMemo(() => parsePatch(props.patch));
  return (
    <Show when={files().length} fallback={<Empty icon="document" title="No file changes" />}>
      <DiffView files={files()} label={props.label} />
    </Show>
  );
}

function reviewLabel(decision: string) {
  switch (decision.toUpperCase()) {
    case "APPROVED":
      return "Approved";
    case "CHANGES_REQUESTED":
      return "Changes requested";
    case "REVIEW_REQUIRED":
      return "Review required";
    default:
      return "";
  }
}

function MergeBox(props: { detail: ChangeRequestDetail; busy: boolean; onMerge: (method: Method) => void; onReady: () => void }) {
  const failing = () => props.detail.statusCheckRollup.filter((c) => /FAILURE|ERROR|TIMED_OUT|ACTION_REQUIRED|CANCELLED|STARTUP_FAILURE/.test(checkStatus(c))).length;
  const pending = () => props.detail.statusCheckRollup.filter((c) => /PENDING|QUEUED|IN_PROGRESS|EXPECTED|WAITING|REQUESTED/.test(checkStatus(c))).length;
  const conflicted = () => props.detail.mergeable.toUpperCase() === "CONFLICTING";
  const blocked = () => props.detail.isDraft || conflicted();
  const summary = () => {
    if (props.detail.isDraft) return "This pull request is still a draft.";
    if (conflicted()) return "This branch has conflicts that must be resolved.";
    if (props.detail.autoMergeRequest) return `Auto-merge is on (${props.detail.autoMergeRequest.mergeMethod.toLowerCase()}).`;
    if (failing()) return `${plural(failing(), "check")} failing.`;
    if (pending()) return `${plural(pending(), "check")} still running.`;
    if (props.detail.reviewDecision.toUpperCase() === "CHANGES_REQUESTED") return "Changes were requested.";
    if (props.detail.reviewDecision.toUpperCase() === "REVIEW_REQUIRED") return "A review is required before merging.";
    return "Ready to merge.";
  };
  return (
    <section class="ws-pr-merge" aria-label="Merge">
      <Icon name={blocked() || failing() ? "danger-triangle" : "git-merge"} size={14} class={blocked() || failing() ? "ws-warn" : "ws-muted"} />
      <span class="ws-grow ws-small">{summary()}</span>
      <Show
        when={!props.detail.isDraft}
        fallback={<Button size="sm" disabled={props.busy} onClick={() => props.onReady()}>Ready for review</Button>}
      >
        <Menu
          placement="bottom-end"
          trigger={(p) => (
            <button {...p} type="button" class="btn" data-variant="primary" data-size="sm" disabled={props.busy || conflicted()}>
              Merge
              <Icon name="alt-arrow-down" size={12} />
            </button>
          )}
          items={METHODS.map((m) => ({ label: m.label, onSelect: () => props.onMerge(m.value) }))}
        />
      </Show>
    </section>
  );
}

function checkStatus(check: ChangeRequestCheck) {
  return (check.conclusion || check.state || check.status || "").toUpperCase();
}

function CheckRow(props: { check: ChangeRequestCheck }) {
  const status = () => checkStatus(props.check);
  const tone = () =>
    /FAILURE|ERROR|TIMED_OUT|ACTION_REQUIRED|CANCELLED|STARTUP_FAILURE/.test(status())
      ? "danger"
      : /SUCCESS|NEUTRAL|SKIPPED/.test(status())
        ? "success"
        : "pending";
  const link = () => props.check.detailsUrl || props.check.targetUrl;
  const label = () => (tone() === "danger" ? "Failed" : tone() === "success" ? (status() === "SKIPPED" ? "Skipped" : "Passed") : "Running");
  return (
    <div class="ws-check-row">
      <span class="ws-check-mark">
        <Show when={tone() === "pending"} fallback={<span class="ws-dot" data-tone={tone()} role="img" aria-label={label()} />}>
          <TerminalSpinner size={14} tone="warning" label={label()} />
        </Show>
      </span>
      <span class="ws-truncate ws-grow">{props.check.name || props.check.context}</span>
      <span class="ws-faint ws-small">{label()}</span>
      <Show when={link()}>
        {(href) => <IconButton icon="arrow-up-right" label={`Open ${props.check.name || props.check.context}`} size="sm" iconSize={12} onClick={() => openExternal(href())} />}
      </Show>
    </div>
  );
}

function CommentView(props: { comment: ChangeRequestComment }) {
  const review = () => {
    const s = props.comment.state.toUpperCase();
    return s === "APPROVED" ? "approved" : s === "CHANGES_REQUESTED" ? "requested changes" : s === "COMMENTED" && !props.comment.body.trim() ? "reviewed" : "";
  };
  return (
    <article class="ws-comment">
      <div class="ws-comment-head ws-small">
        <span class="ws-comment-author">{props.comment.author.login || "Someone"}</span>
        <Show when={review()}>
          <span class="ws-muted">{review()}</span>
        </Show>
        <span class="ws-faint">{timeAgo(props.comment.submittedAt || props.comment.createdAt)}</span>
        <div class="ws-grow" />
        <Show when={props.comment.url}>
          <IconButton icon="arrow-up-right" label="Open on GitHub" size="sm" iconSize={12} onClick={() => openExternal(props.comment.url)} />
        </Show>
      </div>
      <Show when={props.comment.diffHunk}>
        <pre class="ws-comment-hunk">{props.comment.diffHunk.split("\n").slice(-4).join("\n")}</pre>
      </Show>
      <Show when={props.comment.body.trim()}>
        <MarkdownBody text={props.comment.body} />
      </Show>
    </article>
  );
}

/** Provider markdown is written by anyone who can open a PR or comment: raw HTML shows as text and images become links. */
function MarkdownBody(props: { text: string }) {
  // eslint-disable-next-line solid/no-innerhtml -- providerMarkdownHtml output is DOMPurify-sanitized
  return <div class="ws-markdown" innerHTML={providerMarkdownHtml(props.text)} />;
}
