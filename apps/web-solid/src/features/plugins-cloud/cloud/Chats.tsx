import { ModelSelect } from "@/ui/ModelSelect";
import { Menu } from "@/ui/primitives";
import { openCloudWorkspace } from "@/features/connection/cloud";
// Starting cloud chats, their first-message queue, and the account's live workspaces (crates/viewport/src/cloud.rs, cloud_queue.rs).
import { navigate, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { createEffect, createMemo, createSignal, For, onSettled, Show } from "solid-js";
import { owner } from "../account";
import { Action, Badge, Group, Row, type BadgeTone } from "../ui";
import { archiveVisible } from "./Archives";
import { connectionMethods, connectionName, launchConnections } from "./connections";
import { cloud, dismissLaunch, launch, loadContext, loadWorkspaces, openHistory, pauseWorkspace, workspaceLabel, type Launch, type LaunchDraft } from "./state";
import type { CloudContextProject, Workspace } from "./types";

const MODES: { value: LaunchDraft["runtimeMode"]; label: string }[] = [
  { value: "approval-required", label: "Ask for approval" },
  { value: "auto", label: "Approve for me" },
  { value: "full-access", label: "Full access" },
];

export function Chats() {
  onSettled(() => {
    void loadContext();
    if (!cloud.workspaces.loaded) void loadWorkspaces();
  });
  return (
    <div class="pc-stack">
      <Show when={!owner()?.organizationId} fallback={<Row title="Cloud chats start from your personal account" description="Switch to your personal account to start or open cloud chats." />}>
        <NewChat />
        <Show when={cloud.launches.length > 0}>
          <div class="pc-queue" aria-label="Starting cloud chats">
            <For each={cloud.launches}>{(item) => <Queued launch={item} />}</For>
          </div>
        </Show>
        <Workspaces />
      </Show>
    </div>
  );
}

function NewChat() {
  const [projectId, setProjectId] = createSignal("");
  const [modelKey, setModelKey] = createSignal("");
  const [connectionId, setConnectionId] = createSignal("");
  const [mode, setMode] = createSignal<LaunchDraft["runtimeMode"]>(view.preferences.defaultPermissionMode);
  const [branch, setBranch] = createSignal("");
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const prepared = (p: CloudContextProject) => p.prepared !== false;
  const projects = () => cloud.context.projects;
  const project = createMemo(() => projects().find((p) => p.projectId === projectId()) ?? null);
  // Pick the first usable choices once the context arrives; the user's picks are kept after that.
  createEffect(() => [project(), projects().find(prepared), modelKey(), cloud.context.agents[0]?.id, cloud.context.agents[0]?.defaultModel, cloud.context.agents[0]?.models[0]?.id], () => {
    if (!project()) {
      const first = projects().find(prepared);
      if (first) setProjectId(first.projectId);
    }
    if (!modelKey()) {
      const agent = cloud.context.agents[0];
      if (agent) setModelKey(`${agent.id}\u0000${agent.defaultModel || agent.models[0]?.id || ""}`);
    }
  });
  const selection = () => {
    const [agent, model] = modelKey().split("\u0000");
    const provider = cloud.context.agents.find((a) => a.id === agent);
    return provider && model && provider.models.some((m) => m.id === model) ? { agent: provider.id, model } : null;
  };
  const connections = createMemo(() => launchConnections(cloud.context.connections ?? [], selection()?.agent));
  const connection = () => connectionId() ? connections().find((row) => row.connectionId === connectionId()) : connections()[0];
  const connectionLabel = () => {
    const current = connection();
    return current ? connectionName(current) : "Choose an account";
  };
  const submit = (event: Event) => {
    event.preventDefault();
    const chosen = project();
    if (!chosen || !prepared(chosen)) return setError("Select a prepared cloud repository before starting.");
    const picked = selection();
    if (!picked) return setError("Choose an available Cloud model before starting. Your draft is preserved.");
    const auth = connection();
    if (cloud.context.connections !== null && !auth) return setError("Choose a connected account for this model before starting.");
    if (!text().trim()) return setError("Write the first message for this cloud chat.");
    setError(null);
    void launch({ project: chosen, agent: picked.agent, agentConnectionId: auth?.connectionId, model: picked.model, branch: branch(), runtimeMode: mode(), text: text().trim() });
    setText("");
    setBranch("");
  };
  return (
    <Group title="New cloud chat" help="Cloud chats run in a private workspace built from your cloud image and keep working when this computer is off.">
      <Show
        when={cloud.context.loaded && !cloud.context.loading ? true : null}
        fallback={<Row title="Loading cloud repositories…" description="Reading prepared repositories and available agents." />}
      >
        <Show
          when={projects().length > 0}
          fallback={
            <Row
              title={cloud.context.error ? "Cloud repositories are unavailable" : "No prepared repositories"}
              description={cloud.context.error ?? "Add a repository and build the cloud image in Setup, then start a cloud chat here."}
              action={<Action onClick={() => void loadContext()}>Refresh</Action>}
            />
          }
        >
          <form class="pc-launch" onSubmit={submit}>
            <div class="pc-launch-pickers">
              <label class="pc-field">
                <span>Repository</span>
                <Menu placement="bottom-start" trigger={attrs => <button {...attrs} type="button" class="model-select-trigger"><Icon name="folder" size={14} /><span>{project()?.displayName ?? project()?.name ?? "Choose a repository"}</span><Icon name="alt-arrow-down" size={12} /></button>} items={projects().map(row => ({ label: (row.displayName ?? row.name ?? row.projectId) + (prepared(row) ? "" : " · image not built"), icon: "folder", disabled: !prepared(row), onSelect: () => setProjectId(row.projectId) }))} />
              </label>
              <label class="pc-field">
                <span>Model</span>
                <ModelSelect label="Model" value={modelKey()} onChange={(value) => {
                  if (value.split("\u0000")[0] !== modelKey().split("\u0000")[0]) setConnectionId("");
                  setModelKey(value);
                }} options={cloud.context.agents.flatMap(agent => agent.models.map(model => ({ key: `${agent.id}\u0000${model.id}`, label: model.label ?? model.name ?? model.id, provider: agent.id, providerLabel: agent.name })))} />
              </label>
              <Show when={cloud.context.connections !== null}>
                <div class="pc-field">
                  <span>Connection</span>
                  <Show when={connections().length} fallback={<Action onClick={() => navigate({ kind: "settings", section: "connections" })}>Manage connections</Action>}>
                    <Menu placement="bottom-start" trigger={attrs => <button {...attrs} type="button" class="model-select-trigger" aria-label={`Connection: ${connectionLabel()}`}><Icon name="link" size={14} /><span>{connectionLabel()}</span><Icon name="alt-arrow-down" size={12} /></button>} items={connections().map(row => ({ label: `${connectionName(row)} · ${connectionMethods[row.method]}`, icon: row.connectionId === connection()?.connectionId ? "check" : undefined, onSelect: () => { setConnectionId(row.connectionId); setError(null); } }))} />
                  </Show>
                </div>
              </Show>
              <label class="pc-field">
                <span>Permissions</span>
                <Menu placement="bottom-start" trigger={attrs => <button {...attrs} type="button" class="model-select-trigger"><Icon name={mode() === "full-access" ? "access-full" : mode() === "auto" ? "access-auto" : "access-ask"} size={14} /><span>{MODES.find(item => item.value === mode())?.label}</span><Icon name="alt-arrow-down" size={12} /></button>} items={MODES.map(item => ({ label: item.label, icon: item.value === "full-access" ? "access-full" : item.value === "auto" ? "access-auto" : "access-ask", onSelect: () => setMode(item.value) }))} />
              </label>
              <label class="pc-field">
                <span>Branch</span>
                <input class="field pc-mono" value={branch()} placeholder={`New branch from ${project()?.defaultBranch ?? "main"}`} onInput={(e) => setBranch(e.currentTarget.value)} />
              </label>
            </div>
            <div class="pc-launch-compose">
              <textarea
                class="pc-launch-input"
                rows={3}
                aria-label="First message"
                placeholder="Describe the task for the cloud agent"
                value={text()}
                onInput={(e) => {
                  setText(e.currentTarget.value);
                  setError(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
                    e.preventDefault();
                    e.currentTarget.form?.requestSubmit();
                  }
                }}
              />
              <button type="submit" class="pc-launch-send" aria-label="Start cloud chat" title="Start cloud chat" disabled={!text().trim()}>
                <Icon name="arrow-up" size={16} />
              </button>
            </div>
            <Show when={error() ?? cloud.context.error}>
              {(message) => (
                <div class="pc-form-error" role="alert">
                  {message()}
                </div>
              )}
            </Show>
          </form>
        </Show>
      </Show>
    </Group>
  );
}

/** The queued first message stays visible, with its status, until the cloud accepts it. */
function Queued(props: { launch: Launch }) {
  const busy = () => props.launch.state === "sending";
  const retry = () => props.launch.state === "failed" || props.launch.state === "uncertain";
  return (
    <div class="pc-queue-item" data-state={props.launch.state}>
      <div class="pc-queue-status" role="status">
        <Show when={busy()} fallback={<Icon name={retry() ? "danger-triangle" : "cloud"} size={12} />}>
          <GlyphSpinner cell={1.5} box={12} decorative />
        </Show>
        <span class="pc-truncate pc-grow">{props.launch.message ?? "Preparing cloud workspace…"}</span>
        <span class="pc-muted pc-truncate">{props.launch.repository}</span>
      </div>
      <div class="pc-queue-prompt">{props.launch.text}</div>
      <div class="pc-queue-actions">
        <Show when={props.launch.workspaceId}>
          {(id) => <Action onClick={() => void openHistory(id())}>View history</Action>}
        </Show>
        <Show when={retry()}>
          <Action primary onClick={() => void launch(null, props.launch.key)}>
            Retry
          </Action>
        </Show>
        <Show when={!busy()}>
          <Action onClick={() => dismissLaunch(props.launch.key)}>Dismiss</Action>
        </Show>
      </div>
    </div>
  );
}

function stateBadge(row: Workspace): { tone: BadgeTone; label: string } {
  switch (row.state) {
    case "ready":
    case "running":
      return { tone: "ok", label: "Running" };
    case "paused":
      return { tone: "outline", label: "Paused" };
    case "failed":
    case "blocked":
      return { tone: "error", label: row.state === "failed" ? "Failed" : "Blocked" };
    case "pausing":
      return { tone: "warn", label: "Pausing" };
    case "starting":
    case "provisioning":
    case "creating":
      return { tone: "warn", label: "Starting" };
    default:
      return { tone: "outline", label: row.state || "Unknown" };
  }
}

function Workspaces() {
  const rows = createMemo(() => cloud.workspaces.rows.filter((row) => row.state !== "deleted" && !archiveVisible(row)));
  return (
    <Group
      title="Cloud chats"
      help="Workspaces keep their files and history. Paused workspaces resume when you open them in a connected app."
      action={
        <Action icon="refresh" busy={cloud.workspaces.loading} onClick={() => void loadWorkspaces()}>
          Refresh
        </Action>
      }
    >
      <Show when={cloud.workspaces.error}>
        {(error) => (
          <div class="pc-inline-alert" role="alert">
            {error()}
          </div>
        )}
      </Show>
      <Show
        when={rows().length > 0}
        fallback={<Row title={cloud.workspaces.loading ? "Loading cloud chats…" : "No cloud chats yet"} description="Start a cloud chat above and its workspace will appear here." />}
      >
        <For each={rows()}>
          {(row) => {
            const badge = () => stateBadge(row);
            const pending = () => cloud.workspaces.pending[row.workspaceId];
            const canPause = () => ["ready", "running", "starting"].includes(row.state);
            return (
              <Row
                title={
                  <span class="pc-ws-title">
                    <Icon name="cloud" size={13} />
                    <span class="pc-truncate">{workspaceLabel(row)}</span>
                  </span>
                }
                description={
                  <>
                    {row.branch ? `Branch ${row.branch}` : "Cloud workspace"}
                    <Show when={cloud.workspaces.rowError?.id === row.workspaceId}>
                      <span class="pc-error-text"> · {cloud.workspaces.rowError?.message}</span>
                    </Show>
                  </>
                }
                action={
                  <>
                    <Badge tone={badge().tone}>{badge().label}</Badge>
                    <Show when={!archiveVisible(row) && !["failed", "blocked", "deleting"].includes(row.state)}>
                      <Action onClick={() => { const account = owner(); if (account) openCloudWorkspace({ workspaceId: row.workspaceId, accountId: account.accountId, organizationId: account.organizationId }); }}>Open workspace</Action>
                    </Show>
                    <Action disabled={cloud.history.loading} onClick={() => void openHistory(row.workspaceId, row.initialSessionId || null)}>
                      View history
                    </Action>
                    <Show when={canPause()}>
                      <Action busy={pending() === "pause"} disabled={!!pending()} onClick={() => void pauseWorkspace(row.workspaceId)}>
                        Pause
                      </Action>
                    </Show>
                  </>
                }
              />
            );
          }}
        </For>
      </Show>
    </Group>
  );
}
