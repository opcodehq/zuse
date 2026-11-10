// The environment summary card: where the chat runs, what it changed, its pull request, background work and the
// project's actions. Mirrors crates/ui/src/shell/session_info.rs: 288px card, 28px rows with a 16px icon slot and 12px
// labels, no dividers but a fading rule between the workspace rows and the session's activity.
import { createMemo, createSignal, For, lazy, onCleanup, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { chatRequest } from "@/core/chat";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { deviceOnline } from "@/core/util";
import { Icon } from "@/ui/Icon";
import { DitherRing } from "@/ui/loaders";
import type { BackgroundTask } from "./types";
import { diffStatFor, openPanel, selectedTarget, targetMemoOptions, workspace } from "./store";
import { DiffStat, PrStateIcon } from "./parts";

// The project's run actions, as desktop's card ends with them; loaded with the first open card.
const ProjectActions = lazy(() => import("@/features/projects/ProjectActions").then((m) => ({ default: m.ProjectActions })));

/** Desktop's format_elapsed: 42s, 3m 5s, 2h 10m, 1d 4h. */
function elapsed(since: string, now: number) {
  const secs = Math.max(0, Math.floor((now - Date.parse(since)) / 1000));
  if (!Number.isFinite(secs)) return "";
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}

// Sections folded away from their default stay that way while the page lives, as desktop's session_info_folded.
const [backgroundOpen, setBackgroundOpen] = createSignal(false);
const [actionsOpen, setActionsOpen] = createSignal(false);

export function SessionInfo(props: { onClose: () => void }) {
  const target = createMemo(selectedTarget, null, targetMemoOptions);
  return (
    <Show when={target()} fallback={<div class="ws-menu-note ws-muted">No conversation selected.</div>}>
      {(t) => {
        const device = () => state.devices[t().deviceId];
        const online = () => t().local || deviceOnline(t().deviceId);
        const stat = () => diffStatFor(t().chatId);
        const pr = () => workspace.changeRequests[t().chatId]?.changeRequest ?? null;
        const worktree = () => !!t().space && !!t().cwd && t().cwd !== t().space!.path;
        const background = () =>
          ((state.sessions[t().chatId]?.backgroundTasks as BackgroundTask[] | undefined) ?? []).filter((task) => !task.endedAt && task.kind !== "agent");
        const go = (panel: Parameters<typeof openPanel>[0]) => {
          props.onClose();
          openPanel(panel);
        };
        return (
          <div class="ws-session-body">
            {/* Where it runs: the device (a remote one shows whether it is reachable; its tooltip names the project folder). */}
            <div class="ws-summary-row" title={t().space?.path ?? undefined}>
              <span class="ws-summary-icon"><Icon name={t().local ? "laptop" : "monitor"} size={16} /></span>
              <span class="ws-truncate ws-grow">{device()?.name ?? (t().local ? "This computer" : "Unknown device")}</span>
              <Show when={!t().local}>
                <span class="ws-summary-meta">
                  <span class="ws-dot" data-tone={online() ? "success" : "warning"} role="img" aria-label={online() ? "Online" : "Offline"} />
                </span>
              </Show>
            </div>
            {/* The branch; without one, the checkout keeps a row of its own. */}
            <Show
              when={t().git && t().branch}
              fallback={
                <Show when={t().git && t().cwd}>
                  <div class="ws-summary-row" title={t().cwd ?? undefined}>
                    <span class="ws-summary-icon"><Icon name={worktree() ? "worktree" : "folder"} size={16} /></span>
                    <span class="ws-truncate ws-grow">{worktree() ? "Worktree" : "Local checkout"}</span>
                  </div>
                </Show>
              }
            >
              <div class="ws-summary-row" title={t().cwd ?? t().branch ?? undefined}>
                <span class="ws-summary-icon"><Icon name="git-branch" size={16} /></span>
                <span class="ws-truncate ws-grow">{t().branch}</span>
              </div>
            </Show>
            {/* What it changed: the checkout's diff and the branch's pull request. */}
            <Show when={t().git}>
              <button type="button" class="ws-summary-row ws-summary-action" onClick={() => go("changes")}>
                <span class="ws-summary-icon"><Icon name="git-compare" size={16} /></span>
                <span class="ws-truncate ws-grow">Branch changes</span>
                <Show when={stat() && (stat()!.additions || stat()!.deletions)}>
                  <DiffStat additions={stat()!.additions} deletions={stat()!.deletions} />
                </Show>
              </button>
            </Show>
            <Show when={pr()}>
              {(summary) => (
                <button type="button" class="ws-summary-row ws-summary-action" onClick={() => go("pullRequest")}>
                  <span class="ws-summary-icon"><PrStateIcon state={summary().state} draft={!!summary().isDraft} /></span>
                  <span class="ws-truncate ws-grow">
                    #{summary().number} {summary().title}
                  </span>
                </button>
              )}
            </Show>
            {/* What runs for it, under a rule apart from the workspace it runs in. */}
            <Show when={background().length}>
              <div class="ws-fade-rule" />
              <Background chatId={t().chatId} tasks={background()} />
            </Show>
            <Show when={t().space}>
              <div class="ws-fade-rule" />
              <ProjectActions onRun={props.onClose} defaultOpen={actionsOpen()} onOpenChange={setActionsOpen} />
            </Show>
          </div>
        );
      }}
    </Show>
  );
}

/** Background work, folded by default: each running process with its command, its age and a stop control. */
function Background(props: { chatId: string; tasks: BackgroundTask[] }) {
  const [now, setNow] = createSignal(Date.now());
  const timer = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(timer));
  const [stopping, setStopping] = createSignal<string[]>([]);
  const stop = (id: string) => {
    if (stopping().includes(id)) return;
    setStopping((list) => [...list, id]);
    rpc
      .call(M.QUEUE_COMMAND, chatRequest(props.chatId, { command: { kind: "stopBackgroundTask", taskId: id } }))
      .catch(() => setStopping((list) => list.filter((other) => other !== id)));
  };
  const stoppable = () => props.tasks.filter((task) => task.stoppable && !stopping().includes(task.id));
  const label = (task: BackgroundTask) => task.description.trim() || "Background process";
  const command = (task: BackgroundTask) => {
    const text = task.command?.trim();
    return text && text !== label(task) ? text : null;
  };
  return (
    <>
      <button
        type="button"
        class="ws-summary-row ws-summary-action"
        aria-expanded={backgroundOpen()}
        aria-label="Background work"
        onClick={() => setBackgroundOpen((open) => !open)}
      >
        <span class="ws-summary-icon"><Icon name="summary-background" size={16} /></span>
        <span class="ws-truncate ws-grow">Background work</span>
        <span class="ws-summary-meta">
          {props.tasks.length} running
          <Icon name="alt-arrow-down" size={12} class="ws-summary-chevron" />
        </span>
      </button>
      <Show when={backgroundOpen()}>
        <For each={props.tasks}>
          {(task) => (
            <div class="ws-summary-sub ws-summary-task" title={command(task) ? `${label(task)}\n${command(task)}` : label(task)}>
              <span class="ws-summary-task-text">
                <span class="ws-truncate">{label(task)}</span>
                <Show when={command(task)}>{(text) => <span class="ws-truncate ws-mono ws-summary-command">{text()}</span>}</Show>
              </span>
              <span class="ws-summary-meta">
                {elapsed(task.startedAt, now())}
                <Show
                  when={!stopping().includes(task.id)}
                  fallback={<DitherRing size={12} label="Stopping" />}
                >
                  <Show when={task.stoppable} fallback={<span class="ws-summary-live" role="img" aria-label="Running" />}>
                    <button type="button" class="ws-summary-stop" aria-label={`Stop ${label(task)}`} title="Stop" onClick={() => stop(task.id)}>
                      <span />
                    </button>
                  </Show>
                </Show>
              </span>
            </div>
          )}
        </For>
        <Show when={stoppable().length > 1}>
          <div class="ws-summary-sub ws-summary-stop-all">
            <button type="button" onClick={() => stoppable().forEach((task) => stop(task.id))}>Stop all</button>
          </div>
        </Show>
      </Show>
    </>
  );
}
