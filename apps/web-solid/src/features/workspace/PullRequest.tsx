// Pull requests for the chat's repository: the branch's own pull request when it has one, the
// repository's list otherwise, and a detail view with checks, discussion, files, diff and status
// actions. Reads follow crates/ui/src/pull_request_detail.rs; updates are explicit and never resent.
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { openChat, view } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { draftFor, focusComposer, setDraft } from "@/features/composer/drafts";
import { chatReferences, decorateReferences, referenceClick } from "@/features/transcript/ReferenceMarkdown";
import { issueLabel, type ReferenceCatalog } from "@/features/transcript/references";
import { Icon } from "@/ui/Icon";
import { TerminalSpinner } from "@/ui/loaders";
import { providerMarkdownHtml } from "@/ui/markdown";
import { Button, Dialog, IconButton, Menu, Spinner, Tabs, toast, type MenuItem } from "@/ui/primitives";
import { Popover } from "@kobalte/core/popover";
import { type JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { parsePatch } from "./diff";
import { addPrompt, checksSummary, checksTone, checkTone, cleanBody, fixAllPrompt, fixPrompt, groupFeedback, sortChecks, statusLine, type CheckTone, type FeedbackItem } from "./feedback";
import { DiffView } from "./DiffView";
import { allowedMethods, MERGE_METHODS, mergeButton, pickMethod, type MergeMethod } from "./merge";
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

const openExternal = (url: string) => window.open(url, "_blank", "noopener,noreferrer");

export function PullRequest(props: { target: Target; active: boolean; url?: string; onBack?: ()=>void }) {
  const own = () => workspace.changeRequests[props.target.chatId]?.changeRequest ?? null;
  const [picked, setPicked] = createSignal<string | null>(null);
  // The branch's pull request shows by default; the list is one step back.
  const [browsing, setBrowsing] = createSignal(false);
  const url = () => props.url ?? picked() ?? (browsing() ? null : own()?.url ?? null);

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
              if(props.url) {props.onBack?.();return;}
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
          (() => {
            const seen = new Set(append ? items().map((i) => i.url) : []);
            setItems([...(append ? items() : []), ...result.items.filter((i) => !seen.has(i.url))]);
            setNext(result.nextCursor ?? null);
            setTotal(result.totalCount ?? null);
            setError(null);
            setLoaded(true);
          })();
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

  createEffect(() => [(() => props.active)(), (connected)(), (() => state.epoch)()] as const, ([active, live]) => { void (active && live && refresh()); });
  createEffect(filter, () => page(false), { defer: true });

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
  createEffect(() => [(connected)(), (() => state.epoch)()] as const, ([live]) => {
    const cleanups: (() => void)[] = [];
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
        if (frame.reset) (() => (setDetail(null), setPatch(null)))();
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
    cleanups.push(() => sub.cancel());

    return () => cleanups.forEach(cleanup => cleanup());
  });

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
  createEffect(() => [(tab)(), (() => detail()?.headRefOid)()] as const, ([t, head]) => { void (t === "files" && head && patch() === null && loadPatch()); });
  createEffect(() => detail()?.headRefOid, () => { void (setPatch(null)); }, { defer: true });

  const update = (change: ChangeRequestUpdate, label: string): Promise<boolean> => {
    const d = detail();
    if (!d || updating()) return Promise.resolve(false);
    const merging = change.kind === "merge" || change.kind === "enableAutoMerge";
    setUpdating(label);
    setNotice(null);
    return rpc
      .call<{ refusal?: string | null }>(merging ? M.UPDATE_CHANGE_REQUEST_AT_HEAD : M.UPDATE_CHANGE_REQUEST, host({ url: d.url, update: change, headRefOid: d.headRefOid }))
      .then(
        (result) => {
          if (result?.refusal) setNotice(result.refusal);
          read();
          return !result?.refusal;
        },
        (e) => {
          if (e instanceof RpcError && e.outcomeUnknown) setNotice("The connection dropped. Refresh the pull request before trying again; the change may have been made.");
          else setNotice(errorText(e, "GitHub refused the change."));
          return false;
        },
      )
      .finally(() => setUpdating(null));
  };

  /** Merging or enabling auto-merge posts the optional comment once GitHub accepts the change. */
  const updateWithComment = (change: ChangeRequestUpdate, label: string, body: string) =>
    update(change, label).then((done) => {
      if (!done || !body.trim()) return done;
      return rpc.call(M.POST_CHANGE_REQUEST_COMMENT, host({ url: props.url, body })).then(
        () => (read(), true),
        (e) => (setNotice(e instanceof RpcError && e.outcomeUnknown ? "Comment delivery is uncertain. Check the discussion before posting again." : errorText(e, "Couldn't post the comment.")), true),
      );
    });

  const reply = (threadId: string, body: string) =>
    rpc.call(M.REPLY_CHANGE_REQUEST_THREAD, host({ url: props.url, threadId, body })).then(
      () => (read(), true),
      (e) => (setNotice(e instanceof RpcError && e.outcomeUnknown ? "Reply delivery is uncertain. Refresh before replying again." : errorText(e, "Couldn't post the reply.")), false),
    );

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

  createEffect(() => [comment(), props.url], () => {
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
  const references = chatReferences({ get chatId() { return props.target.chatId; } });
  const chat = () => state.chats[props.target.chatId];
  const issue = () => {
    const url = detail()?.url.replace(/\/$/, "");
    return zapperForChat(props.target.chatId)?.issues.find((i) => i.pullRequests.some((pr) => pr.url.replace(/\/$/, "") === url));
  };

  /** Puts a fix prompt into the linked chat's composer, or the current chat's, without sending it. */
  const addToChat = (text: string, what: string, replaces: string[] = []) => {
    const chatId = chat() ? props.target.chatId : view.selectedChatId ?? "";
    setDraft(chatId, { text: addPrompt(draftFor(chatId).text, text, replaces) });
    if (chatId && view.selectedChatId !== chatId) openChat(chatId);
    focusComposer();
    toast(`${what} added to the chat`);
  };

  const [mergeOpen, setMergeOpen] = createSignal(false);
  const [confirmClose, setConfirmClose] = createSignal(false);
  const linkedChats = () => {
    const url = detail()?.url.replace(/\/$/, "");
    return Object.entries(workspace.changeRequests).flatMap(([chatId, status]) => (status?.changeRequest?.url.replace(/\/$/, "") === url && state.chats[chatId] ? [state.chats[chatId]!] : []));
  };

  const overflowItems = (): MenuItem[] => {
    const d = detail();
    if (!d) return [];
    const chats = linkedChats();
    const items: MenuItem[] = [
      { label: "Open in GitHub", icon: "pr-github", onSelect: () => openExternal(d.url) },
      {
        label: "Copy",
        icon: "copy",
        children: [
          { label: "Link", onSelect: () => void copyText(d.url, "Link copied") },
          { label: "Branch name", hint: d.headRefName.length > 28 ? `${d.headRefName.slice(0, 27)}…` : d.headRefName, onSelect: () => void copyText(d.headRefName, "Branch name copied") },
          { label: "Number", hint: `#${d.number}`, onSelect: () => void copyText(`#${d.number}`, "Number copied") },
        ],
      },
      { label: "Threads", icon: "chat-round-line", children: chats.length ? chats.map((c) => ({ label: c.title || "Untitled chat", onSelect: () => openChat(c.id) })) : [{ label: "No linked threads", disabled: true }] },
      { label: "Refresh", icon: "refresh", onSelect: () => read() },
    ];
    if (open()) {
      items.push({ label: "", separator: true }, d.isDraft ? { label: "Ready for review", icon: "eye", onSelect: () => void update({ kind: "markReady" }, "Marking ready") } : { label: "Convert to draft", icon: "git-state-draft", onSelect: () => void update({ kind: "markDraft" }, "Converting to draft") });
      if (!d.isDraft) items.push({ label: "Merge pull request", icon: "git-merge", onSelect: () => requestAnimationFrame(() => setMergeOpen(true)) });
      items.push({ label: "", separator: true }, { label: "Close pull request", icon: "close", danger: true, onSelect: () => setConfirmClose(true) });
    } else if (!merged()) items.push({ label: "", separator: true }, { label: "Reopen pull request", icon: "git-state-open", onSelect: () => void update({ kind: "reopen" }, "Reopening") });
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
        <Show when={loading()}>
          <span class="ws-icon-slot"><Spinner label="Loading pull request" /></span>
        </Show>
        <Show when={detail() && mergeButton(detail()!) && detail()}>
          {(d) => <MergeControl detail={d()} busy={!!updating()} open={mergeOpen()} onOpenChange={setMergeOpen} onRun={updateWithComment} />}
        </Show>
        <Show when={detail()}>
          <Menu
            placement="bottom-end"
            trigger={(p) => (
              <button {...p} type="button" class="icon-btn" data-size="sm" aria-label="Pull request actions" title="Pull request actions" disabled={!!updating()}>
                <Icon name="more-horizontal" size={14} />
              </button>
            )}
            items={overflowItems()}
          />
        </Show>
      </div>
      <Dialog
        open={confirmClose()}
        onOpenChange={setConfirmClose}
        title="Close this pull request?"
        description="It stays on GitHub and can be reopened later."
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setConfirmClose(false)}>Cancel</Button>
            <Button size="sm" variant="danger" onClick={() => { setConfirmClose(false); void update({ kind: "close" }, "Closing"); }}>Close pull request</Button>
          </>
        }
      />
      <Show when={error()}>
        <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={() => read()}>Retry</Button>}>{error()}</Banner>
      </Show>
      <Show when={notice()}>
        <Banner tone="warning">{notice()}</Banner>
      </Show>
      <Show when={detail()} fallback={<Show when={!error()}><div class="ws-empty"><Spinner label="Loading pull request" /></div></Show>}>
        {(d) => (
          <div class="ws-pr-detail">
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
                  <div class="ws-pr-head">
                    <h3 class="ws-pr-title">{d().title}</h3>
                    <div class="ws-pr-meta">
                      <Avatar login={d().author.login} />
                      <span class="ws-pr-author">{d().author.login}</span>
                      <span class="ws-faint">· #{d().number}</span>
                      <Show when={props.own}>
                        <span class="ws-chip">This branch</span>
                      </Show>
                    </div>
                  </div>
                  <dl class="ws-pr-props">
                    <Property icon="git-branch" label="Branch">
                      <span class="ws-truncate ws-pr-branch" title={d().headRefName}>{d().headRefName}</span>
                      <Icon name="alt-arrow-right" size={12} class="ws-faint" />
                      <span class="ws-pr-branch">{d().baseRefName}</span>
                      <span class="ws-faint ws-nowrap">{plural(d().files.length, "file")}</span>
                      <DiffStat additions={d().additions} deletions={d().deletions} />
                    </Property>
                    <Property icon="pull-request" label="Status">
                      <StatusText detail={d()} />
                    </Property>
                    <Property icon="eye" label="Reviews">
                      <span class="ws-truncate">{reviewsText(d())}</span>
                    </Property>
                    <Show when={d().statusCheckRollup.length}>
                      <Property icon="pr-checks" label="Checks">
                        <CheckMark tone={checksTone(d().statusCheckRollup)} />
                        <span class="ws-truncate">{checksSummary(d().statusCheckRollup)}</span>
                      </Property>
                    </Show>
                    <Show when={chat()}>
                      <Property icon="chat-round-line" label="Thread">
                        <ReferenceText text={props.target.chatId} references={references()} chatId={props.target.chatId} />
                      </Property>
                    </Show>
                    <Show when={issue()}>
                      {(found) => (
                        <Property icon="checklist" label="Issue">
                          <ReferenceText text={`${issueLabel(found().number)} ${found().details.title}`} references={references()} chatId={props.target.chatId} />
                        </Property>
                      )}
                    </Show>
                  </dl>
                  <Description body={d().body} references={references()} chatId={props.target.chatId} />
                  <Show when={d().statusCheckRollup.length}>
                    <ChecksSection checks={d().statusCheckRollup} />
                  </Show>
                  <FeedbackSection detail={d()} onAdd={addToChat} onReply={reply} />
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

function reviewsText(detail: ChangeRequestDetail) {
  const reviewers = [...new Set(detail.reviews.filter((r) => r.author.login && r.author.login !== detail.author.login).map((r) => r.author.login))];
  const decision = reviewLabel(detail.reviewDecision);
  if (!reviewers.length) return decision || "No reviews";
  return decision ? `${decision} · ${reviewers.join(", ")}` : reviewers.join(", ");
}

function Property(props: { icon: string; label: string; children: JSX.Element }) {
  return (
    <div class="ws-pr-prop">
      <dt><Icon name={props.icon} size={14} class="ws-pr-prop-icon" />{props.label}</dt>
      <dd>{props.children}</dd>
    </div>
  );
}

function StatusText(props: { detail: ChangeRequestDetail }) {
  const status = () => statusLine(props.detail);
  return (
    <span class="ws-pr-status ws-truncate">
      <PrStateIcon state={props.detail.state} draft={props.detail.isDraft} />
      <span class="ws-pr-state" data-tone={status().tone}>{status().state}</span>
      <Show when={status().detail}>
        <span class="ws-faint">·</span>
        <span class="ws-muted">{status().detail}</span>
      </Show>
    </span>
  );
}

const METHOD_KEY = "zuse.pullRequest.mergeMethod";
const readMethod = () => {
  try {
    return localStorage.getItem(METHOD_KEY);
  } catch {
    return null;
  }
};

/**
 * One split button: the main half follows the pull request's state (Merge, Auto-merge, Auto-merge on); the
 * menu holds an optional comment, the repository's merge methods, and auto-merge or force-merge.
 */
function MergeControl(props: { detail: ChangeRequestDetail; busy: boolean; open: boolean; onOpenChange: (open: boolean) => void; onRun: (change: ChangeRequestUpdate, label: string, comment: string) => Promise<boolean> }) {
  const button = () => mergeButton(props.detail) ?? { label: "Merge", action: "merge" as const, primary: false };
  const methods = () => allowedMethods(props.detail);
  const [remembered, setRemembered] = createSignal(readMethod());
  const method = () => pickMethod(methods(), remembered());
  const methodLabel = () => MERGE_METHODS.find((m) => m.value === method())!.label;
  const [comment, setComment] = createSignal("");
  const [forcing, setForcing] = createSignal(false);
  const armed = () => !!props.detail.autoMergeRequest;
  const choose = (value: MergeMethod) => {
    setRemembered(value);
    try {
      localStorage.setItem(METHOD_KEY, value);
    } catch {
      // Private windows may refuse storage; the pick then lasts for the tab.
    }
  };
  const run = (change: ChangeRequestUpdate, label: string) =>
    void props.onRun(change, label, armed() ? "" : comment()).then((done) => {
      if (!done) return;
      setComment("");
      setForcing(false);
      props.onOpenChange(false);
    });
  const main = () => {
    const action = button().action;
    if (action === "menu") return props.onOpenChange(!props.open);
    if (action === "markReady") return run({ kind: "markReady" }, "Marking ready");
    if (action === "merge") return run({ kind: "merge", method: method() }, "Merging");
    run({ kind: "enableAutoMerge", method: method() }, "Enabling auto-merge");
  };
  return (
    <Popover open={props.open} onOpenChange={(next) => { props.onOpenChange(next); if (!next) setForcing(false); }} placement="bottom-end" gutter={4}>
      <Popover.Anchor class="ws-pr-merge" data-primary={button().primary ? "" : undefined}>
        <button type="button" class="btn ws-pr-merge-main" data-variant={button().primary ? "primary" : "default"} data-size="sm" disabled={props.busy} onClick={main}>
          <Icon name={button().action === "markReady" ? "eye" : "git-merge"} size={14} />
          {button().label}
        </button>
        <Show when={!props.detail.isDraft}>
          <Popover.Trigger class="btn ws-pr-merge-more" data-variant={button().primary ? "primary" : "default"} data-size="sm" aria-label="Merge options" disabled={props.busy}>
            <Icon name="alt-arrow-down" size={12} />
          </Popover.Trigger>
        </Show>
      </Popover.Anchor>
      <Popover.Portal>
        <Popover.Content class="menu ws-merge-menu" aria-label="Merge options">
          <Show when={!armed()}>
            <textarea class="field ws-merge-comment" rows={3} placeholder="Leave a comment (optional)" aria-label="Merge comment" value={comment()} onInput={(e) => setComment(e.currentTarget.value)} />
            <div class="ws-merge-label">Select method</div>
            <div role="radiogroup" aria-label="Merge method">
              <For each={methods()}>
                {(m) => (
                  <button type="button" role="radio" class="ws-merge-row" aria-checked={m.value === method() ? "true" : "false"} onClick={() => choose(m.value)}>
                    <span class="ws-merge-text">
                      <span>{m.label}</span>
                      <span class="ws-merge-detail">{m.detail}</span>
                    </span>
                    <Show when={m.value === method()}><Icon name="check" size={14} /></Show>
                  </button>
                )}
              </For>
            </div>
          </Show>
          <Show when={!armed()}>
            <div class="menu-sep" />
          </Show>
          <Switch>
            <Match when={armed()}>
              <div class="ws-merge-label">Auto-merge is on ({props.detail.autoMergeRequest!.mergeMethod.toLowerCase()})</div>
              <button type="button" class="ws-merge-row" onClick={() => run({ kind: "disableAutoMerge" }, "Cancelling auto-merge")}>
                <span class="ws-merge-text"><span>Cancel auto-merge</span><span class="ws-merge-detail">Wait for someone to merge by hand</span></span>
              </button>
            </Match>
            <Match when={button().action === "merge"}>
              <button type="button" class="ws-merge-row" onClick={() => run({ kind: "merge", method: method() }, "Merging")}>
                <span class="ws-merge-text"><span>Merge pull request</span><span class="ws-merge-detail">{methodLabel()}</span></span>
              </button>
            </Match>
            <Match when={forcing()}>
              <div class="ws-merge-confirm" role="alertdialog" aria-label="Confirm force merge">
                <span class="ws-merge-detail">Merge before checks pass? GitHub still refuses if branch rules require them.</span>
                <div class="ws-form-row">
                  <div class="ws-grow" />
                  <Button size="sm" variant="ghost" onClick={() => setForcing(false)}>Cancel</Button>
                  <Button size="sm" variant="danger" onClick={() => run({ kind: "merge", method: method() }, "Merging")}>Force merge</Button>
                </div>
              </div>
            </Match>
            <Match when={true}>
              <button type="button" class="ws-merge-row" onClick={() => run({ kind: "enableAutoMerge", method: method() }, "Enabling auto-merge")}>
                <span class="ws-merge-text"><span>Enable auto-merge</span><span class="ws-merge-detail">Merge when checks pass</span></span>
              </button>
              <button type="button" class="ws-merge-row" onClick={() => setForcing(true)}>
                <span class="ws-merge-text"><span>Force merge</span><span class="ws-merge-detail">Attempt before checks pass</span></span>
                <Icon name="alt-arrow-right" size={12} class="ws-faint" />
              </button>
            </Match>
          </Switch>
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}

const escapeHtml = (text: string) => text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

/** Plain text with issue, pull request and thread references drawn as the transcript's chips. */
function ReferenceText(props: { text: string; references: ReferenceCatalog; chatId: string }) {
  return (
    // eslint-disable-next-line solid/no-innerhtml -- escaped text; decorateReferences only adds chip anchors
    <span class="ws-pr-ref ws-truncate" innerHTML={decorateReferences(escapeHtml(props.text), props.references)} onClick={(event) => referenceClick(event, props.references, props.chatId)} />
  );
}

function Description(props: { body: string; references: ReferenceCatalog; chatId: string }) {
  const [expanded, setExpanded] = createSignal(false);
  const long = () => props.body.length > 360 || props.body.split("\n").length > 8;
  return (
    <section class="ws-pr-section" aria-label="Description">
      <div class="ws-section-label">Description</div>
      <Show when={props.body.trim()} fallback={<div class="ws-faint ws-small">No description provided.</div>}>
        <div class="ws-pr-desc" data-clamped={long() && !expanded() ? "" : undefined}>
          <MarkdownBody text={props.body} references={props.references} chatId={props.chatId} />
        </div>
        <Show when={long()}>
          <button type="button" class="ws-pr-link" aria-expanded={expanded() ? "true" : "false"} onClick={() => setExpanded(!expanded())}>{expanded() ? "Show less" : "Show more"}</button>
        </Show>
      </Show>
    </section>
  );
}

function SectionHead(props: { label: string; count: number; open: boolean; onToggle: () => void; children?: JSX.Element }) {
  return (
    <div class="ws-pr-section-head">
      <button type="button" class="ws-pr-section-toggle" aria-expanded={props.open ? "true" : "false"} onClick={() => props.onToggle()}>
        {props.label}
        <span class="ws-faint ws-count">{props.count}</span>
        <Icon name={props.open ? "alt-arrow-down" : "alt-arrow-right"} size={12} class="ws-faint" />
      </button>
      <div class="ws-grow" />
      {props.children}
    </div>
  );
}

const CHECK_LABELS: Record<CheckTone, string> = { running: "Running", failed: "Failed", passed: "Passed", skipped: "Skipped" };

function CheckMark(props: { tone: CheckTone }) {
  return (
    <span class="ws-check-mark" data-tone={props.tone}>
      <Switch fallback={<Icon name="check" size={14} label={CHECK_LABELS[props.tone]} />}>
        <Match when={props.tone === "running"}><TerminalSpinner size={13} label={CHECK_LABELS.running} /></Match>
        <Match when={props.tone === "failed"}><Icon name="close" size={14} label={CHECK_LABELS.failed} /></Match>
        <Match when={props.tone === "skipped"}><span class="ws-check-skip" role="img" aria-label={CHECK_LABELS.skipped} /></Match>
      </Switch>
    </span>
  );
}

function ChecksSection(props: { checks: ChangeRequestCheck[] }) {
  const [open, setOpen] = createSignal(true);
  return (
    <section class="ws-pr-section" aria-label="Checks">
      <SectionHead label="Checks" count={props.checks.length} open={open()} onToggle={() => setOpen(!open())} />
      <Show when={open()}>
        <For each={sortChecks(props.checks)}>{(check) => <CheckRow check={check} />}</For>
      </Show>
    </section>
  );
}

function CheckRow(props: { check: ChangeRequestCheck }) {
  const tone = () => checkTone(props.check);
  const name = () => props.check.name || props.check.context;
  const link = () => props.check.detailsUrl || props.check.targetUrl;
  return (
    <div class="ws-check-row" data-tone={tone()}>
      <CheckMark tone={tone()} />
      <span class="ws-truncate ws-grow">{name()}</span>
      <span class="ws-faint">{CHECK_LABELS[tone()]}</span>
      <Show when={link()} fallback={<span class="ws-check-link" />}>
        {(href) => <IconButton class="ws-check-link" icon="arrow-up-right" label={`Open ${name()}`} size="sm" iconSize={12} onClick={() => openExternal(href())} />}
      </Show>
    </div>
  );
}

function FeedbackSection(props: { detail: ChangeRequestDetail; onAdd: (text: string, what: string, replaces?: string[]) => void; onReply: (threadId: string, body: string) => Promise<boolean> }) {
  const groups = createMemo(() => groupFeedback(props.detail));
  const [open, setOpen] = createSignal(true);
  const [showClosed, setShowClosed] = createSignal(false);
  // Keyed by item id, so a refresh after a reply or a merge keeps rows open.
  const [expanded, setExpanded] = createSignal<ReadonlySet<string>>(new Set());
  const toggle = (id: string) => {
    const next = new Set(expanded());
    if (!next.delete(id)) next.add(id);
    setExpanded(next);
  };
  return (
    <Show when={groups().open.length || groups().closed.length}>
      <section class="ws-pr-section" aria-label="Feedback">
        <SectionHead label="Feedback" count={groups().open.length} open={open()} onToggle={() => setOpen(!open())}>
          <Show when={groups().open.length}>
            <Button size="sm" variant="ghost" icon="pr-add-to-chat" class="ws-fb-add-all" onClick={() => props.onAdd(fixAllPrompt(groups().open, props.detail), plural(groups().open.length, "comment"), groups().open.map(fixPrompt))}>Add all</Button>
          </Show>
        </SectionHead>
        <Show when={open()}>
          <Show when={groups().open.length}>
            <div class="ws-fb-list" role="list" aria-label="Open feedback">
              <For each={groups().open}>{(item) => <FeedbackRow item={item} expanded={expanded().has(item.id)} onToggle={() => toggle(item.id)} onAdd={(text) => props.onAdd(text, "Comment")} onReply={item.kind === "thread" ? (body) => props.onReply(item.id, body) : undefined} />}</For>
            </div>
          </Show>
          <Show when={groups().closed.length}>
            <button type="button" class="ws-pr-link ws-fb-closed-toggle" aria-expanded={showClosed() ? "true" : "false"} onClick={() => setShowClosed(!showClosed())}>
              <Icon name="check" size={12} />
              {groups().closed.length} resolved
              <Icon name={showClosed() ? "alt-arrow-down" : "alt-arrow-right"} size={12} />
            </button>
            <Show when={showClosed()}>
              <div class="ws-fb-list" role="list" aria-label="Resolved feedback">
                <For each={groups().closed}>{(item) => <FeedbackRow item={item} expanded={expanded().has(item.id)} onToggle={() => toggle(item.id)} onAdd={(text) => props.onAdd(text, "Comment")} onReply={(body) => props.onReply(item.id, body)} />}</For>
              </div>
            </Show>
          </Show>
        </Show>
      </section>
    </Show>
  );
}

function FeedbackRow(props: { item: FeedbackItem; expanded: boolean; onToggle: () => void; onAdd: (text: string) => void; onReply?: (body: string) => Promise<boolean> }) {
  const expanded = () => props.expanded;
  const closed = () => props.item.resolved || props.item.outdated;
  const prompt = () => fixPrompt(props.item);
  return (
    <article class="ws-fb" role="listitem" data-closed={closed() ? "" : undefined} data-expanded={expanded() ? "" : undefined}>
      <div class="ws-fb-row">
        <button type="button" class="ws-fb-toggle" aria-expanded={expanded() ? "true" : "false"} onClick={() => props.onToggle()}>
          <Show when={closed()} fallback={<Avatar login={props.item.author} />}>
            <Icon name="check" size={14} class="ws-fb-check" label={props.item.resolved ? "Resolved" : "Outdated"} />
          </Show>
          <span class="ws-fb-main">
            <span class="ws-fb-line">
              <span class="ws-fb-author ws-truncate">{props.item.author}</span>
              <span class="ws-faint ws-nowrap">{timeAgo(props.item.createdAt)}</span>
              <Show when={props.item.outdated && !props.item.resolved}>
                <span class="ws-faint ws-nowrap">· Outdated</span>
              </Show>
            </span>
            <span class="ws-fb-summary ws-truncate">
              <Show when={props.item.severity}>
                <span class="ws-fb-severity">{props.item.severity}</span>
                <span class="ws-faint"> · </span>
              </Show>
              <span class="ws-fb-title">{props.item.title || "Comment"}</span>
            </span>
          </span>
        </button>
        <div class="ws-fb-actions">
          <IconButton icon="pr-add-to-chat" label="Add to chat" size="sm" iconSize={14} onClick={() => props.onAdd(prompt())} />
          <Show when={props.item.url}>
            <IconButton icon="arrow-up-right" label="Open on GitHub" size="sm" iconSize={12} onClick={() => openExternal(props.item.url)} />
          </Show>
        </div>
      </div>
      <Show when={expanded()}>
        <div class="ws-fb-body">
          <Show when={props.item.path}>
            <div class="ws-fb-path ws-mono ws-faint ws-truncate">{props.item.line ? `${props.item.path}:${props.item.line}` : props.item.path}</div>
          </Show>
          <Show when={props.item.diffHunk}>
            <pre class="ws-comment-hunk">{props.item.diffHunk.split("\n").slice(-6).join("\n")}</pre>
          </Show>
          <MarkdownBody text={cleanBody(props.item.body)} />
          <Show when={props.item.replies}>
            <div class="ws-faint ws-small">{plural(props.item.replies, "reply", "replies")} on GitHub</div>
          </Show>
          <div class="ws-fb-prompt">
            <div class="ws-fb-prompt-head">
              <Icon name="bot" size={12} />
              <span class="ws-grow">Prompt to fix</span>
              <IconButton icon="copy" label="Copy prompt" size="sm" iconSize={12} onClick={() => void copyText(prompt(), "Prompt copied")} />
            </div>
            <pre class="ws-fb-prompt-text">{prompt()}</pre>
          </div>
          <Show when={props.onReply}>{(send) => <ReplyBox onSend={send()} />}</Show>
        </div>
      </Show>
    </article>
  );
}

/** An inline reply to a review thread, posted to GitHub as the signed-in account. */
function ReplyBox(props: { onSend: (body: string) => Promise<boolean> }) {
  const [text, setText] = createSignal("");
  const [sending, setSending] = createSignal(false);
  const send = () => {
    const body = text();
    if (!body.trim() || sending()) return;
    setSending(true);
    void props.onSend(body).then((done) => done && setText("")).finally(() => setSending(false));
  };
  return (
    <form class="ws-fb-reply" onSubmit={(e) => { e.preventDefault(); send(); }}>
      <textarea
        class="field ws-textarea"
        rows={2}
        placeholder="Reply…"
        aria-label="Reply to this thread"
        value={text()}
        onInput={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div class="ws-form-row">
        <span class="ws-faint ws-small ws-grow">Replies on GitHub as you.</span>
        <Button size="sm" type="submit" disabled={!text().trim() || sending()}>{sending() ? "Replying…" : "Reply"}</Button>
      </div>
    </form>
  );
}

/** A GitHub avatar over the login's initial, which stays when the image cannot load. */
function Avatar(props: { login: string }) {
  const [failed, setFailed] = createSignal(false);
  const owner = () => props.login.replace(/\[bot\]$/, "");
  return (
    <span class="ws-avatar" aria-hidden="true">
      {(owner()[0] ?? "?").toUpperCase()}
      <Show when={owner() && !failed()}>
        <img src={`https://github.com/${encodeURIComponent(owner())}.png?size=40`} alt="" loading="lazy" onError={() => setFailed(true)} />
      </Show>
    </span>
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
function MarkdownBody(props: { text: string; references?: ReferenceCatalog; chatId?: string }) {
  const html = () => {
    const safe = providerMarkdownHtml(props.text);
    return props.references ? decorateReferences(safe, props.references) : safe;
  };
  return (
    // eslint-disable-next-line solid/no-innerhtml -- providerMarkdownHtml output is DOMPurify-sanitized; decorateReferences only adds chip anchors
    <div class="ws-markdown" innerHTML={html()} onClick={(event) => props.references && props.chatId !== undefined && referenceClick(event, props.references, props.chatId)} />
  );
}
