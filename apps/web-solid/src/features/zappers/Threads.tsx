import { createMemo, createSignal, For, Show, untrack } from "solid-js";
import { state } from "@/core/store";
import { openChat } from "@/core/view";
import { isZapperChat } from "@/core/zappers";
import type { Zapper } from "@/core/zapper-types";
import { Button, Dialog, Menu } from "@/ui/primitives";
import {
  act,
  command,
  commandState,
  statusLabel,
  threadTitle,
  workerVisible,
} from "./state";

export function Threads(props: { zot: Zapper }) {
  const [mode, setMode] = createSignal<"delegate" | "adopt" | null>(null);
  const [message, setMessage] = createSignal<string | null>(null);
  const [text, setText] = createSignal("");
  const sendMessage = async () => {
    try {
      await command(props.zot.id, {
        type: "messageWorker",
        chatId: message()!,
        text: text().trim(),
        automatic: false,
      });
      setMessage(null);
    } catch {
      /* shared alert */
    }
  };
  return (
    <>
      <div class="zp-section-heading">
        <h3>Worker threads</h3>
        <div>
          <Button size="sm" onClick={() => setMode("adopt")}>
            Adopt thread
          </Button>
          <Button
            size="sm"
            variant="primary"
            onClick={() => setMode("delegate")}
          >
            Delegate
          </Button>
        </div>
      </div>
      <For
        each={props.zot.workers.filter((w) => workerVisible(w.chatId))}
        fallback={
          <p class="zp-muted">
            Delegate a task or adopt an existing project conversation.
          </p>
        }
      >
        {(worker) => (
          <div class="zp-card zp-thread">
            <div class="zp-section-heading">
              <button class="zp-link" onClick={() => openChat(worker.chatId)}>
                {threadTitle(worker.chatId, worker.objective)}
              </button>
              <Menu
                trigger={(attrs) => (
                  <Button
                    {...attrs}
                    size="sm"
                    aria-label={`Thread actions for ${threadTitle(worker.chatId, worker.objective)}`}
                  >
                    •••
                  </Button>
                )}
                items={[
                  {
                    label: "Message thread",
                    onSelect: () => {
                      setText("");
                      setMessage(worker.chatId);
                    },
                  },
                  {
                    label: "Stop thread",
                    onSelect: () =>
                      act(props.zot.id, {
                        type: "stopWorker",
                        chatId: worker.chatId,
                      }),
                  },
                  {
                    label: "Archive thread",
                    onSelect: () =>
                      act(props.zot.id, {
                        type: "archiveWorker",
                        chatId: worker.chatId,
                      }),
                  },
                  {
                    label: "Release to project",
                    onSelect: () =>
                      act(props.zot.id, {
                        type: "releaseWorker",
                        chatId: worker.chatId,
                      }),
                  },
                ]}
              />
            </div>
            <p>{worker.objective}</p>
            <small class="zp-muted">
              {statusLabel(state.sessions[worker.chatId]?.status)} ·{" "}
              {state.spaces[worker.spaceId]?.name ||
                state.spaces[worker.spaceId]?.path ||
                "Repository"}{" "}
              · {worker.followups} follow-ups
            </small>
          </div>
        )}
      </For>
      <h3>Coordinator conversations</h3>
      <For
        each={[
          props.zot.coordinatorChatId,
          ...props.zot.previousCoordinatorChats,
        ]}
      >
        {(id) => (
          <button class="zp-history-row" onClick={() => openChat(id)}>
            {threadTitle(
              id,
              id === props.zot.coordinatorChatId
                ? "Current coordinator"
                : "Previous coordinator",
            )}
            <span class="zp-muted">
              {id === props.zot.coordinatorChatId ? "Current" : "Previous"}
            </span>
          </button>
        )}
      </For>
      <Show when={mode()}>
        {(value) => (
          <WorkerDialog
            zot={props.zot}
            mode={value()}
            close={() => setMode(null)}
          />
        )}
      </Show>
      <Dialog
        open={!!message()}
        onOpenChange={(open) => {
          if (!open) setMessage(null);
        }}
        title="Message worker"
        footer={
          <Button
            disabled={commandState.busy || !text().trim()}
            onClick={() => void sendMessage()}
          >
            Send
          </Button>
        }
      >
        <Show when={commandState.error}>
          <p class="zp-error" role="alert">
            {commandState.error}
          </p>
        </Show>
        <label class="zp-form">
          Message
          <textarea
            value={text()}
            onInput={(e) => setText(e.currentTarget.value)}
          />
        </label>
      </Dialog>
    </>
  );
}
export function WorkerDialog(props: {
  zot: Zapper;
  mode: "delegate" | "adopt";
  close: () => void;
  issueId?: string;
}) {
  const [repo, setRepo] = createSignal(0);
  const [objective, setObjective] = createSignal(() =>
    untrack(() =>
      props.issueId
        ? (props.zot.issues.find((i) => i.id === props.issueId)?.details
            .title ?? "")
        : "",
    ),
  );
  const [chat, setChat] = createSignal("");
  const [base, setBase] = createSignal("HEAD");
  const [harness, setHarness] = createSignal(() =>
    untrack(() => props.zot.config.coordinatorModel.harness),
  );
  const [model, setModel] = createSignal(() =>
    untrack(() => props.zot.config.coordinatorModel.model ?? ""),
  );
  const repository = () => props.zot.config.repositories[repo()];
  const candidates = createMemo(() =>
    Object.values(state.chats).filter(
      (c) =>
        !c.archived &&
        !c.parentChatId &&
        !isZapperChat(c.id) &&
        c.spaceId === repository()?.spaceId &&
        c.deviceId === repository()?.deviceId,
    ),
  );
  const save = async (event: SubmitEvent) => {
    event.preventDefault();
    const r = repository();
    if (!r || !objective().trim()) return;
    const worker = {
      chatId: props.mode === "adopt" ? chat() : "",
      deviceId: r.deviceId,
      spaceId: r.spaceId,
      objective: objective().trim(),
      followups: 0,
    };
    try {
      const result = await command(
        props.zot.id,
        props.mode === "adopt"
          ? { type: "adoptWorker", worker }
          : {
              type: "delegate",
              worker,
              issueId: props.issueId,
              model: { harness: harness(), model: model().trim() || null },
              base: base(),
            },
      );
      props.close();
      if (result) {
        await Promise.resolve();
        const added = result.workers.find(
          (w) =>
            w.objective === worker.objective &&
            (props.mode !== "adopt" || w.chatId === chat()),
        );
        if (added) openChat(added.chatId);
      }
    } catch {
      /* shared alert */
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) props.close();
      }}
      title={props.mode === "adopt" ? "Adopt a thread" : "Delegate a task"}
    >
      <form class="zp-form" onSubmit={save}>
        <label>
          Repository
          <select
            required
            value={repo()}
            onChange={(e) => {
              setRepo(Number(e.currentTarget.value));
              setChat("");
            }}
          >
            <For each={props.zot.config.repositories}>
              {(r, i) => (
                <option value={i()}>
                  {state.spaces[r.spaceId]?.name ||
                    state.spaces[r.spaceId]?.path ||
                    r.spaceId}{" "}
                  · {state.devices[r.deviceId]?.name || "This computer"}
                </option>
              )}
            </For>
          </select>
        </label>
        <Show when={!props.zot.config.repositories.length}>
          <p>Add a repository in Zapper settings before delegating work.</p>
        </Show>
        <Show when={props.mode === "adopt"}>
          <label>
            Existing thread
            <select
              required
              value={chat()}
              onChange={(e) => setChat(e.currentTarget.value)}
            >
              <option value="">Choose a thread</option>
              <For each={candidates()}>
                {(c) => (
                  <option value={c.id}>{c.title || "Untitled thread"}</option>
                )}
              </For>
            </select>
          </label>
        </Show>
        <label>
          Objective
          <textarea
            required
            value={objective()}
            onInput={(e) => setObjective(e.currentTarget.value)}
          />
        </label>
        <Show when={props.mode === "delegate"}>
          <label>
            Agent
            <select
              value={harness()}
              onChange={(e) => setHarness(e.currentTarget.value)}
            >
              <For each={state.harnesses}>
                {(h) => <option value={h.id}>{h.name || h.id}</option>}
              </For>
            </select>
          </label>
          <label>
            Model
            <input
              placeholder="Agent default"
              value={model()}
              onInput={(e) => setModel(e.currentTarget.value)}
            />
          </label>
          <label>
            Base branch or commit
            <input
              required
              value={base()}
              onInput={(e) => setBase(e.currentTarget.value)}
            />
          </label>
        </Show>
        <Show when={commandState.error}>
          <p class="zp-error" role="alert">
            {commandState.error}
          </p>
        </Show>
        <div class="zp-form-actions">
          <Button onClick={props.close}>Cancel</Button>
          <Button
            type="submit"
            variant="primary"
            disabled={commandState.busy || !repository()}
          >
            {props.mode === "adopt" ? "Adopt thread" : "Delegate task"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
