// Workspace data for the selected chat: diff sizes, pull request, previews, live Git status.
// Engine-owned rows live here keyed by id; components read them and send requests.
import { batch, createEffect, createMemo, createRoot, on, onCleanup, untrack } from "solid-js";
import { createStore, reconcile, unwrap } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError, type Subscription } from "@/core/rpc";
import { state } from "@/core/store";
import { setView, view, type RightPanel } from "@/core/view";
import type { Chat, Space } from "@/core/protocol";
import { errorText } from "@/core/util";
import type {
  CheckoutChangeRequestStatus,
  CheckoutDiff,
  GitWorkspace,
  PreviewSnapshot,
  SourceControlFrame,
} from "./types";

export interface Target {
  chatId: string;
  deviceId: string;
  /** Null when the device is ours, so requests stay local. */
  targetDeviceId: string | null;
  cwd: string | null;
  archived: boolean;
  space: Space | null;
  git: boolean;
  local: boolean;
  branch: string | null;
  repoPath: string | null;
}

interface SourceContext {
  repoRoot?: string;
  branch?: string;
}

/** Where the selected chat runs, for every workspace request. */
export function selectedTarget(): Target | null {
  const id = view.selectedChatId;
  const chat: Chat | undefined = id ? state.chats[id] : undefined;
  if (!chat) return null;
  const space = chat.spaceId ? state.spaces[chat.spaceId] ?? null : null;
  const source = chat.sourceContext as SourceContext | undefined;
  const local = !state.localDeviceId || state.localDeviceId === chat.deviceId;
  const branch = (workspace.branches[chat.id] || source?.branch?.trim() || chat.branch?.trim()) ?? null;
  return {
    chatId: chat.id,
    deviceId: chat.deviceId,
    targetDeviceId: local ? null : chat.deviceId,
    cwd: chat.cwd ?? space?.path ?? null,
    archived: chat.archived,
    space,
    git: !!space?.gitDetected || !!chat.checkoutId,
    local,
    branch: branch || null,
    repoPath: source?.repoRoot ?? chat.cwd ?? space?.path ?? null,
  };
}

export function sameTarget(a: Target | null, b: Target | null) {
  if (a === b) return true;
  if (!a || !b) return false;
  return (Object.keys(a) as (keyof Target)[]).every((key) => a[key] === b[key]);
}

/** Memo-friendly accessor: only changes when a target field changes, not on every chat row update. */
export const targetMemoOptions = { equals: sameTarget };

/** Params naming the chat's host so remote devices route correctly. */
export function hostParams(target: Target, params: Record<string, unknown> = {}) {
  return { ...params, targetDeviceId: target.deviceId };
}

export interface DiffStat {
  checkoutId: string;
  deviceId: string;
  cwd: string;
  additions: number;
  deletions: number;
  files: number;
  checksum: string;
}

export interface GitState {
  snapshot: GitWorkspace | null;
  updatedAt: number | null;
  refreshing: boolean;
  error: string | null;
  fetchError: string | null;
  lastFetchAt: number | null;
}

interface WorkspaceData {
  diffStats: Record<string, DiffStat>;
  changeRequests: Record<string, CheckoutChangeRequestStatus | null>;
  git: Record<string, GitState>;
  previews: Record<string, PreviewSnapshot & { failed?: string | null }>;
  /** The checkout's current branch per chat; chat rows keep the branch they started on. */
  branches: Record<string, string>;
  /** Open right-panel surfaces, in strip order. */
  tabs: Exclude<RightPanel, null>[];
  lastPanel: Exclude<RightPanel, null>;
  sessionInfoOpen: boolean;
}

export const [workspace, setWorkspace] = createStore<WorkspaceData>({
  diffStats: {},
  changeRequests: {},
  git: {},
  previews: {},
  branches: {},
  tabs: [],
  lastPanel: "files",
  sessionInfoOpen: false,
});

/** A stream that resubscribes after it ends with an error while connected. */
export function liveSubscribe<T>(method: string, params: unknown, onItem: (value: T) => void, onError?: (error: RpcError) => void): Subscription {
  let sub: Subscription | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let cancelled = false;
  const open = () => {
    sub = rpc.subscribe<T>(method, params, onItem, (error) => {
      if (cancelled || !error) return;
      onError?.(error);
      if (rpc.connected) retry = setTimeout(open, 2000);
    });
  };
  open();
  return {
    cancel() {
      cancelled = true;
      clearTimeout(retry);
      sub?.cancel();
    },
  };
}

/** A read the engine doesn't list as retryable still fails as "may have completed"; a read changed nothing. */
export function readErrorText(error: unknown, fallback: string, reloads = false) {
  if (error instanceof RpcError && error.outcomeUnknown)
    return reloads ? "The connection closed before the reply arrived. This reloads when it returns." : "The connection closed before the reply arrived. Try again once it returns.";
  return errorText(error, fallback);
}

export { errorText };

/** The selected chat's diff size, matched like desktop: checkout id, then device and cwd, then cwd. */
export function diffStatFor(chatId: string): DiffStat | null {
  const chat = state.chats[chatId];
  if (!chat) return null;
  const stats = workspace.diffStats;
  if (chat.checkoutId && stats[chat.checkoutId]) return stats[chat.checkoutId]!;
  if (!chat.cwd) return null;
  const all = Object.values(stats);
  return all.find((s) => s.deviceId === chat.deviceId && s.cwd === chat.cwd) ?? all.find((s) => s.cwd === chat.cwd) ?? null;
}

function applyDiffFrame(value: unknown) {
  const rows = (Array.isArray(value) ? value : value && typeof value === "object" ? [value] : []) as CheckoutDiff[];
  batch(() => {
    for (const diff of rows) {
      if (!diff?.checkoutId) continue;
      setWorkspace("diffStats", diff.checkoutId, reconcile({
        checkoutId: diff.checkoutId,
        deviceId: diff.deviceId,
        cwd: diff.cwd,
        additions: diff.additions,
        deletions: diff.deletions,
        files: diff.files?.length ?? 0,
        checksum: diff.checksum,
      }));
    }
  });
}

/** Open a surface in the right panel, adding its tab when needed. */
export function openPanel(panel: Exclude<RightPanel, null>) {
  batch(() => {
    if (!workspace.tabs.includes(panel)) setWorkspace("tabs", (tabs) => [...tabs, panel]);
    setWorkspace("lastPanel", panel);
    setView("rightPanel", panel);
  });
}

export function closePanelTab(panel: Exclude<RightPanel, null>) {
  const index = workspace.tabs.indexOf(panel);
  const tabs = workspace.tabs.filter((tab) => tab !== panel);
  batch(() => {
    setWorkspace("tabs", tabs);
    if (view.rightPanel === panel) {
      // The right neighbour takes over, else the left, like desktop.
      const next = tabs[Math.min(index, tabs.length - 1)] ?? null;
      setView("rightPanel", next);
      if (next) setWorkspace("lastPanel", next);
    }
  });
}

export function toggleRightPanel() {
  if (view.rightPanel) setView("rightPanel", null);
  else openPanel(workspace.tabs.includes(workspace.lastPanel) ? workspace.lastPanel : workspace.tabs[0] ?? "files");
}

// Diff sizes from every device we can see, patches stripped. Desktop keeps the same watch.
onConnected(() => liveSubscribe(M.WATCH_CHECKOUT_DIFFS, { summaryOnly: true }, applyDiffFrame));

const sameItems = (a: readonly unknown[], b: readonly unknown[]) => a.length === b.length && a.every((v, i) => v === b[i]);

/** Run `start` whenever the key's values change; its subscription ends when they change again. */
export function keyedWatch<K extends readonly unknown[]>(key: () => K, start: (key: K) => Subscription | void) {
  const memo = createMemo(key, undefined, { equals: sameItems });
  createEffect(
    on(memo, (value) => {
      const sub = start(value);
      if (sub) onCleanup(() => sub.cancel());
    }),
  );
}

const connected = () => state.connection.kind === "connected";

createRoot(() => {
  // Shell links (sidebar, palette) may set view.rightPanel directly; keep the strip in step.
  createEffect(() => {
    const panel = view.rightPanel;
    if (!panel) return;
    if (!untrack(() => workspace.tabs.includes(panel))) setWorkspace("tabs", (tabs) => [...tabs, panel]);
    setWorkspace("lastPanel", panel);
  });

  // A remote chat's host streams its own diff sizes.
  keyedWatch(
    () => [selectedTarget()?.targetDeviceId ?? null, state.epoch, connected()] as const,
    ([device, , live]) => (device && live ? liveSubscribe(M.WATCH_CHECKOUT_DIFFS, { summaryOnly: true, targetDeviceId: device }, applyDiffFrame) : undefined),
  );

  // The branch's pull request, resolved on the checkout's host.
  keyedWatch(
    () => {
      const t = selectedTarget();
      return [t?.chatId, t?.git && !t.archived ? t.cwd : null, t?.branch, t?.deviceId, state.epoch, connected()] as const;
    },
    ([chatId, cwd, branch, deviceId, , live]) =>
      chatId && cwd && live
        ? liveSubscribe<CheckoutChangeRequestStatus>(
            M.WATCH_CHECKOUT_CHANGE_REQUEST,
            { cwd, branch: branch ?? undefined, targetDeviceId: deviceId },
            (status) => setWorkspace("changeRequests", chatId, reconcile(status)),
          )
        : undefined,
  );

  // The top bar's branch follows checkouts and switches made elsewhere: re-read when the diff changes.
  let branchTimer: ReturnType<typeof setTimeout> | undefined;
  keyedWatch(
    () => {
      const t = selectedTarget();
      const usable = t?.git && !t.archived && t.cwd;
      return [usable ? t.chatId : null, t?.cwd, t?.deviceId, usable ? diffStatFor(t.chatId)?.checksum ?? null : null, state.epoch, connected()] as const;
    },
    ([chatId, cwd, deviceId, , , live]) => {
      clearTimeout(branchTimer);
      if (!chatId || !cwd || !deviceId || !live) return;
      branchTimer = setTimeout(() => refreshBranch(chatId, cwd, deviceId), 300);
    },
  );

  // Live Git status only while the Git panel shows: the watch also fetches remotes periodically.
  keyedWatch(
    () => {
      const t = selectedTarget();
      const visible = view.rightPanel === "git" && view.route.kind === "chat";
      return [visible && t?.cwd && !t.archived ? t.chatId : null, t?.cwd, t?.deviceId, state.epoch, connected()] as const;
    },
    ([chatId, cwd, deviceId, , live]) =>
      chatId && cwd && live
        ? liveSubscribe<SourceControlFrame>(
            M.WATCH_GIT_WORKSPACE,
            { chatId, cwd, targetDeviceId: deviceId },
            (frame) => applyGitFrame(chatId, frame),
            (error) => setWorkspace("git", chatId, (prev) => ({ ...emptyGit(), ...prev, error: error.message })),
          )
        : undefined,
  );

  // Previews only while the Browser panel shows.
  keyedWatch(
    () => [view.rightPanel === "browser" ? view.selectedChatId : null, state.epoch, connected()] as const,
    ([chatId, , live]) =>
      chatId && live
        ? liveSubscribe<PreviewSnapshot>(
            M.WATCH_PREVIEWS,
            { chatId },
            (snapshot) => setWorkspace("previews", chatId, reconcile({ ...snapshot, failed: null })),
            (error) => setWorkspace("previews", chatId, (prev) => ({ ...(prev ?? { services: [], proxyPort: 7331 }), failed: error.message })),
          )
        : undefined,
  );
});

export function refreshBranch(chatId: string, cwd: string, deviceId: string) {
  rpc.call<GitWorkspace>(M.GET_GIT_WORKSPACE, { chatId, cwd, targetDeviceId: deviceId }).then(
    (value) => value?.chatId === chatId && value.branch && setWorkspace("branches", chatId, value.branch),
    () => {},
  );
}

export function emptyGit(): GitState {
  return { snapshot: null, updatedAt: null, refreshing: false, error: null, fetchError: null, lastFetchAt: null };
}

/** Pending Git mutations hold the snapshot they started from; frames wait until the reply lands. */
export const gitPending = new Set<string>();

export function applyGitFrame(chatId: string, frame: SourceControlFrame) {
  if (gitPending.has(chatId)) return;
  const prev = workspace.git[chatId];
  // A refreshing frame still seeds an empty panel on reopen.
  const usable = !frame.refreshing || !prev?.snapshot;
  const snapshot = usable && frame.snapshot && frame.snapshot.chatId === chatId ? frame.snapshot : prev?.snapshot ?? null;
  if (snapshot?.branch) setWorkspace("branches", chatId, snapshot.branch);
  setWorkspace("git", chatId, reconcile({
    snapshot: snapshot ? structuredClone(unwrap(snapshot)) : null,
    updatedAt: usable && frame.snapshot ? frame.updatedAt ?? null : prev?.updatedAt ?? null,
    refreshing: frame.refreshing,
    error: frame.error ?? null,
    fetchError: frame.fetchError ?? null,
    lastFetchAt: frame.lastFetchAt ?? null,
  }));
}

/** Install a snapshot returned by a Git request. */
export function setGitSnapshot(chatId: string, snapshot: GitWorkspace) {
  setWorkspace("git", chatId, (prev) => ({ ...emptyGit(), ...prev, snapshot: null }));
  setWorkspace("git", chatId, "snapshot", reconcile(snapshot));
  if (snapshot.branch) setWorkspace("branches", chatId, snapshot.branch);
}
