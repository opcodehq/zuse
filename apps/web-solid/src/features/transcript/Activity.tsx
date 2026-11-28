// Recorded issue activity as compact one-line rows between messages, grouped under a summarizing divider.
import { state } from "@/core/store";
import { Avatar, defaultAvatar } from "@/features/zappers/Avatar";
import { timeAgo } from "@/features/workspace/util";
import { Icon } from "@/ui/Icon";
import { For, Match, Show, Switch } from "solid-js";
import { type ActivityEvent, COLLAPSE_AFTER, summarize } from "./events";
import { decorateReferences, ownerZapper, referenceClick } from "./ReferenceMarkdown";
import type { ReferenceCatalog } from "./references";
import { isOpen, toggleOpen } from "./resources";

const initial = () => {
  const user = (state.account as { user?: { name?: string; email?: string } } | null)?.user;
  return (user?.name || user?.email || "Y").charAt(0).toUpperCase();
};

const escapeHtml = (text: string) => text.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

export function ActivityGroup(props: { events: ActivityEvent[]; chatId: string; references: ReferenceCatalog }) {
  const key = () => `activity:${props.events[0]!.id}`;
  const collapsedByDefault = () => props.events.length > COLLAPSE_AFTER;
  const open = () => isOpen(key(), !collapsedByDefault());
  const zapper = () => ownerZapper(props.chatId);
  const issueCode = (issueId: string) => {
    const number = zapper()?.issues.find(issue => issue.id === issueId)?.number;
    return number === undefined ? undefined : `ZAP-${number}`;
  };
  return (
    <div class="t-activity" data-open={open() ? "true" : undefined}>
      <button type="button" class="t-activity-divider" aria-expanded={open() ? "true" : "false"} onClick={() => toggleOpen(key(), !collapsedByDefault())}>
        <span class="t-activity-summary">
          <Icon name={open() ? "alt-arrow-down" : "alt-arrow-right"} size={11} />
          {summarize(props.events, issueCode)}
        </span>
      </button>
      <Show when={open()}>
        <div class="t-activity-rows" onClick={(event) => referenceClick(event, props.references, props.chatId)}>
          <For each={props.events}>
            {(event) => (
              <div class="t-activity-row">
                <Switch>
                  <Match when={event.actor.kind === "zapper" && zapper()}>
                    {(owner) => <span class="t-activity-actor"><Avatar avatar={owner().config.avatar ?? defaultAvatar(owner().id)} image={owner().config.avatarImage} size={16} /><strong>{owner().config.name}</strong></span>}
                  </Match>
                  <Match when={event.actor.kind === "thread" && event.actor}>
                    {(actor) => <span class="t-activity-actor"><Icon name="chat-round-line" size={14} /><strong>{state.chats[(actor() as { chatId: string }).chatId]?.title || "A thread"}</strong></span>}
                  </Match>
                  <Match when={event.actor.kind === "user"}>
                    <span class="t-activity-actor"><span class="t-activity-initial" aria-hidden="true">{initial()}</span><strong>You</strong></span>
                  </Match>
                </Switch>
                {/* eslint-disable-next-line solid/no-innerhtml -- escaped text; chips are built with DOM APIs and bundled SVGs */}
                <span class="t-activity-text" innerHTML={decorateReferences(escapeHtml(event.text), props.references)} />
                <time class="t-activity-time" datetime={new Date(event.at).toISOString()} title={new Date(event.at).toLocaleString()}>{timeAgo(event.at)}</time>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}
