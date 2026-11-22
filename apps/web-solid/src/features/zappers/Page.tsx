import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { state } from "@/core/store";
import { navigate, openChat, view } from "@/core/view";
import type { Zapper, ZapperDecision, ZapperAction } from "@/core/zapper-types";
import { Composer } from "@/features/composer";
import { Transcript } from "@/features/transcript";
import { TopBar } from "@/features/workspace";
import { Button, Dialog, Menu } from "@/ui/primitives";
import { Icon } from "@/ui/Icon";
import { Avatar, avatarOf } from "./Avatar";
import { Issues } from "./Issues";
import { Threads } from "./Threads";
import { Drive } from "./Drive";
import {
  act,
  command,
  commandState,
  inspectorOpen,
  loadResources,
  openZapper,
  resources,
  selectedZapper,
  setInspectorOpen,
  setTab,
  statusLabel,
  tab,
  type Tab,
} from "./state";
import { CommandAlert } from "./Alert";
import "./zappers.css";

export function ZapperPage() {
  createEffect(
    () => {
      const z = selectedZapper();
      return z
        ? `${z.id}:${z.revision}:${z.activityRevision}:${state.epoch}`
        : "";
    },
    () => {
      const z = selectedZapper();
      if (z) void loadResources(z);
    },
  );
  return (
    <>
      <CommandAlert />
      <Show when={selectedZapper()} fallback={<Directory />}>
        {(z) => (
          <div class="zp-page">
            <div class="zp-conversation">
              <div class="zp-context">
                <button class="zp-context-name" onClick={() => openZapper(z())}>
                  <Avatar avatar={avatarOf(z())} size={24} />
                  <strong>{z().config.name}</strong>
                  <span class="zp-muted">
                    {view.selectedChatId === z().coordinatorChatId
                      ? "Coordinator"
                      : "Thread"}
                  </span>
                </button>
                <Button
                  size="sm"
                  onClick={() => setInspectorOpen(!inspectorOpen())}
                >
                  {inspectorOpen() ? "Hide Zapper" : "Show Zapper"}
                </Button>
              </div>
              <TopBar />
              <div class="app-chat">
                <Transcript />
                <Composer />
              </div>
            </div>
            <Show when={inspectorOpen() && !view.rightPanel}>
              <Inspector zapper={z()} />
            </Show>
          </div>
        )}
      </Show>
    </>
  );
}
function Directory() {
  const [archived, setArchived] = createSignal(false);
  return (
    <div class="zp-directory">
      <div class="zp-section-heading">
        <h1>Zappers</h1>
        <Button
          variant="primary"
          onClick={() => navigate({ kind: "newZapper" })}
        >
          New Zapper
        </Button>
      </div>
      <p class="zp-muted">
        Captains for your projects, with their own conversations, threads,
        issues and shared notes.
      </p>
      <label class="zp-check">
        <input
          type="checkbox"
          checked={archived()}
          onChange={(e) => setArchived(e.currentTarget.checked)}
        />
        Show archived Zappers
      </label>
      <Show when={!state.zappersReady}>
        <p>Loading Zappers…</p>
      </Show>
      <Show when={state.zappersError}>
        <p role="alert">{state.zappersError}</p>
      </Show>
      <div class="zp-directory-grid">
        <For
          each={state.zappers.filter(
            (z) => archived() || z.status !== "archived",
          )}
        >
          {(z) => (
            <button
              class="zp-card zp-directory-card"
              onClick={() => openZapper(z)}
            >
              <Avatar avatar={avatarOf(z)} size={42} />
              <h2>{z.config.name}</h2>
              <p>{z.config.purpose || "Your project captain"}</p>
              <span class="zp-muted">
                {statusLabel(z.status)} · {z.workers.length} threads ·{" "}
                {z.issues.length} issues
              </span>
            </button>
          )}
        </For>
      </div>
      <Show when={state.zappersReady && !state.zappers.length}>
        <div class="zp-empty">
          Create a Zapper to start coordinating work across your projects.
        </div>
      </Show>
    </div>
  );
}
function Inspector(props: { zapper: Zapper }) {
  const [confirm, setConfirm] = createSignal<"delete" | "archive" | null>(null);
  const files = createMemo(() =>
    resources.files.filter(
      (f) => f.path !== "INSTRUCTIONS.md" && f.content.trim(),
    ),
  );
  const tabs = createMemo(
    () =>
      [
        "overview",
        "issues",
        "threads",
        ...(files().length || tab() === "drive" ? ["drive"] : []),
        ...(resources.files.some((f) => f.version > 1) || tab() === "history"
          ? ["history"]
          : []),
      ] as Tab[],
  );
  const fresh = async () => {
    try {
      const z = await command(props.zapper.id, { type: "freshConversation" });
      if (z) {
        await Promise.resolve();
        openZapper(z);
      }
    } catch {
      /* shared alert */
    }
  };
  const finish = async () => {
    const action: ZapperAction =
      confirm() === "delete"
        ? { type: "delete" }
        : { type: "setStatus", status: "archived" };
    try {
      await command(props.zapper.id, action);
      setConfirm(null);
      if (action.type === "delete") navigate({ kind: "zappers", zapperId: "" });
    } catch {
      /* shared alert */
    }
  };
  return (
    <aside class="zp-inspector" aria-label="Zapper inspector">
      <header class="zp-inspector-header">
        <Avatar avatar={avatarOf(props.zapper)} size={38} />
        <div>
          <h2>{props.zapper.config.name}</h2>
          <span class="zp-muted">{statusLabel(props.zapper.status)}</span>
        </div>
        <Menu
          trigger={(attrs) => (
            <Button {...attrs} aria-label="Zapper actions">
              •••
            </Button>
          )}
          items={[
            {
              label: "Zapper settings",
              onSelect: () =>
                navigate({ kind: "newZapper", zapperId: props.zapper.id }),
            },
            { label: "Fresh conversation", onSelect: () => void fresh() },
            {
              label: props.zapper.status === "active" ? "Pause" : "Resume",
              onSelect: () =>
                act(props.zapper.id, {
                  type: "setStatus",
                  status: props.zapper.status === "active" ? "paused" : "active",
                }),
            },
            {
              label: "Stop Zapper",
              onSelect: () =>
                act(props.zapper.id, { type: "setStatus", status: "stopped" }),
            },
            { label: "Archive Zapper", onSelect: () => setConfirm("archive") },
            {
              label: "Delete Zapper",
              danger: true,
              onSelect: () => setConfirm("delete"),
            },
          ]}
        />
      </header>
      <nav class="zp-tabs" aria-label="Zapper sections">
        <For each={tabs()}>
          {(name) => (
            <button
              aria-label={name[0]!.toUpperCase() + name.slice(1)}
              title={name[0]!.toUpperCase() + name.slice(1)}
              aria-current={tab() === name ? "page" : undefined}
              onClick={() => setTab(name)}
            >
              <Icon
                name={
                  {
                    overview: "list",
                    issues: "clock-circle",
                    threads: "chat-round-line",
                    drive: "folder-open",
                    history: "history",
                  }[name]
                }
                size={13}
              />
              <span class="zp-tab-label">
                <span>{name[0]!.toUpperCase() + name.slice(1)}</span>
              </span>
            </button>
          )}
        </For>
      </nav>
      <div class="zp-inspector-body">
        <Show when={tab() === "overview"}>
          <Overview zapper={props.zapper} />
        </Show>
        <Show when={tab() === "issues"}>
          <Issues zapper={props.zapper} />
        </Show>
        <Show when={tab() === "threads"}>
          <Threads zapper={props.zapper} />
        </Show>
        <Show when={tab() === "drive" || tab() === "history"}>
          <Drive zapper={props.zapper} history={tab() === "history"} />
        </Show>
      </div>
      <Dialog
        open={!!confirm()}
        onOpenChange={(open) => {
          if (!open) setConfirm(null);
        }}
        title={confirm() === "delete" ? "Delete Zapper?" : "Archive Zapper?"}
        description={
          confirm() === "delete"
            ? "Its conversations and files remain available. The Zapper will no longer coordinate work."
            : "The Zapper and all its worker threads will be archived."
        }
        footer={
          <>
            <Button onClick={() => setConfirm(null)}>Cancel</Button>
            <Button
              variant="danger"
              disabled={commandState.busy}
              onClick={() => void finish()}
            >
              Confirm
            </Button>
          </>
        }
      />
    </aside>
  );
}
function Overview(props: { zapper: Zapper }) {
  return (
    <>
      <p class="zp-purpose">
        {props.zapper.config.purpose || "Add a purpose in Zapper settings."}
      </p>
      <div class="zp-stats">
        <button onClick={() => setTab("threads")}>
          <strong>{props.zapper.workers.length}</strong>Threads
        </button>
        <button onClick={() => setTab("issues")}>
          <strong>
            {
              props.zapper.issues.filter(
                (i) => !["done", "canceled"].includes(i.details.status),
              ).length
            }
          </strong>
          Open issues
        </button>
        <span>
          <strong>
            {props.zapper.decisions.filter((d) => d.answer === null).length}
          </strong>
          Decisions
        </span>
      </div>
      <h3>Needs your decision</h3>
      <For
        each={props.zapper.decisions.filter((d) => d.answer === null)}
        fallback={<p class="zp-muted">No decisions waiting for you.</p>}
      >
        {(decision) => <Decision zapper={props.zapper} decision={decision} />}
      </For>
      <div class="zp-section-heading">
        <h3>Issues</h3>
        <Button size="sm" onClick={() => setTab("issues")}>
          View all
        </Button>
      </div>
      <Issues zapper={props.zapper} compact />
      <div class="zp-section-heading">
        <h3>Drive</h3>
        <Button size="sm" onClick={() => setTab("drive")}>
          Open Drive
        </Button>
      </div>
      <p class="zp-muted">
        Shared notes and instructions persist across coordinator conversations.
      </p>
      <h3>Recent activity</h3>
      <Show when={resources.error}>
        <p class="zp-error">
          {resources.error}
          <Button onClick={() => void loadResources(props.zapper)}>Retry</Button>
        </p>
      </Show>
      <For
        each={resources.events.slice(-12).reverse()}
        fallback={<p class="zp-muted">No activity yet.</p>}
      >
        {(event) => (
          <div class="zp-activity">
            <span>{event.kind}</span>
            <p>{event.summary}</p>
            <Show when={event.workerChatId}>
              <button onClick={() => openChat(event.workerChatId)}>
                Open thread
              </button>
            </Show>
          </div>
        )}
      </For>
    </>
  );
}
function Decision(props: { zapper: Zapper; decision: ZapperDecision }) {
  const [answer, setAnswer] = createSignal("");
  return (
    <form
      class="zp-card zp-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (answer().trim())
          act(props.zapper.id, {
            type: "decide",
            decision: { ...props.decision, answer: answer().trim() },
          });
      }}
    >
      <p>{props.decision.question}</p>
      <Show when={props.decision.workerChatId}>
        <button
          type="button"
          onClick={() => openChat(props.decision.workerChatId)}
        >
          Open related thread
        </button>
      </Show>
      <label>
        Your answer
        <textarea
          required
          value={answer()}
          onInput={(e) => setAnswer(e.currentTarget.value)}
        />
      </label>
      <Button type="submit" disabled={commandState.busy}>
        Send decision
      </Button>
    </form>
  );
}
