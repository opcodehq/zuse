// Saved cloud history, read without opening an execution channel (crates/viewport/src/cloud_history.rs).
import { createVirtualizer } from "@tanstack/solid-virtual";
import { createMemo, For, Match, Show, Switch } from "solid-js";
import type { MessagePart, SessionMessageEntry } from "@/core/protocol";
import { Markdown } from "@/features/transcript/Markdown";
import { Icon } from "@/ui/Icon";
import { Action } from "../ui";
import { closeHistory, cloud, isArchived, openHistory, pauseWorkspace, restoreWorkspace, workspaceLabel } from "./state";

const visible = (part: MessagePart) => part.kind === "text" || part.kind === "tool" || part.kind === "error" || part.kind === "image" || part.kind === "reasoning" || part.kind === "visual";

export function History() {
  const head = () => cloud.history.head;
  const row = () => cloud.workspaces.rows.find((r) => r.workspaceId === cloud.history.workspaceId) ?? null;
  const entries = createMemo<SessionMessageEntry[]>(() => (head()?.transcript.reset ?? []).filter((entry) => entry.parts.some(visible)));
  const title = () => head()?.chat.title?.trim() || (row() ? workspaceLabel(row()!) : "") || "Cloud conversation";
  const busy = () => cloud.history.loading || (!!cloud.history.workspaceId && !!cloud.workspaces.pending[cloud.history.workspaceId]);
  const archived = () => !!row() && isArchived(row()!);
  const running = () => !!row() && ["ready", "running", "starting"].includes(row()!.state) && !archived();
  let scroller!: HTMLDivElement;
  const virtualizer = createVirtualizer({
    get count() {
      return entries().length;
    },
    getScrollElement: () => scroller,
    estimateSize: () => 96,
    overscan: 6,
    getItemKey: (index) => entries()[index]?.id ?? index,
  });
  return (
    <div class="pc-history">
      <div class="pc-history-bar">
        <Action icon="alt-arrow-left" onClick={closeHistory}>
          Back
        </Action>
        <h2 class="pc-truncate pc-grow">{title()}</h2>
        <span class="pc-muted">Cloud</span>
      </div>
      <Show when={(head()?.conversations.length ?? 0) > 1}>
        <div class="pc-history-sessions" role="tablist" aria-label="Conversations">
          <For each={head()?.conversations ?? []}>
            {(chat, index) => (
              <button
                type="button"
                role="tab"
                class="pc-history-session"
                aria-selected={chat.id === head()?.sessionId}
                disabled={busy()}
                onClick={() => chat.id !== head()?.sessionId && cloud.history.workspaceId && void openHistory(cloud.history.workspaceId, chat.id)}
              >
                <span class="pc-truncate">{chat.title?.trim() || `Conversation ${index() + 1}`}</span>
              </button>
            )}
          </For>
        </div>
      </Show>
      <div class="pc-history-scroll" ref={scroller} aria-busy={cloud.history.loading}>
        <Switch>
          <Match when={cloud.history.loading && !head()}>
            <p class="pc-history-empty">Loading saved conversation…</p>
          </Match>
          <Match when={cloud.history.error && !head()}>
            <p class="pc-history-empty pc-error-text" role="alert">
              {cloud.history.error}
            </p>
          </Match>
          <Match when={entries().length === 0}>
            <p class="pc-history-empty">{cloud.history.loading ? "Loading saved conversation…" : "This conversation has no saved messages yet."}</p>
          </Match>
          <Match when={true}>
            <div class="pc-history-list" style={{ height: `${virtualizer.getTotalSize()}px` }} role="log" aria-label="Saved messages">
              <For each={virtualizer.getVirtualItems()}>
                {(item) => (
                  <div class="pc-history-item" data-index={item.index} ref={(el) => queueMicrotask(() => virtualizer.measureElement(el))} style={{ transform: `translateY(${item.start}px)` }}>
                    <Show when={entries()[item.index]}>{(entry) => <Entry entry={entry()} />}</Show>
                  </div>
                )}
              </For>
            </div>
          </Match>
        </Switch>
      </div>
      <div class="pc-history-foot">
        <Icon name={archived() ? "archive-minimalistic" : "history"} size={16} />
        <div class="pc-grow pc-history-foot-copy">
          <span>{archived() ? "This chat is archived." : "Saved history. Messages are read-only here."}</span>
          <Show when={cloud.history.error && head()}>
            <span class="pc-error-text">{cloud.history.error}</span>
          </Show>
          <Show when={cloud.workspaces.rowError?.id === cloud.history.workspaceId}>
            <span class="pc-error-text">{cloud.workspaces.rowError?.message}</span>
          </Show>
        </div>
        <Show when={archived()}>
          <Action
            busy={cloud.workspaces.pending[cloud.history.workspaceId ?? ""] === "restore"}
            disabled={busy() || row()?.state !== "archived"}
            onClick={() => cloud.history.workspaceId && void restoreWorkspace(cloud.history.workspaceId)}
          >
            Restore chat
          </Action>
        </Show>
        <Show when={running()}>
          <Action
            busy={cloud.workspaces.pending[cloud.history.workspaceId ?? ""] === "pause"}
            disabled={busy()}
            onClick={() => cloud.history.workspaceId && void pauseWorkspace(cloud.history.workspaceId)}
          >
            Pause workspace
          </Action>
        </Show>
      </div>
    </div>
  );
}

function Entry(props: { entry: SessionMessageEntry }) {
  const user = () => props.entry.role === "user";
  return (
    <article class="pc-msg" data-role={props.entry.role} aria-label={user() ? "You" : props.entry.role === "assistant" ? "Agent" : "System"}>
      <For each={props.entry.parts.filter(visible)}>
        {(part) => (
          <Switch>
            <Match when={part.kind === "text" && part}>
              {(text) => (user() ? <div class="pc-msg-user">{text().text}</div> : <Markdown text={text().text} live={false} class="pc-msg-text" />)}
            </Match>
            <Match when={part.kind === "reasoning" && part}>
              {(reasoning) => (
                <details class="pc-msg-reasoning">
                  <summary>Thinking</summary>
                  <p>{reasoning().text}</p>
                </details>
              )}
            </Match>
            <Match when={part.kind === "tool" && part}>
              {(tool) => (
                <div class="pc-msg-tool" data-error={tool().isError ? "true" : undefined}>
                  <Icon name={tool().isError ? "close-circle" : "check-circle"} size={12} />
                  <span class="pc-truncate">{tool().call.name}</span>
                </div>
              )}
            </Match>
            <Match when={part.kind === "image" && part}>{(image) => <div class="pc-msg-tool">Image · {image().name}</div>}</Match>
            <Match when={part.kind === "visual" && part}>{(visual) => <div class="pc-msg-tool">Visual · {visual().title}</div>}</Match>
            <Match when={part.kind === "error" && part}>
              {(error) => (
                <div class="pc-msg-error" role="note">
                  {error().message}
                </div>
              )}
            </Match>
          </Switch>
        )}
      </For>
    </article>
  );
}
