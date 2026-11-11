// Git controls: stage, commit, push/pull/fetch, branches and pull request creation.
// Same behaviour as crates/viewport/src/git_controls.rs (rendered by desktop's GitPanel); actions never replay.
import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { Button, Menu, TextField, Toggle } from "@/ui/primitives";
import { Banner, Empty, Loading, statusLetter, statusTone } from "./parts";
import { emptyGit, errorText, gitPending, setGitSnapshot, setWorkspace, workspace, type Target } from "./store";
import type { GitFileStatus, GitOperation, GitWorkspace } from "./types";
import { basename, dirname, plural, timeAgo } from "./util";

type Primary = { kind: "publish" } | { kind: "pull" } | { kind: "push" } | { kind: "openPr"; url: string };

function primaryAction(s: GitWorkspace, created: string | null): Primary | null {
  if (!s.upstream) return s.headSha ? { kind: "publish" } : null;
  // A diverged branch has no one-click step: fast-forward pulls and plain pushes both refuse it.
  if (s.behind > 0 && s.ahead > 0) return null;
  if (s.behind > 0) return { kind: "pull" };
  if (s.ahead > 0) return { kind: "push" };
  return created ? { kind: "openPr", url: created } : null;
}

const staged = (f: GitFileStatus) => f.index !== "unchanged" && f.index !== "untracked";
const unstaged = (f: GitFileStatus) => f.worktree !== "unchanged";

export function GitPanel(props: { target: Target }) {
  // eslint-disable-next-line solid/reactivity -- RightPanel remounts this per chat
  const chatId = props.target.chatId;
  const git = () => workspace.git[chatId] ?? emptyGit();
  const snapshot = () => git().snapshot;
  const [pending, setPending] = createSignal<null | { mutation: boolean; label: string }>(null);
  const [notice, setNotice] = createSignal<string | null>(null);
  const [message, setMessage] = createSignal("");
  const [form, setForm] = createSignal<null | "create" | "switch" | "pr">(null);
  const [branchName, setBranchName] = createSignal("");
  const [pr, setPr] = createSignal({ title: "", body: "", base: "", repository: "", draft: true, showRepository: false });
  const [created, setCreated] = createSignal<string | null>(null);

  // Forms opened from the menu focus their first field; the menu hands focus back to its trigger as it
  // closes, so the trigger passes it on.
  let focusNext: HTMLElement | null = null;
  let focusExpiry: ReturnType<typeof setTimeout> | undefined;
  const focusSoon = (el: HTMLElement) => {
    focusNext = el;
    clearTimeout(focusExpiry);
    focusExpiry = setTimeout(() => (focusNext = null), 1500);
    queueMicrotask(() => el.focus());
  };
  const takeFocus = () => {
    const el = focusNext;
    focusNext = null;
    if (el?.isConnected) queueMicrotask(() => el.focus());
  };
  onCleanup(() => clearTimeout(focusExpiry));

  const available = () => !!props.target.cwd && !props.target.archived;
  const busy = () => !!pending();

  const send = (operation: GitOperation | null, label: string) => {
    if (pending() || !props.target.cwd) return;
    const mutation = !!operation;
    const current = snapshot();
    if (mutation && !current?.complete) return;
    const params: Record<string, unknown> = { chatId, cwd: props.target.cwd, targetDeviceId: props.target.deviceId };
    if (operation && current) Object.assign(params, { checkoutId: current.checkoutId, revision: current.revision, operation });
    if (mutation) {
      setNotice(null);
      gitPending.add(chatId);
    }
    setPending({ mutation, label });
    let settled = false;
    const deadline = setTimeout(
      () => {
        if (settled) return;
        settled = true;
        gitPending.delete(chatId);
        setPending(null);
        const text = mutation ? "Couldn't confirm the Git action. Check the current status before repeating it." : "Git status timed out. Check your device connection and refresh.";
        if (mutation) setNotice(text);
        else setWorkspace("git", chatId, (prev) => ({ ...emptyGit(), ...prev, error: text }));
      },
      mutation ? 150_000 : 30_000,
    );
    rpc
      .call<GitWorkspace>(mutation ? M.GIT_WORKSPACE_ACTION : M.GET_GIT_WORKSPACE, params)
      .then(
        (value) => {
          if (settled) return;
          if (value?.chatId !== chatId) {
            setWorkspace("git", chatId, (prev) => ({ ...emptyGit(), ...prev, error: "The Git response belongs to another thread." }));
            return;
          }
          setGitSnapshot(chatId, value);
          setWorkspace("git", chatId, "error", null);
          if (value.createdPullRequestUrl) {
            setCreated(value.createdPullRequestUrl);
            setForm(null);
            setPr({ title: "", body: "", base: "", repository: "", draft: true, showRepository: false });
          }
          if (operation?.action === "commit") setMessage("");
          if (operation?.action === "createBranch" || operation?.action === "switchBranch") setBranchName("");
        },
        (error) => {
          if (settled) return;
          const text = error instanceof RpcError && error.outcomeUnknown ? "Connection interrupted. Refresh Git before repeating an action." : errorText(error);
          if (mutation) {
            setNotice(operation?.action === "createPullRequest" ? `${text} Check pull requests before trying Create again.` : text);
            // Like desktop: the pre-action snapshot may be wrong now, so nothing acts on it until Git reports again.
            setWorkspace("git", chatId, (prev) => ({ ...emptyGit(), ...prev, snapshot: null }));
          } else setWorkspace("git", chatId, (prev) => ({ ...emptyGit(), ...prev, error: text }));
        },
      )
      .finally(() => {
        if (settled) return;
        settled = true;
        clearTimeout(deadline);
        gitPending.delete(chatId);
        setPending(null);
        if (mutation && notice()) send(null, "Refreshing");
      });
  };

  const stagedPaths = createMemo(() => (snapshot()?.files ?? []).filter(staged).map((f) => f.path));
  const unstagedPaths = createMemo(() => (snapshot()?.files ?? []).filter(unstaged).map((f) => f.path));
  const primary = () => {
    const s = snapshot();
    return s && available() ? primaryAction(s, created() ?? s.createdPullRequestUrl ?? null) : null;
  };
  const failure = () => notice() ?? git().error ?? git().fetchError;
  const canCommit = () => stagedPaths().length > 0 && message().trim().length > 0 && !busy();

  const menuItems = () => {
    const s = snapshot();
    const publish = !s?.upstream;
    return [
      { label: "Fetch", icon: "refresh", disabled: busy(), onSelect: () => send({ action: "fetch" }, "Fetching") },
      ...(publish ? [] : [{ label: "Pull", icon: "arrow-down", disabled: busy(), onSelect: () => send({ action: "pull" }, "Pulling") }]),
      { label: publish ? "Publish branch" : "Push", icon: "arrow-up", disabled: busy(), onSelect: () => send({ action: "push" }, publish ? "Publishing" : "Pushing") },
      { label: "", separator: true },
      { label: "Create pull request…", icon: "pull-request", onSelect: () => setForm("pr") },
      { label: "New branch…", icon: "git-branch", onSelect: () => (setBranchName(""), setForm("create")) },
      { label: "Switch branch…", icon: "git-merge", onSelect: () => (setBranchName(""), setForm("switch")) },
      { label: "", separator: true },
      { label: "Refresh", icon: "restart", disabled: busy(), onSelect: () => send(null, "Refreshing") },
    ];
  };

  return (
    <div class="ws-surface-col ws-git">
      <div class="ws-panel-header">
        <Icon name="git-branch" size={14} class="ws-muted" />
        <h2 class="ws-panel-title ws-truncate" title={snapshot()?.upstream ? `Tracking ${snapshot()!.upstream}` : undefined}>
          {snapshot()?.branch ?? props.target.branch ?? "Git"}
        </h2>
        <Show when={(snapshot()?.ahead ?? 0) > 0}>
          <span class="ws-muted ws-count" aria-label={`${snapshot()!.ahead} commits ahead`}>↑{snapshot()!.ahead}</span>
        </Show>
        <Show when={(snapshot()?.behind ?? 0) > 0}>
          <span class="ws-muted ws-count" aria-label={`${snapshot()!.behind} commits behind`}>↓{snapshot()!.behind}</span>
        </Show>
        <div class="ws-grow" />
        <Show when={pending()}>
          <span class="ws-muted ws-small">{pending()!.label}…</span>
        </Show>
        <Show when={primary()}>
          {(action) => (
            <Show
              when={action().kind !== "openPr"}
              fallback={
                <Button size="sm" variant="primary" onClick={() => window.open((action() as { url: string }).url, "_blank", "noopener")}>
                  Open PR
                </Button>
              }
            >
              <Button
                size="sm"
                variant="primary"
                disabled={busy()}
                onClick={() => {
                  const kind = action().kind;
                  if (kind === "pull") send({ action: "pull" }, "Pulling");
                  else send({ action: "push" }, kind === "publish" ? "Publishing" : "Pushing");
                }}
              >
                {action().kind === "publish" ? "Publish" : action().kind === "pull" ? `Pull ${snapshot()?.behind}` : `Push ${snapshot()?.ahead}`}
              </Button>
            </Show>
          )}
        </Show>
        <Show when={snapshot() && available()}>
          <Menu
            placement="bottom-end"
            trigger={(p) => (
              <button
                {...p}
                type="button"
                class="icon-btn"
                data-size="sm"
                aria-label="More Git actions"
                title="More Git actions"
                onFocus={(e) => {
                  (p.onFocus as ((event: FocusEvent) => void) | undefined)?.(e);
                  takeFocus();
                }}
              >
                <Icon name="more-horizontal" size={14} />
              </button>
            )}
            items={menuItems()}
          />
        </Show>
      </div>

      <Show when={available()} fallback={<Empty icon="git-branch" title={props.target.archived ? "Unarchive this thread to use Git." : "No Git checkout for this thread."} />}>
        <Show when={failure()}>
          <Banner tone="danger">{failure()}</Banner>
        </Show>
        <Show when={created()}>
          {(url) => (
            <Banner tone="info" actions={<Button size="sm" variant="ghost" onClick={() => window.open(url(), "_blank", "noopener")}>Open</Button>}>
              Pull request created.
            </Banner>
          )}
        </Show>
        <Show when={snapshot()} fallback={<Show when={!failure()} fallback={<div class="ws-git-body"><Button size="sm" onClick={() => send(null, "Refreshing")}>Refresh</Button></div>}><Loading label="Loading…" /></Show>}>
          {(s) => (
            <div class="ws-git-body">
              <Show when={form() === "pr"}>
                <form
                  class="ws-form"
                  onSubmit={(e) => {
                    e.preventDefault();
                    const value = pr();
                    send(
                      { action: "createPullRequest", title: value.title, body: value.body, base: value.base.trim(), baseRepository: value.repository.trim() || null, draft: value.draft },
                      "Creating",
                    );
                  }}
                >
                  <div class="ws-form-title">Create pull request</div>
                  <TextField placeholder="Title" aria-label="Pull request title" value={pr().title} onInput={(e) => setPr({ ...pr(), title: e.currentTarget.value })} ref={focusSoon} />
                  <textarea class="field ws-textarea" placeholder="Description" aria-label="Pull request description" rows={4} value={pr().body} onInput={(e) => setPr({ ...pr(), body: e.currentTarget.value })} />
                  <TextField placeholder="Base branch (optional)" aria-label="Base branch" value={pr().base} onInput={(e) => setPr({ ...pr(), base: e.currentTarget.value })} />
                  <Show when={pr().showRepository}>
                    <TextField placeholder="owner/repo" aria-label="Base repository" value={pr().repository} onInput={(e) => setPr({ ...pr(), repository: e.currentTarget.value })} />
                  </Show>
                  <div class="ws-form-row">
                    <label class="ws-inline-toggle">
                      <Toggle checked={pr().draft} label="Draft" onChange={(draft) => !busy() && setPr({ ...pr(), draft })} />
                      <span>Draft</span>
                    </label>
                    <Show when={!pr().showRepository}>
                      <button type="button" class="ws-link" onClick={() => setPr({ ...pr(), showRepository: true })}>Other repository…</button>
                    </Show>
                    <div class="ws-grow" />
                    <Button size="sm" variant="ghost" disabled={busy()} onClick={() => setForm(null)}>Cancel</Button>
                    <Button size="sm" variant="primary" type="submit" disabled={busy() || !s().complete}>{pending()?.label === "Creating" ? "Creating…" : "Create"}</Button>
                  </div>
                </form>
              </Show>
              <Show when={form() === "create" || form() === "switch"}>
                <form
                  class="ws-form-row"
                  onSubmit={(e) => {
                    e.preventDefault();
                    const name = branchName().trim();
                    if (!name) return;
                    const kind = form();
                    setForm(null);
                    send(kind === "create" ? { action: "createBranch", name } : { action: "switchBranch", name }, kind === "create" ? "Creating branch" : "Switching");
                  }}
                >
                  <TextField
                    placeholder="Branch name"
                    aria-label={form() === "create" ? "New branch name" : "Branch to switch to"}
                    value={branchName()}
                    onInput={(e) => setBranchName(e.currentTarget.value)}
                    ref={focusSoon}
                  />
                  <Button size="sm" variant="ghost" onClick={() => setForm(null)}>Cancel</Button>
                  <Button size="sm" variant="primary" type="submit" disabled={!branchName().trim() || busy()}>{form() === "create" ? "Create" : "Switch"}</Button>
                </form>
              </Show>
              <Show when={!s().complete}>
                <div class="ws-small ws-danger">Partial status. Actions are unavailable until Git reports every file.</div>
              </Show>
              <Show when={s().files.length} fallback={<div class="ws-small ws-muted ws-git-clean">No local changes</div>}>
                <button
                  type="button"
                  class="ws-git-row ws-git-all"
                  role="checkbox"
                  aria-checked={unstagedPaths().length === 0 ? "true" : stagedPaths().length === 0 ? "false" : "mixed"}
                  aria-label="Stage all changes"
                  disabled={busy() || !s().complete}
                  onClick={() =>
                    unstagedPaths().length === 0
                      ? send({ action: "unstage", paths: stagedPaths() }, "Unstaging")
                      : send({ action: "stage", paths: unstagedPaths() }, "Staging")
                  }
                >
                  <Check state={unstagedPaths().length === 0 ? true : stagedPaths().length === 0 ? false : null} />
                  <span>{plural(s().files.length, "change")}</span>
                  <span class="ws-grow" />
                  <Show when={stagedPaths().length}>
                    <span class="ws-small ws-muted">{stagedPaths().length} staged</span>
                  </Show>
                </button>
                <div class="ws-git-files">
                  <For each={s().files}>
                    {(file) => {
                      const isStaged = () => staged(file);
                      const work = () => unstaged(file);
                      const letter = () => statusLetter(work() ? file.worktree : file.index);
                      return (
                        <button
                          type="button"
                          class="ws-git-row"
                          role="checkbox"
                          aria-checked={isStaged() && !work() ? "true" : !isStaged() ? "false" : "mixed"}
                          aria-label={`${file.path}, ${isStaged() ? (work() ? "partly staged" : "staged") : "not staged"}`}
                          disabled={busy() || !s().complete}
                          title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
                          // A partly staged file stages the rest first.
                          onClick={() => send(work() ? { action: "stage", paths: [file.path] } : { action: "unstage", paths: [file.path] }, work() ? "Staging" : "Unstaging")}
                        >
                          <Check state={isStaged() && !work() ? true : !isStaged() ? false : null} />
                          <span class="ws-git-letter" data-tone={statusTone(letter())}>{letter()}</span>
                          <span class="ws-git-name">{basename(file.path)}</span>
                          <span class="ws-truncate ws-faint ws-small">{dirname(file.path)}</span>
                        </button>
                      );
                    }}
                  </For>
                </div>
                <form
                  class="ws-form-row ws-commit-form"
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (canCommit()) send({ action: "commit", message: message() }, "Committing");
                  }}
                >
                  <TextField placeholder="Commit message" aria-label="Commit message" value={message()} onInput={(e) => setMessage(e.currentTarget.value)} />
                  <Button size="sm" variant={canCommit() ? "primary" : "default"} type="submit" disabled={!canCommit() || !s().complete}>
                    Commit
                  </Button>
                </form>
              </Show>
              <Show when={git().lastFetchAt}>
                <div class="ws-small ws-faint">Fetched {timeAgo(git().lastFetchAt!)}</div>
              </Show>
              <Show when={state.connection.kind !== "connected"}>
                <div class="ws-small ws-muted">Disconnected. Git status will refresh when the connection returns.</div>
              </Show>
            </div>
          )}
        </Show>
      </Show>
    </div>
  );
}

function Check(props: { state: boolean | null }) {
  return (
    <span class="ws-check" data-state={props.state === true ? "on" : props.state === null ? "mixed" : "off"} aria-hidden="true">
      <Show when={props.state === true}>
        <Icon name="check" size={10} />
      </Show>
    </span>
  );
}
