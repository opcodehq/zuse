// The conversation's top bar: project mark, branch switcher with changed-file count, copy controls,
// then the session card, terminal and right-panel toggles. Mirrors crates/ui/src/shell/top_bar.rs.
import { Popover } from "@kobalte/core/popover";
import { createMemo, createSignal, For, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { narrowLayout, setSidebarVisible, sidebarVisible, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { IconButton, Spinner } from "@/ui/primitives";
import { SessionInfo } from "./SessionInfo";
import { diffStatFor, errorText, openPanel, readErrorText, refreshBranch, selectedTarget, targetMemoOptions, setWorkspace, toggleRightPanel, workspace, type Target } from "./store";
import { basename, copyText, plural, rovingKeys } from "./util";

export function TopBar() {
  const target = createMemo(selectedTarget, null, targetMemoOptions);
  return (
    <header class="ws-topbar" aria-label="Conversation">
      {/* A hidden sidebar comes back from here as well as with its shortcut. The drawer covers it on narrow
          windows, where it stays mounted so closing the drawer can hand focus back. */}
      <Show when={!sidebarVisible() || narrowLayout()}>
        <IconButton icon="split-columns" label="Show sidebar" size="sm" iconSize={14} onClick={() => setSidebarVisible(true)} />
      </Show>
      <div class="ws-topbar-identity">
        <Show when={target()}>{(t) => <Identity target={t()} />}</Show>
      </div>
      <Show when={target()}>
        <div class="ws-topbar-actions">
          <SessionInfoToggle />
          <IconButton
            icon="terminal"
            label="Toggle terminal"
            size="sm"
            iconSize={14}
            active={view.rightPanel === "terminal"}
            aria-pressed={view.rightPanel === "terminal"}
            onClick={() => (view.rightPanel === "terminal" ? toggleRightPanel() : openPanel("terminal"))}
          />
          <IconButton
            icon="split-columns"
            label={view.rightPanel ? "Hide panel" : "Show panel"}
            size="sm"
            iconSize={14}
            active={!!view.rightPanel}
            aria-pressed={!!view.rightPanel}
            onClick={toggleRightPanel}
          />
        </div>
      </Show>
    </header>
  );
}

function projectName(target: Target) {
  return target.space?.name?.trim() || (target.space ? basename(target.space.path) : target.cwd ? basename(target.cwd) : "Workspace");
}

function Identity(props: { target: Target }) {
  const name = () => projectName(props.target);
  const branch = () => props.target.branch;
  return (
    <nav class="ws-identity" aria-label="Branch">
      <span class="ws-project-mark" aria-hidden="true">{name().slice(0, 1).toUpperCase()}</span>
      <Show
        when={props.target.git && branch()}
        fallback={<span class="ws-identity-name" title={props.target.cwd ?? undefined}>{name()}</span>}
      >
        {(label) => (
          <>
            <BranchMenu target={props.target} branch={label()} />
            <IconButton icon="copy" label="Copy branch name" size="sm" iconSize={14} onClick={() => copyText(label(), "Branch name copied")} />
          </>
        )}
      </Show>
    </nav>
  );
}

function BranchMenu(props: { target: Target; branch: string }) {
  const [open, setOpen] = createSignal(false);
  const [branches, setBranches] = createSignal<string[]>([]);
  const [loadedFor, setLoadedFor] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [switching, setSwitching] = createSignal<string | null>(null);
  const dirty = () => diffStatFor(props.target.chatId)?.files ?? 0;
  const checkout = () => props.target.cwd;
  const worktree = () => !!props.target.space && !!checkout() && checkout() !== props.target.space.path;

  const load = () => {
    const repoPath = props.target.repoPath;
    if (!repoPath) return;
    const key = `${props.target.deviceId}:${repoPath}`;
    if (loadedFor() !== key) setBranches([]);
    setLoadedFor(key);
    setLoading(true);
    rpc
      .call<string[]>(M.LIST_BRANCHES, { repoPath, chatId: props.target.chatId, targetDeviceId: props.target.deviceId })
      .then(
        (list) => loadedFor() === key && setBranches(Array.isArray(list) ? list : []),
        (e) => loadedFor() === key && setError(readErrorText(e, "Couldn't list branches.")),
      )
      .finally(() => loadedFor() === key && setLoading(false));
  };

  const switchTo = (name: string) => {
    if (switching()) return;
    if (name === props.branch) return setOpen(false);
    const repoPath = props.target.cwd ?? props.target.repoPath;
    if (!repoPath) return;
    setError(null);
    setSwitching(name);
    rpc
      .call(M.SWITCH_REF, { repoPath, refName: name, targetDeviceId: props.target.deviceId })
      .then(
        () => {
          setOpen(false);
          if (props.target.cwd) refreshBranch(props.target.chatId, props.target.cwd, props.target.deviceId);
        },
        (e) => setError(errorText(e)),
      )
      .finally(() => setSwitching(null));
  };

  return (
    <Popover
      open={open()}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) {
          setError(null);
          load();
        }
      }}
      placement="bottom-start"
      gutter={4}
    >
      <Popover.Trigger class="ws-branch-trigger" aria-label={`Switch branch, current ${props.branch}`} title={props.branch}>
        <span class="ws-branch-label">{props.branch}</span>
        <Show when={dirty() > 0}>
          <span class="ws-muted">· {dirty()}</span>
        </Show>
        <Icon name="alt-arrow-down" size={12} class="ws-muted" />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content class="ws-popover ws-branch-menu" aria-label="Branches">
          <div class="ws-branch-head">
            <Icon name="git-branch" size={14} class="ws-muted" />
            <span class="ws-branch-current">{props.branch}</span>
            <IconButton icon="copy" label="Copy branch name" size="sm" iconSize={14} onClick={() => copyText(props.branch, "Branch name copied")} />
          </div>
          <Show when={checkout()}>
            {(path) => (
              <div class="ws-branch-head">
                <Icon name={worktree() ? "worktree" : "folder"} size={14} class="ws-muted" />
                <span class="ws-branch-path" title={path()}>
                  {worktree() ? "Worktree" : "Local checkout"} <span class="ws-muted">{path()}</span>
                </span>
                <IconButton icon="copy" label={worktree() ? "Copy worktree path" : "Copy checkout path"} size="sm" iconSize={14} onClick={() => copyText(path(), "Path copied")} />
              </div>
            )}
          </Show>
          <div class="ws-menu-sep" />
          <Show when={error()}>
            <div class="ws-menu-note ws-danger" role="alert">{error()}</div>
          </Show>
          <div class="ws-menu-label">Local branches</div>
          <div class="ws-branch-list" role="listbox" aria-label="Local branches" onKeyDown={(e) => rovingKeys(e, ".ws-branch-row")}>
            <For each={branches()}>
              {(name) => (
                <button
                  type="button"
                  class="ws-branch-row"
                  role="option"
                  aria-selected={name === props.branch}
                  disabled={!!switching() && switching() !== name}
                  onClick={() => switchTo(name)}
                >
                  <Icon name="check" size={14} style={{ opacity: name === props.branch ? 1 : 0 }} />
                  <span class="ws-truncate">{name}</span>
                  <Show when={switching() === name}>
                    <span class="ws-muted ws-row-meta">Switching…</span>
                  </Show>
                </button>
              )}
            </For>
            <Show when={!branches().length}>
              <div class="ws-menu-note ws-muted">
                <Show when={loading()} fallback="No local branches">
                  <Spinner label="Loading branches" /> Loading branches…
                </Show>
              </div>
            </Show>
          </div>
          <Show when={dirty() > 0}>
            <div class="ws-menu-sep" />
            <div class="ws-menu-note ws-muted">
              {plural(dirty(), "uncommitted change")}. Git refuses a switch that would overwrite them.
            </div>
          </Show>
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}

function SessionInfoToggle() {
  return (
    <Popover open={workspace.sessionInfoOpen} onOpenChange={(next) => setWorkspace("sessionInfoOpen", next)} placement="bottom-end" gutter={6}>
      <Popover.Trigger
        class="icon-btn"
        data-size="sm"
        data-active={workspace.sessionInfoOpen ? "true" : undefined}
        aria-label="Toggle environment summary"
        title="Environment summary"
      >
        <Icon name="list" size={14} />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          class="ws-popover ws-session-card"
          aria-label="Environment summary"
          // Desktop's card opens without a row focused; the card itself holds focus so Escape and Tab still work.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)?.focus({ preventScroll: true });
          }}
        >
          <SessionInfo onClose={() => setWorkspace("sessionInfoOpen", false)} />
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}
