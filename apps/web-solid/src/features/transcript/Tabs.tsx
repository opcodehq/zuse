// The conversation tab row (desktop `render_thread_conversations`): the thread's conversations, its side chats,
// `+` for a new conversation in the same checkout, and the history of closed ones.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { createMemo, For, Show } from "solid-js";
import type { Chat } from "@/core/protocol";
import { state } from "@/core/store";
import { openChat } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { addConversation, closeConversation, conversationTabs, restoreConversation } from "./actions";
import { harnessIcon } from "./model";

function timeAgo(iso: string | null | undefined) {
  const at = iso ? Date.parse(iso) : NaN;
  if (!Number.isFinite(at)) return "";
  const s = Math.max(0, (Date.now() - at) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

/** Desktop's title for chats without one: the last message, then a generic name. */
const fallbackTitle = (chat: Chat, generic: string) => chat.title?.trim() || chat.lastMessagePreview?.trim() || generic;

export function ConversationTabs() {
  const tabs = createMemo(conversationTabs);
  return (
    <Show when={tabs()}>
      {(t) => (
        <div class="t-tabs">
          <div class="t-tab-strip" role="tablist" aria-label="Conversations">
            <For each={t().open}>
              {(chat, ix) => (
                <Tab
                  chat={chat}
                  title={chat.title?.trim() || `Chat ${ix() + 1}`}
                  active={chat.id === t().selected.id}
                  closable={t().open.length > 1}
                  onSelect={() => openChat(chat.id)}
                />
              )}
            </For>
            <For each={t().side}>
              {(chat) => (
                <Tab chat={chat} title={fallbackTitle(chat, "Side chat")} active={chat.id === t().selected.id} closable={false} onSelect={() => openChat(chat.id)} side />
              )}
            </For>
            <Show when={!t().anchor.archived}>
              <button type="button" class="t-strip-btn" aria-label="New chat in this thread" title="New chat in this thread" onClick={() => void addConversation()}>
                <Icon name="plus" size={14} />
              </button>
            </Show>
          </div>
          <History chats={t().hidden} />
        </div>
      )}
    </Show>
  );
}

/** The strip's clock: this thread's closed conversations, newest first; a row reopens one as the active tab. */
function History(props: { chats: Chat[] }) {
  return (
    <DropdownMenu placement="bottom-end" gutter={4} modal={false} preventScroll={false}>
      <DropdownMenu.Trigger class="t-strip-btn" aria-label="Earlier chats" title="Earlier chats">
        <Icon name="history" size={14} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="menu t-history">
          <Show when={props.chats.length} fallback={<div class="t-history-empty">No earlier chats</div>}>
            <For each={props.chats}>
              {(chat) => (
                <DropdownMenu.Item class="menu-item t-history-row" onSelect={() => restoreConversation(chat.id)}>
                  <Icon name={chat.config?.harness ? harnessIcon(chat.config.harness) : "chat-round-line"} size={14} />
                  <span class="t-history-title">{fallbackTitle(chat, "Untitled chat")}</span>
                  <span class="t-history-time">{timeAgo(chat.lastMessageAt ?? chat.createdAt)}</span>
                  <Icon name="restore" size={13} />
                </DropdownMenu.Item>
              )}
            </For>
          </Show>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

function Tab(props: { chat: Chat; title: string; active: boolean; closable: boolean; side?: boolean; onSelect: () => void }) {
  const working = () => state.sessions[props.chat.id]?.status === "working";
  return (
    <div
      class="t-tab"
      role="tab"
      tabIndex={props.active ? 0 : -1}
      aria-selected={props.active}
      data-active={props.active ? "true" : undefined}
      title={props.side ? `${props.title} (side chat)` : props.title}
      onClick={() => props.onSelect()}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          props.onSelect();
        } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
          const tabs = [...(e.currentTarget.parentElement?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])];
          const at = tabs.indexOf(e.currentTarget);
          tabs[(at + (e.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length]?.focus();
        } else if ((e.key === "Delete" || e.key === "Backspace") && props.closable) {
          // Keep keyboard focus in the strip: the neighbour that takes the closed tab's place.
          const tabs = [...(e.currentTarget.parentElement?.querySelectorAll<HTMLElement>('[role="tab"]') ?? [])];
          const at = tabs.indexOf(e.currentTarget);
          const strip = e.currentTarget.parentElement;
          closeConversation(props.chat.id);
          queueMicrotask(() => strip?.querySelectorAll<HTMLElement>('[role="tab"]')[Math.min(at, tabs.length - 2)]?.focus());
        }
      }}
      onAuxClick={(e) => {
        if (e.button === 1 && props.closable) closeConversation(props.chat.id);
      }}
    >
      <Show when={working()} fallback={<Icon name={props.chat.config?.harness ? harnessIcon(props.chat.config.harness) : "chat-round-line"} size={14} />}>
        <GlyphSpinner label="Working" cell={2} box={14} />
      </Show>
      <span class="t-tab-title">{props.title}</span>
      <Show when={props.closable}>
        <button
          type="button"
          class="t-tab-close"
          aria-label="Close chat"
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            closeConversation(props.chat.id);
          }}
        >
          <Icon name="close" size={10} />
        </button>
      </Show>
    </div>
  );
}
