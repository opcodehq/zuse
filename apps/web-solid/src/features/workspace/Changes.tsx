// Changes review: the checkout's diff for one scope (working tree, branch, latest turn), refreshed
// whenever the engine reports a new working-tree checksum. Mirrors crates/ui/src/changes.rs.
import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import { createStore } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { Button, Dialog, IconButton, Menu, toast } from "@/ui/primitives";
import { Icon } from "@/ui/Icon";
import { parsePatch } from "./diff";
import { DiffView } from "./DiffView";
import { Banner, DiffStat, Empty, Loading } from "./parts";
import { diffStatFor, errorText, openPanel, readErrorText, type Target } from "./store";
import type { CheckoutDiff } from "./types";
import { openFileInPanel } from "./files/model";
import { copyText, plural } from "./util";

type Scope = "workingTree" | "branch" | "turn" | "commit";
const SCOPES: { value: Exclude<Scope, "commit">; label: string }[] = [
  { value: "workingTree", label: "Working tree" },
  { value: "branch", label: "Branch changes" },
  { value: "turn", label: "Latest turn" },
];

interface ReviewScope {
  scope: Scope;
  commit: { sha: string; subject: string } | null;
}

/** The scope each chat's Changes surface shows; History opens a commit here. */
const [scopes, setScopes] = createStore<Record<string, ReviewScope>>({});

export function reviewCommit(chatId: string, sha: string, subject: string) {
  setScopes(chatId, { scope: "commit", commit: { sha, subject } });
  openPanel("changes");
}

export function defaultBaseRef(branches: string[], current: string | null) {
  const first = branches[0];
  if (!first) return null;
  if (current !== first) return first;
  for (const candidate of ["main", "master"]) if (branches.includes(candidate)) return candidate;
  return branches.find((b) => b !== current) ?? first;
}

export function Changes(props: { target: Target }) {
  // eslint-disable-next-line solid/reactivity -- RightPanel remounts this per chat
  const chatId = props.target.chatId;
  const scope = () => scopes[chatId]?.scope ?? "workingTree";
  const commit = () => (scope() === "commit" ? scopes[chatId]?.commit ?? null : null);
  const setScope = (next: Scope) => setScopes(chatId, { scope: next, commit: next === "commit" ? commit() : null });
  const [diff, setDiff] = createSignal<CheckoutDiff | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [branches, setBranches] = createSignal<string[]>([]);
  const [base, setBase] = createSignal<string | null>(null);
  const [wrap, setWrap] = createSignal(false);
  const [confirmDiscard, setConfirmDiscard] = createSignal(false);
  const [discarding, setDiscarding] = createSignal(false);
  let request = 0;

  const files = createMemo(() => parsePatch(diff()?.patch ?? ""));
  const connected = () => state.connection.kind === "connected";
  const checksum = () => diffStatFor(props.target.chatId)?.checksum ?? null;

  const refresh = () => {
    const cwd = props.target.cwd;
    if (!cwd) return;
    if (!connected()) {
      setError("Reconnect to read changes.");
      return;
    }
    if (scope() === "branch" && !base()) return;
    if (scope() === "commit" && !commit()) return;
    const id = ++request;
    setLoading(true);
    rpc
      .call<CheckoutDiff>(M.GET_CHECKOUT_DIFF, {
        cwd,
        mode: scope(),
        chatId: props.target.chatId,
        baseRef: scope() === "branch" ? base() : undefined,
        commitSha: commit()?.sha,
        targetDeviceId: props.target.deviceId,
      })
      .then(
        (value) => {
          if (id !== request) return;
          setDiff(value);
          setError(null);
        },
        (e) => id === request && setError(errorText(e, "Couldn't read changes.")),
      )
      .finally(() => id === request && setLoading(false));
  };

  const loadBranches = () => {
    const repoPath = props.target.repoPath;
    if (!repoPath) return;
    rpc.call<string[]>(M.LIST_BRANCHES, { repoPath, chatId: props.target.chatId, targetDeviceId: props.target.deviceId }).then(
      (list) => {
        const names = Array.isArray(list) ? list : [];
        setBranches(names);
        if (!base()) setBase(defaultBaseRef(names, props.target.branch));
      },
      (e) => setError(readErrorText(e, "Couldn't list branches.")),
    );
  };

  // A different scope is a different diff: never show the previous one under the new label.
  createEffect(on([scope, () => commit()?.sha, base], () => (setDiff(null), setError(null)), { defer: true }));
  // Re-read when the scope, base or the checkout's checksum changes, and after reconnects.
  let timer: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on([scope, () => commit()?.sha, base, checksum, () => state.epoch, connected], () => {
      clearTimeout(timer);
      timer = setTimeout(refresh, diff() ? 250 : 0);
    }),
  );
  createEffect(on(scope, (s) => s === "branch" && !branches().length && loadBranches()));
  onCleanup(() => clearTimeout(timer));

  const label = () => {
    const count = files().length;
    if (scope() === "workingTree") return count === 1 ? "1 Uncommitted change" : `${count} Uncommitted changes`;
    if (scope() === "branch") return `${count} Changed ${count === 1 ? "file" : "files"}${base() ? ` vs ${base()}` : ""}`;
    if (scope() === "commit") return `${count} Changed ${count === 1 ? "file" : "files"} in ${commit()?.sha.slice(0, 7) ?? "commit"}`;
    return `${count} Changed ${count === 1 ? "file" : "files"} this turn`;
  };
  const clean = () =>
    scope() === "workingTree"
      ? "No uncommitted changes"
      : scope() === "branch"
        ? base() ? `No changes vs ${base()}` : "No branch changes"
        : scope() === "commit"
          ? "This commit changes no files"
          : "No changes this turn";
  // Expected states the engine reports as errors read as quiet notes, like desktop.
  const quietNotice = () => {
    const message = scope() === "workingTree" ? null : error();
    if (!message) return null;
    if (message.includes("no turn recorded")) return "No turn recorded yet. Send a message first.";
    if (message.includes("unknown method")) return "This chat's device is running an older Zuse. Update it to view branch and turn diffs.";
    return null;
  };
  const scopeLabel = () => (scope() === "commit" ? `Commit ${commit()?.sha.slice(0, 7) ?? ""}` : SCOPES.find((s) => s.value === scope())?.label ?? "");

  const discard = () => {
    const current = diff();
    if (!current) return;
    setDiscarding(true);
    rpc
      .call(M.DISCARD_WORKING_TREE, { chatId: props.target.chatId, checkoutId: current.checkoutId, expectedChecksum: current.checksum, targetDeviceId: props.target.deviceId })
      .then(
        () => {
          toast("Changes discarded");
          setConfirmDiscard(false);
          refresh();
        },
        (e) => toast(errorText(e, "Couldn't discard changes."), "danger"),
      )
      .finally(() => setDiscarding(false));
  };

  return (
    <div class="ws-surface-col">
      <div class="ws-panel-header">
        <Menu
          trigger={(p) => (
            <button {...p} type="button" class="ws-select" aria-label={`Diff scope: ${scopeLabel()}`} title={commit()?.subject}>
              {scopeLabel()}
              <Icon name="alt-arrow-down" size={12} class="ws-muted" />
            </button>
          )}
          items={SCOPES.map((s) => ({ label: s.label, icon: s.value === scope() ? "check" : undefined, onSelect: () => setScope(s.value) }))}
        />
        <Show when={scope() === "branch"}>
          <Menu
            trigger={(p) => (
              <button {...p} type="button" class="ws-select" aria-label={`Compare with ${base() ?? "branch"}`}>
                <Icon name="git-branch" size={12} class="ws-muted" />
                {base() ?? "Choose base"}
                <Icon name="alt-arrow-down" size={12} class="ws-muted" />
              </button>
            )}
            items={
              branches().length
                ? branches().map((name) => ({ label: name, icon: name === base() ? "check" : undefined, onSelect: () => setBase(name) }))
                : [{ label: "No branches", disabled: true }]
            }
          />
        </Show>
        <div class="ws-grow" />
        <Show when={diff() && (diff()!.additions || diff()!.deletions)}>
          <DiffStat additions={diff()!.additions} deletions={diff()!.deletions} />
        </Show>
        <IconButton
          icon="copy"
          label="Copy all patches"
          size="sm"
          iconSize={14}
          disabled={!diff()?.patch}
          onClick={() => void copyText(diff()?.patch ?? "", "Patch copied")}
        />
        <IconButton icon="wrap-text" label="Wrap long lines" size="sm" iconSize={14} active={wrap()} aria-pressed={wrap()} onClick={() => setWrap(!wrap())} />
        <IconButton icon="refresh" label="Refresh" size="sm" iconSize={14} disabled={loading()} onClick={refresh} />
        <Show when={scope() === "workingTree" && !props.target.archived}>
          <IconButton icon="restore" label="Discard all changes" size="sm" iconSize={14} disabled={!files().length || discarding()} onClick={() => setConfirmDiscard(true)} />
        </Show>
      </div>
      <Show when={error() && !quietNotice()}>
        <Banner tone="danger" actions={<Button size="sm" variant="ghost" onClick={refresh}>Retry</Button>}>{error()}</Banner>
      </Show>
      <Show when={quietNotice()}>{(text) => <Empty icon="git-compare" title={text()} />}</Show>
      <Show when={diff()?.truncated}>
        <Banner tone="warning">Partial snapshot: the patch was larger than the engine sends at once.</Banner>
      </Show>
      <Show when={props.target.cwd} fallback={<Empty icon="git-compare" title="No checkout" detail="This conversation has no Git checkout." />}>
        <Show when={!quietNotice() && diff()} fallback={<Show when={!error()}><Loading label="Preparing diff…" /></Show>}>
          <Show when={files().length} fallback={<Empty icon="check-circle" title={clean()} />}>
            <ChangedFiles files={files()} />
            <DiffView files={files()} wrap={wrap()} label={label()} onOpenFile={(path) => openFileInPanel(path)} />
          </Show>
        </Show>
      </Show>
      <Dialog
        open={confirmDiscard()}
        onOpenChange={setConfirmDiscard}
        title="Discard all changes?"
        description={`This restores ${plural(files().length, "file")} to the last commit and deletes untracked files. It cannot be undone.`}
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirmDiscard(false)}>Cancel</Button>
            <Button variant="danger" disabled={discarding()} onClick={discard}>{discarding() ? "Discarding…" : "Discard"}</Button>
          </>
        }
      />
    </div>
  );
}

function ChangedFiles(props: { files: ReturnType<typeof parsePatch> }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="ws-changed-files">
      <button type="button" class="ws-changed-toggle" aria-expanded={open()} onClick={() => setOpen(!open())}>
        <Icon name={open() ? "alt-arrow-down" : "alt-arrow-right"} size={11} class="ws-faint" />
        <span>{plural(props.files.length, "file")}</span>
      </button>
      <Show when={open()}>
        <ul class="ws-changed-list">
          <For each={props.files}>
            {(file) => (
              <li>
                <button type="button" class="ws-changed-row" onClick={() => openFileInPanel(file.path)} disabled={file.status === "deleted"}>
                  <span class="ws-truncate ws-grow">{file.path}</span>
                  <DiffStat additions={file.additions} deletions={file.deletions} />
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}
