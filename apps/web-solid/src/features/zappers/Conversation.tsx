import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Button } from "@/ui/primitives";
import { Markdown } from "@/features/transcript/Markdown";
import { openChat } from "@/core/view";
import { state } from "@/core/store";
import type { IssueActivity, Zapper, ZapperIssue } from "@/core/zapper-types";
import { Avatar, avatarOf } from "./Avatar";
import { command, commandState, threadTitle } from "./state";
import "./conversation.css";
import { deliveryNote, indicator, indicatorText, phrase, runState, summary, timeline, type Item } from "./timeline";

type Props = { zapper: Zapper; issue: ZapperIssue; onOpenPr?: (url: string, chatId: string) => void };

const relative = (at: number) => {
  const s = Math.max(0, Math.floor((Date.now() - at) / 1000));
  return s < 60 ? "just now" : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
};
const exact = (at: number) => new Date(at).toLocaleString();

/** The shared issue conversation: attributed comments with nested replies, quiet events, who is working, and the comment box. */
export function Conversation(props: Props) {
  const coordinators = () => [props.zapper.coordinatorChatId, ...props.zapper.previousCoordinatorChats];
  const isThread = (id: string) => props.zapper.workers.some(w => w.chatId === id) || props.issue.threads.includes(id);
  const name = (id: string) =>
    id === "user" ? "you" : id === "zapper" || coordinators().includes(id) ? props.zapper.config.name : threadTitle(id, props.zapper.workers.find(w => w.chatId === id)?.objective.split("\n")[0] ?? "Earlier thread");
  const pr = (url: string) => {
    const parts = url.replace(/\/$/, "").split("/");
    return `${parts.at(-3) ?? ""}#${parts.at(-1) ?? ""}`;
  };
  const replies = createMemo(() => {
    const map = new Map<string, IssueActivity[]>();
    for (const e of props.issue.activity) if (e.replyTo) map.set(e.replyTo, [...(map.get(e.replyTo) ?? []), e]);
    return map;
  });
  const [open, setOpen] = createSignal<Set<string>>(new Set());
  const toggle = (key: string) => setOpen(s => { const next = new Set(s); if (!next.delete(key)) next.add(key); return next; });
  const status = () => indicator(props.zapper, props.issue, chat => runState(state.sessions[chat] as never), name);
  return (
    <section class="zp-conversation" aria-label="Issue conversation">
      <For each={timeline(props.issue.activity)}>
        {(item: Item) => (
          <Show when={item.kind === "card"} fallback={
            <Show when={item.kind === "group" && item} fallback={item.kind === "event" && <EventRow event={item.event} name={name} pr={pr} issue={props.issue} isThread={isThread} coordinators={coordinators()} zapper={props.zapper} onOpenPr={props.onOpenPr} />}>
              {group => {
                const events = () => (group() as { events: IssueActivity[] }).events;
                const key = () => events()[0]!.id;
                return <div class="zp-event-group">
                  <button type="button" class="zp-event-summary" aria-expanded={open().has(key()) ? "true" : "false"} onClick={() => toggle(key())}>
                    <Icon name="history" size={12} />
                    <span>{summary(events(), pr)}</span>
                    <Icon name={open().has(key()) ? "alt-arrow-up" : "alt-arrow-down"} size={10} />
                  </button>
                  <Show when={open().has(key())}>
                    <For each={events()}>{event => <EventRow event={event} name={name} pr={pr} issue={props.issue} isThread={isThread} coordinators={coordinators()} zapper={props.zapper} onOpenPr={props.onOpenPr} />}</For>
                  </Show>
                </div>;
              }}
            </Show>
          }>
            <Card event={(item as { event: IssueActivity }).event} replies={replies().get((item as { event: IssueActivity }).event.id) ?? []} name={name} isThread={isThread} coordinators={coordinators()} zapper={props.zapper} issue={props.issue} />
          </Show>
        )}
      </For>
      <Show when={status()}>
        {s => <div class="zp-run-state" role="status" data-kind={s().kind}><span class="zp-run-dots" aria-hidden="true" />{indicatorText(s())}</div>}
      </Show>
      <Composer zapper={props.zapper} issue={props.issue} />
    </section>
  );
}

function Who(props: { actor?: string | null; name: (id: string) => string; isThread: (id: string) => boolean; coordinators: string[]; zapper: Zapper; size?: number }) {
  const actor = () => props.actor ?? null;
  return (
    <Show when={actor()} fallback={<><span class="zp-person" aria-hidden="true">Y</span><strong>You</strong></>}>
      {id => (
        <Show when={props.coordinators.includes(id()) || id() === props.zapper.config.name} fallback={
          <Show when={props.isThread(id())} fallback={<strong title="Unknown author">{id()}</strong>}>
            <Icon name="chat-round-line" size={props.size ?? 14} />
            <button type="button" class="zp-author-link" onClick={() => openChat(id())}>{props.name(id())}</button>
          </Show>
        }>
          <Avatar avatar={avatarOf(props.zapper)} image={props.zapper.config.avatarImage} size={props.size ?? 18} />
          <strong>{props.zapper.config.name}</strong>
        </Show>
      )}
    </Show>
  );
}

function Delivery(props: { event: IssueActivity; name: (id: string) => string }) {
  return <Show when={props.event.delivery}>{d => {
    const note = () => deliveryNote(d(), props.name(d().chatId));
    return <p class="zp-delivery" data-attention={note().attention ? "true" : undefined}>{note().text}</p>;
  }}</Show>;
}

function Card(props: { event: IssueActivity; replies: IssueActivity[]; name: (id: string) => string; isThread: (id: string) => boolean; coordinators: string[]; zapper: Zapper; issue: ZapperIssue }) {
  const [replying, setReplying] = createSignal(false);
  const tag = () => props.event.kind === "question" ? (props.issue.details.status === "needsYou" ? "Needs your answer" : "Question") : props.event.kind === "workerReport" && /^Worker (failed|interrupted|needs input)/.test(props.event.text) ? props.event.text.split("\n")[0]!.replace("Worker ", "") : undefined;
  const body = (e: IssueActivity) => e.kind === "workerReport" ? e.text.replace(/^Worker [^\n]*\n/, "") : e.text;
  // Replying in a card continues with the thread that spoke last in it.
  const target = () => [...props.replies].reverse().find(r => r.actor && props.isThread(r.actor) && !props.coordinators.includes(r.actor))?.id ?? props.event.id;
  return (
    <article class="zp-card" data-kind={props.event.kind}>
      <header class="zp-card-head">
        <Who actor={props.event.actor} name={props.name} isThread={props.isThread} coordinators={props.coordinators} zapper={props.zapper} />
        <Show when={tag()}><span class="zp-card-tag">{tag()}</span></Show>
        <time title={exact(props.event.createdAt)}>{relative(props.event.createdAt)}</time>
        <button type="button" class="zp-icon-button" aria-label="Reply" onClick={() => setReplying(true)}><Icon name="return" size={12} /></button>
      </header>
      <Markdown text={body(props.event)} live={false} />
      <Delivery event={props.event} name={props.name} />
      <For each={props.replies}>
        {reply => (
          <div class="zp-card-reply">
            <header class="zp-card-head">
              <Who actor={reply.actor} name={props.name} isThread={props.isThread} coordinators={props.coordinators} zapper={props.zapper} size={16} />
              <time title={exact(reply.createdAt)}>{relative(reply.createdAt)}</time>
            </header>
            <Markdown text={body(reply)} live={false} />
            <Delivery event={reply} name={props.name} />
          </div>
        )}
      </For>
      <Show when={replying()}>
        <Composer zapper={props.zapper} issue={props.issue} replyTo={target()} onDone={() => setReplying(false)} />
      </Show>
    </article>
  );
}

function EventRow(props: { event: IssueActivity; name: (id: string) => string; pr: (url: string) => string; issue: ZapperIssue; isThread: (id: string) => boolean; coordinators: string[]; zapper: Zapper; onOpenPr?: (url: string, chatId: string) => void }) {
  const p = () => phrase(props.event, props.name, props.pr);
  const fallback = () => {
    let text = props.event.text.replace(/\n/g, " · ");
    for (const w of [...props.zapper.workers].sort((a, b) => b.chatId.length - a.chatId.length)) text = text.split(w.chatId).join(props.name(w.chatId));
    return text;
  };
  return (
    <div class="zp-event" data-status={p()?.status}>
      <span class="zp-event-mark" aria-hidden="true" />
      <Who actor={props.event.actor} name={props.name} isThread={props.isThread} coordinators={props.coordinators} zapper={props.zapper} size={14} />
      <span class="zp-event-action">{p()?.action ?? fallback()}</span>
      <Show when={p()?.target}>
        {t => <button type="button" class="zp-author-link" onClick={() => t().kind === "thread" ? openChat(t().value) : props.onOpenPr ? props.onOpenPr(t().value, props.issue.threads[0] ?? props.zapper.coordinatorChatId) : window.open(t().value, "_blank", "noreferrer")}>{t().label}</button>}
      </Show>
      <time title={exact(props.event.createdAt)}>{relative(props.event.createdAt)}</time>
    </div>
  );
}

/** A comment or reply box. The draft stays until the engine confirms the write. */
function Composer(props: { zapper: Zapper; issue: ZapperIssue; replyTo?: string; onDone?: () => void }) {
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const send = async (event: Event) => {
    event.preventDefault();
    if (!text().trim() || commandState.busy) return;
    setError(null);
    try {
      await command(props.zapper.id, { type: "commentIssue", issueId: props.issue.id, text: text().trim(), replyTo: props.replyTo });
      setText("");
      props.onDone?.();
    } catch {
      setError(commandState.error ?? "The comment wasn't saved. Your text is still here.");
    }
  };
  return (
    <form class="zp-composer" data-reply={props.replyTo ? "true" : undefined} onSubmit={send}>
      <textarea
        aria-label={props.replyTo ? "Reply" : "Leave a comment"}
        placeholder={props.replyTo ? "Reply…" : "Leave a comment"}
        value={text()}
        rows={props.replyTo ? 2 : 3}
        onInput={e => setText(e.currentTarget.value)}
        onKeyDown={e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void send(e); }}
      />
      <Show when={error()}><p class="zp-error" role="alert">{error()}</p></Show>
      <div class="zp-composer-actions">
        <Show when={props.onDone}><Button size="sm" onClick={() => props.onDone?.()}>Cancel</Button></Show>
        <Button size="sm" type="submit" variant="primary" disabled={commandState.busy || !text().trim()}>{commandState.busy ? "Sending…" : props.replyTo ? "Reply" : "Comment"}</Button>
      </div>
    </form>
  );
}

/** "Threads N": the Zapper and each linked thread, the owner marked, each opening its conversation. */
export function ThreadsControl(props: { zapper: Zapper; issue: ZapperIssue }) {
  const [open, setOpen] = createSignal(false);
  const chats = () => [props.zapper.coordinatorChatId, ...props.issue.threads];
  const owner = () => props.issue.details.owner?.type === "thread" ? props.issue.details.owner.chatId : props.issue.details.owner?.type === "zapper" || (!props.issue.details.owner && props.issue.details.assigned) ? props.zapper.coordinatorChatId : undefined;
  const label = (chat: string, index: number) => {
    const run = runState(state.sessions[chat] as never);
    const role = index === 0 ? (owner() === chat ? "Coordinator · owner" : "Coordinator") : owner() === chat ? "Owner" : "Linked";
    const live = run === "running" ? "Working" : run === "awaitingInput" ? "Needs input" : run === "failed" ? "Failed" : run === "unconfirmed" ? "Not confirmed" : index > 0 && !props.zapper.workers.some(w => w.chatId === chat) ? "Left the Zapper" : "";
    return live ? `${role} · ${live}` : role;
  };
  return (
    <div class="zp-threads-control">
      <button type="button" class="zp-threads-button" aria-haspopup="menu" aria-expanded={open() ? "true" : "false"} onClick={() => setOpen(!open())}>
        <span class="zp-run-dots" aria-hidden="true" />Threads <span class="zp-muted">{chats().length}</span>
      </button>
      <Show when={open()}>
        <div class="zp-threads-menu" role="menu" onKeyDown={e => { if (e.key === "Escape") setOpen(false); }}>
          <For each={chats()}>
            {(chat, index) => (
              <button type="button" role="menuitem" onClick={() => { setOpen(false); openChat(chat); }}>
                <Show when={index() === 0} fallback={<Icon name="chat-round-line" size={14} />}><Avatar avatar={avatarOf(props.zapper)} image={props.zapper.config.avatarImage} size={18} /></Show>
                <span><strong>{index() === 0 ? props.zapper.config.name : threadTitle(chat, props.zapper.workers.find(w => w.chatId === chat)?.objective.split("\n")[0] ?? "Earlier thread")}</strong><small>{label(chat, index())}</small></span>
              </button>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
