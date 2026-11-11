// Archived chats of a project: search, a read-only preview of each session, and restore.
import { createEffect, createMemo, createSignal, For, Match, on, onCleanup, Show, Switch } from "solid-js";
import type { Chat, SessionMessageEntry } from "@/core/protocol";
import { state } from "@/core/store";
import { navigate, openChat, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Button, IconButton, Spinner } from "@/ui/primitives";
import {
  activityLabel,
  archivedThreads,
  chatSummary,
  chatTitle,
  clearFiles,
  closeSession,
  dateLabel,
  chatDate,
  files,
  loadFiles,
  preview,
  progressing,
  projectSpaces,
  readSession,
  restoreChat,
  restoring,
  setRestoring,
  threadSessions,
} from "./archive";
import { spaceName } from "./hosts";
import { Entry, type EntryContext } from "@/features/transcript/Entry";
import "@/features/transcript/transcript.css";
import { VirtualList } from "./Palette";

export function ProjectArchive() {
  const projectKey = () => (view.route.kind === "archive" ? view.route.projectKey : undefined);
  const spaces = createMemo(() => projectSpaces(projectKey()));
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal<string | null>(null);
  const [session, setSession] = createSignal<string | null>(null);

  const project = createMemo(() => {
    const ids = spaces();
    if (!ids) return null;
    const rows = Object.values(state.spaces).filter((s) => ids.has(s.id));
    rows.sort((a, b) => a.createdAt.localeCompare(b.createdAt) || a.id.localeCompare(b.id));
    return rows[0] ?? null;
  });

  const thread = () => {
    const id = selected();
    return id ? state.chats[id] : undefined;
  };
  const sessions = createMemo(() => {
    const id = selected();
    return id ? threadSessions(id, spaces()) : [];
  });

  const open = (id: string) => {
    setRestoring({ error: null, target: null });
    setSelected(id);
    const first = threadSessions(id, spaces())[0]?.id ?? id;
    selectSession(first);
    void loadFiles(id);
  };
  const selectSession = (id: string) => {
    setSession(id);
    readSession(id);
  };
  const back = () => {
    clearFiles();
    closeSession();
    setSelected(null);
    setSession(null);
  };

  // Restored or deleted elsewhere: leave the preview.
  createEffect(() => {
    const id = selected();
    if (id && !restoring.busy && !state.chats[id]?.archived) back();
  });
  createEffect(on(projectKey, () => back(), { defer: true }));
  onCleanup(() => {
    clearFiles();
    closeSession();
  });

  const restore = async (id: string) => {
    if (await restoreChat(id)) openChat(id);
  };

  return (
    <div class="pj-archive" onKeyDown={(e) => e.key === "Escape" && selected() && (e.preventDefault(), back())}>
      <Show
        when={thread()}
        fallback={
          <Show
            when={!projectKey() || project()}
            fallback={<div class="pj-archive-centered">Select a project to view archived chats.</div>}
          >
            <ArchiveList projectName={project() ? spaceName(project()!) : null} scoped={!!projectKey()} query={query()} setQuery={setQuery} onOpen={open} onRestore={restore} spaces={spaces()} />
          </Show>
        }
      >
        {(chat) => (
          <ArchivePreview chat={chat()} sessions={sessions()} session={session()} onSession={selectSession} onBack={back} onRestore={restore} />
        )}
      </Show>
    </div>
  );
}

type ListEntry = { kind: "date"; label: string; first: boolean } | { kind: "chat"; chat: Chat };

function ArchiveList(props: {
  projectName: string | null;
  scoped: boolean;
  query: string;
  setQuery: (value: string) => void;
  onOpen: (id: string) => void;
  onRestore: (id: string) => void;
  spaces: Set<string> | null;
}) {
  const rows = createMemo(() => archivedThreads(props.spaces, props.query));
  const entries = createMemo<ListEntry[]>(() => {
    const out: ListEntry[] = [];
    let previous = "";
    for (const chat of rows()) {
      const label = dateLabel(chatDate(chat));
      if (label !== previous) out.push({ kind: "date", label, first: !out.length });
      previous = label;
      out.push({ kind: "chat", chat });
    }
    return out;
  });
  const count = () => {
    const n = rows().length;
    return `${n} ${props.query.trim() ? (n === 1 ? "match" : "matches") : n === 1 ? "chat" : "chats"}`;
  };
  const projectOf = (chat: Chat) => (chat.spaceId && state.spaces[chat.spaceId] ? spaceName(state.spaces[chat.spaceId]!) : "No project");
  return (
    <>
      <header class="pj-archive-head">
        <div class="pj-archive-titlebar">
          {/* Settings has its own "Back to app"; only the full-page archive needs a way back. */}
          <Show when={view.route.kind === "archive"}>
            <IconButton icon="arrow-left" label="Back to chats" onClick={() => navigate({ kind: "chat" })} />
          </Show>
          <Icon name="archive-minimalistic" size={20} class="pj-glyph" />
          <div class="pj-archive-title">
            <h1>Archived chats</h1>
            <span>{props.projectName ?? "All projects"}</span>
          </div>
          <span class="pj-pill">{count()}</span>
        </div>
        <label class="pj-search">
          <Icon name="magnifer" size={16} />
          <input
            value={props.query}
            placeholder="Search titles, previews or branches…"
            aria-label="Search archived chats"
            onInput={(e) => props.setQuery(e.currentTarget.value)}
          />
        </label>
      </header>
      <Show
        when={rows().length}
        fallback={
          <div class="pj-archive-centered pj-archive-empty">
            <Icon name="archive-minimalistic" size={28} />
            <strong>{props.query.trim() ? "No matching chats" : "No archived chats yet"}</strong>
            <span>
              {props.query.trim()
                ? "Try another title, preview, or branch."
                : `Archived chats from ${props.projectName ?? "your projects"} will appear here. Preview them before restoring.`}
            </span>
          </div>
        }
      >
        <VirtualList
          id="pj-archive-rows"
          class="pj-archive-list"
          role="list"
          label="Archived chats"
          pad={20}
          items={entries()}
          estimate={(e) => (e.kind === "date" ? (e.first ? 24 : 48) : 64)}
          itemKey={(e, i) => (e.kind === "chat" ? e.chat.id : `date-${e.label}-${i}`)}
        >
          {(entry) => (
            <Show
              when={entry.kind === "chat" && entry.chat}
              fallback={
                <div class="pj-archive-date" data-first={(entry as { first: boolean }).first ? "true" : undefined} role="presentation">
                  {(entry as { label: string }).label}
                </div>
              }
            >
              {(chat) => (
                <div class="pj-archive-row" role="listitem">
                  <button type="button" class="pj-archive-open" aria-label={`Open ${chatTitle(chat())}`} onClick={() => props.onOpen(chat().id)}>
                    <span class="pj-archive-icon">
                      <Icon name="archive-minimalistic" size={16} />
                    </span>
                    <span class="pj-archive-text">
                      <span class="pj-archive-row-title">{chatTitle(chat())}</span>
                      <span class="pj-archive-row-summary">
                        {props.scoped ? chatSummary(chat()) : `${projectOf(chat())} · ${chatSummary(chat())}`}
                      </span>
                    </span>
                    <span class="pj-archive-when">{activityLabel(chat())}</span>
                  </button>
                  <div class="pj-archive-actions">
                    <RestoreButton chatId={chat().id} title={chatTitle(chat())} onRestore={props.onRestore} />
                    <Show when={restoring.target === chat().id && restoring.error}>
                      <span class="pj-archive-error" role="alert">
                        {restoring.error}
                      </span>
                    </Show>
                  </div>
                </div>
              )}
            </Show>
          )}
        </VirtualList>
      </Show>
    </>
  );
}

function RestoreButton(props: { chatId: string; title: string; onRestore: (id: string) => void; retry?: boolean; class?: string }) {
  const working = () => restoring.busy === props.chatId;
  return (
    <button
      type="button"
      class={`pj-restore ${props.class ?? ""}`}
      aria-label={working() ? "Restoring…" : `Restore chat ${props.title}`}
      aria-disabled={!!restoring.busy}
      data-busy={restoring.busy ? "true" : undefined}
      onClick={() => !restoring.busy && props.onRestore(props.chatId)}
    >
      <Show when={working()} fallback={<Icon name="archive-up-minimalistic" size={14} />}>
        <Spinner label="Restoring" />
      </Show>
      {working() ? "Restoring…" : props.retry ? "Retry" : "Restore chat"}
    </button>
  );
}

function ArchivePreview(props: {
  chat: Chat;
  sessions: Chat[];
  session: string | null;
  onSession: (id: string) => void;
  onBack: () => void;
  onRestore: (id: string) => void;
}) {
  const error = () => (restoring.target === props.chat.id ? restoring.error : null);
  return (
    <div class="pj-preview">
      <header class="pj-preview-head">
        <IconButton icon="alt-arrow-left" label="Back to archived chats" onClick={() => props.onBack()} />
        <Icon name="archive-minimalistic" size={16} class="pj-glyph" />
        <div class="pj-archive-title pj-preview-title">
          <h1>{chatTitle(props.chat)}</h1>
          <span>{activityLabel(props.chat)}</span>
        </div>
      </header>
      <div class="pj-sessions" role="tablist" aria-label="Archived chat sessions">
        <For each={props.sessions}>
          {(session, index) => (
            <button
              type="button"
              role="tab"
              class="pj-session-tab"
              aria-selected={props.session === session.id}
              onClick={() => props.onSession(session.id)}
            >
              <span>{session.title?.trim() || `Conversation ${index() + 1}`}</span>
            </button>
          )}
        </For>
      </div>
      <FilesBanner />
      <div class="pj-preview-body">
        <Switch>
          <Match when={preview.error}>
            {(message) => (
              <div class="pj-archive-centered">
                <span>{message()}</span>
                <Button size="sm" variant="outline" onClick={() => props.session && props.onSession(props.session)}>
                  Retry
                </Button>
              </div>
            )}
          </Match>
          <Match when={preview.reading}>
            <div class="pj-archive-centered">Loading transcript…</div>
          </Match>
          <Match when={!preview.messages.length}>
            <div class="pj-archive-centered">No messages in this session.</div>
          </Match>
          <Match when={true}>
            <ArchiveTranscript chatId={props.session ?? props.chat.id} messages={preview.messages} />
          </Match>
        </Switch>
      </div>
      <footer class="pj-preview-foot">
        <Icon name="archive-minimalistic" size={16} class="pj-glyph" />
        <div class="pj-preview-note">
          <span>Read-only preview. Restore this chat to continue working.</span>
          <Show when={error()}>
            <span class="pj-archive-error" role="alert">
              {error()}
            </span>
          </Show>
        </div>
        <RestoreButton chatId={props.chat.id} title={chatTitle(props.chat)} onRestore={props.onRestore} retry={!!error()} class="pj-restore-outline" />
      </footer>
    </div>
  );
}

function FilesBanner() {
  const content = (): [string, string] | null => {
    if (files.loading && files.error) return ["Checking local files…", "Saved history remains available."];
    if (files.error) return ["File status unavailable", "Saved history is still available. Retry to check the owning computer."];
    const value = files.value;
    if (!value) return null;
    let pair: [string, string] | null = null;
    if (value.phase === "prepared") pair = ["Preparing local file archive", "Saving the checkout before local storage is reclaimed."];
    else if (value.phase === "restoring") pair = ["Restoring local files", "Recreating the checkout from its saved archive."];
    else if (value.phase === "restored") pair = ["Checkout archive restored", "The saved checkout was restored on its owning computer."];
    else if (value.storageReclaimed) pair = ["Local files archived", "Restore this chat to recreate its checkout."];
    if (!pair) return null;
    const changed = value.changedPaths?.length ?? 0;
    return changed ? [pair[0], `${pair[1]} ${changed} changed paths saved.`] : pair;
  };
  return (
    <Show when={content()}>
      {(text) => (
        <div class="pj-files" role={files.error ? "alert" : "status"}>
          <Icon name="archive-minimalistic" size={16} class="pj-glyph" />
          <div class="pj-files-text">
            <strong>{text()[0]}</strong>
            <span>{text()[1]}</span>
          </div>
          <Button size="sm" variant="ghost" disabled={files.loading} onClick={() => files.chatId && void loadFiles(files.chatId)}>
            {files.loading ? "Checking…" : progressing(files.value) ? "Refresh status" : "Refresh status"}
          </Button>
        </div>
      )}
    </Show>
  );
}

// ── Read-only transcript ─────────────────────────────────────────────────

// Desktop renders archives with its live transcript over frozen state; reuse the transcript's entry view.
function ArchiveTranscript(props: { chatId: string; messages: SessionMessageEntry[] }) {
  const chat = () => state.chats[props.chatId];
  const ctxFor = (message: SessionMessageEntry): EntryContext => ({
    chatId: props.chatId,
    device: () => chat()?.deviceId ?? null,
    harness: () => chat()?.config?.harness,
    last: () => props.messages[props.messages.length - 1]?.id === message.id,
  });
  return (
    <div class="t-root pj-transcript-root">
      <VirtualList
        id="pj-archive-transcript"
        class="pj-transcript"
        role="log"
        label="Archived transcript"
        pad={16}
        items={props.messages}
        estimate={(m) => (m.role === "user" ? 60 : 160)}
        itemKey={(m) => m.id}
      >
        {(message) => (
          <div class="t-row">
            <div class="t-column">
              <Entry entry={message} ctx={ctxFor(message)} />
            </div>
          </div>
        )}
      </VirtualList>
    </div>
  );
}
