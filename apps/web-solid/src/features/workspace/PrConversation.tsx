import { openChat } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { draftFor, focusComposer, setDraft } from "@/features/composer/drafts";
import { queueEdit } from "@/features/composer/queueEdit";
import { Button } from "@/ui/primitives";
import { Show } from "solid-js";
import type { ChangeRequestDetail } from "./types";
import { appendPrQuestion, prQuestion, prStatus } from "./pr-conversation";

export function PrConversation(props: { detail: ChangeRequestDetail; chatId: string; archived: boolean; disabled: boolean }) {
  const chatId = () => zapperForChat(props.chatId)?.coordinatorChatId ?? props.chatId;
  const ask = (intent: "summary" | "blockers") => {
    if (props.disabled || props.archived || queueEdit()) return;
    const id = chatId();
    setDraft(id, { text: appendPrQuestion(draftFor(id).text, prQuestion(props.detail.url, intent)) });
    openChat(id);
    focusComposer();
  };
  return (
    <section class="ws-pr-conversation" aria-label="Pull request conversation">
      <p class="ws-pr-status" role="status">{prStatus(props.detail)}</p>
      <p class="ws-small ws-muted">Continue in {zapperForChat(props.chatId)?.config.name ?? "chat"} to discuss this change. Your question stays editable until you send it.</p>
      <Show when={queueEdit()}><p class="ws-small ws-muted">Finish or cancel your queued-message edit before adding a PR question.</p></Show>
      <div class="ws-pr-prompts">
        <Button size="sm" variant="ghost" disabled={props.disabled || props.archived || !!queueEdit()} onClick={() => ask("summary")}>Summarize this PR</Button>
        <Button size="sm" variant="ghost" disabled={props.disabled || props.archived || !!queueEdit()} onClick={() => ask("blockers")}>Explain blockers</Button>
      </div>
    </section>
  );
}
