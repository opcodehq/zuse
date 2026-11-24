import { finishedThreads, threadMatches, threadTone, workerRank, workerStatus } from "./threads-model";
import { ZapperModelPicker } from "./Controls";
import { Icon } from "@/ui/Icon";
import { createMemo, createSignal, For, Show, untrack } from "solid-js";
import { state } from "@/core/store";
import { openChat, view } from "@/core/view";
import { isZapperChat } from "@/core/zappers";
import type { Zapper, ZapperWorker } from "@/core/zapper-types";
import { Button, Dialog, Menu } from "@/ui/primitives";
import {
  act,
  command,
  commandState,
  threadTitle,
  workerVisible,
} from "./state";

function repositoryName(spaceId: string) {
  return state.spaces[spaceId]?.name || state.spaces[spaceId]?.path || "Repository";
}

function issueLabel(zapper: Zapper, id: string) {
  const issue = zapper.issues.find((item) => item.threads.includes(id));
  return issue?.number ? `ZAP-${issue.number}` : "Finished";
}

function followupLabel(count: number) {
  return count === 1 ? "1 follow-up" : `${count} follow-ups`;
}

export function Threads(props: { zapper: Zapper }) {
  const [finishedOpen, setFinishedOpen] = createSignal(false);
  const [mode, setMode] = createSignal<"delegate" | "adopt" | null>(null);
  const [message, setMessage] = createSignal<string | null>(null);
  const [text, setText] = createSignal("");
  const [query, setQuery] = createSignal("");
  const searching = () => query().trim().length > 0;
  const visible = createMemo(() => props.zapper.workers.filter((worker) => workerVisible(worker.chatId)));
  const workers = createMemo(() =>
    visible()
      .filter((worker) =>
        threadMatches(query(), [
          threadTitle(worker.chatId, worker.objective),
          worker.objective,
          repositoryName(worker.spaceId),
          workerStatus(worker),
        ]),
      )
      .sort(
        (a, b) =>
          workerRank(a) - workerRank(b) ||
          (a.queuePosition ?? 0) - (b.queuePosition ?? 0) ||
          a.chatId.localeCompare(b.chatId),
      ),
  );
  const finishedAll = createMemo(() => finishedThreads(props.zapper));
  const finished = createMemo(() =>
    finishedAll().filter((id) => threadMatches(query(), [threadTitle(id), issueLabel(props.zapper, id)])),
  );
  const coordinators = createMemo(() =>
    [props.zapper.coordinatorChatId, ...props.zapper.previousCoordinatorChats]
      .map((id) => ({
        id,
        title: threadTitle(id, id === props.zapper.coordinatorChatId ? "Current coordinator" : "Previous coordinator"),
        meta: id === props.zapper.coordinatorChatId ? "Current" : "Previous",
      }))
      .filter((row) => threadMatches(query(), [row.title, row.meta])),
  );
  const sendMessage = async () => {
    try {
      await command(props.zapper.id, {
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
  const showFinished = () => finishedOpen() || searching();
  return (
    <>
      <div class="zp-section-heading">
        <h3>Worker threads</h3>
        <div>
          <Button size="sm" onClick={() => setMode("adopt")}>
            Adopt thread
          </Button>
          <Button size="sm" variant="primary" onClick={() => setMode("delegate")}>
            Delegate
          </Button>
        </div>
      </div>
      <Show when={visible().length || finishedAll().length || searching()}>
        <label class="zp-search zp-thread-search">
          <Icon name="palette-search" size={14} />
          <input
            aria-label="Search worker threads"
            placeholder="Search threads…"
            value={query()}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
        </label>
        <div class="zp-list-summary">
          <span>
            {workers().length} {workers().length === 1 ? "thread" : "threads"}
          </span>
          <Show when={searching()}>
            <button type="button" class="zp-link" onClick={() => setQuery("")}>
              Clear search
            </button>
          </Show>
        </div>
      </Show>
      <div class="zp-thread-list">
        <For
          each={workers()}
          fallback={
            <Show when={!searching() || (!finished().length && !coordinators().length)}>
              <div class="zp-list-empty">
                <Icon name="chat-round-line" size={24} />
                <strong>{searching() ? "No matching threads" : "No worker threads yet"}</strong>
                <p>
                  {searching()
                    ? "Try another title, repository, or status."
                    : "Delegate a task or adopt an existing project conversation."}
                </p>
              </div>
            </Show>
          }
        >
          {(worker) => (
            <ThreadRow
              zapper={props.zapper}
              worker={worker}
              onMessage={(chatId) => {
                setText("");
                setMessage(chatId);
              }}
            />
          )}
        </For>
      </div>
      <Show when={finished().length}>
        <div class="zp-thread-group">
          <Show
            when={!searching()}
            fallback={<div class="zp-finished-label">Finished · {finished().length}</div>}
          >
            <button
              type="button"
              class="zp-finished-toggle"
              aria-expanded={finishedOpen() ? "true" : "false"}
              onClick={() => setFinishedOpen((open) => !open)}
            >
              <Icon name={finishedOpen() ? "alt-arrow-down" : "alt-arrow-right"} size={12} />
              Finished · {finished().length}
            </button>
          </Show>
          <Show when={showFinished()}>
            <For each={finished()}>
              {(id) => (
                <button type="button" class="zp-thread-quiet" onClick={() => openChat(id)}>
                  <Icon name="chat-round-line" size={13} />
                  <span class="zp-thread-quiet-title">{threadTitle(id)}</span>
                  <span class="zp-muted">{issueLabel(props.zapper, id)}</span>
                </button>
              )}
            </For>
          </Show>
        </div>
      </Show>
      <Show when={coordinators().length}>
        <div class="zp-thread-group">
          <h3>Coordinator conversations</h3>
          <For each={coordinators()}>
            {(row) => (
              <button type="button" class="zp-thread-quiet" onClick={() => openChat(row.id)}>
                <Icon name={row.meta === "Current" ? "chat-round-line" : "history"} size={13} />
                <span class="zp-thread-quiet-title">{row.title}</span>
                <span class="zp-muted">{row.meta}</span>
              </button>
            )}
          </For>
        </div>
      </Show>
      <Show when={mode()}>
        {(value) => (
          <WorkerDialog
            zapper={props.zapper}
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

function ThreadRow(props: { zapper: Zapper; worker: ZapperWorker; onMessage: (chatId: string) => void }) {
  const title = () => threadTitle(props.worker.chatId, props.worker.objective);
  const tone = () => threadTone(props.worker, state.sessions[props.worker.chatId]?.status);
  const objective = () => {
    const text = props.worker.objective.trim();
    return text && text !== title() ? text : "";
  };
  const selected = () => view.selectedChatId === props.worker.chatId;
  return (
    <div class="zp-thread-row" data-selected={selected() ? "true" : undefined} data-tone={tone()}>
      <button
        type="button"
        class="zp-thread-open"
        aria-current={selected() ? "true" : undefined}
        onClick={() => openChat(props.worker.chatId)}
      >
        <span class="zp-thread-mark" data-tone={tone()} aria-hidden="true" />
        <span class="zp-thread-copy">
          <strong>{title()}</strong>
          <span class="zp-thread-meta">
            <span data-tone={tone()}>{workerStatus(props.worker)}</span>
            <span>{repositoryName(props.worker.spaceId)}</span>
            <Show when={props.worker.followups > 0}>
              <span>{followupLabel(props.worker.followups)}</span>
            </Show>
            <Show when={objective()}>
              <span class="zp-thread-objective">{objective()}</span>
            </Show>
          </span>
        </span>
      </button>
      <Menu
        placement="bottom-end"
        trigger={(attrs) => (
          <Button
            {...attrs}
            size="sm"
            variant="ghost"
            class="zp-thread-menu"
            aria-label={`Thread actions for ${title()}`}
          >
            <Icon name="more-horizontal" size={14} />
          </Button>
        )}
        items={[
          { label: "Message thread", icon: "chat-round-line", onSelect: () => props.onMessage(props.worker.chatId) },
          {
            label: "Stop thread",
            icon: "stop",
            onSelect: () => act(props.zapper.id, { type: "stopWorker", chatId: props.worker.chatId }),
          },
          {
            label: "Release to project",
            icon: "arrow-up-right",
            onSelect: () => act(props.zapper.id, { type: "releaseWorker", chatId: props.worker.chatId }),
          },
          {
            label: "Archive thread",
            icon: "archive-minimalistic",
            danger: true,
            onSelect: () => act(props.zapper.id, { type: "archiveWorker", chatId: props.worker.chatId }),
          },
        ]}
      />
    </div>
  );
}

export function WorkerDialog(props: {
  zapper: Zapper;
  mode: "delegate" | "adopt";
  close: () => void;
  issueId?: string;
}) {
  const [repo, setRepo] = createSignal(0);
  const [objective, setObjective] = createSignal(() =>
    untrack(() =>
      props.issueId
        ? (props.zapper.issues.find((i) => i.id === props.issueId)?.details
            .title ?? "")
        : "",
    ),
  );
  const [chat, setChat] = createSignal("");
  const [base, setBase] = createSignal("HEAD");
  const [harness, setHarness] = createSignal(() =>
    untrack(() => props.zapper.config.coordinatorModel.harness),
  );
  const [model, setModel] = createSignal(() =>
    untrack(() => props.zapper.config.coordinatorModel.model ?? ""),
  );
  const repository = () => props.zapper.config.repositories[repo()];
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
    if (!r || !objective().trim() || (props.mode === "adopt" && !chat())) return;
    const worker = {
      chatId: props.mode === "adopt" ? chat() : "",
      deviceId: r.deviceId,
      spaceId: r.spaceId,
      objective: objective().trim(),
      followups: 0,
    };
    try {
      const result = await command(
        props.zapper.id,
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
            <For each={props.zapper.config.repositories}>
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
        <Show when={!props.zapper.config.repositories.length}>
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
          <label>Agent and model<ZapperModelPicker label="Worker model" deviceId={repository()?.deviceId ?? props.zapper.config.workerDeviceId} value={{harness:harness(),model:model() || null}} onChange={value => { setHarness(value.harness); setModel(value.model ?? ""); }} /></label>
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
