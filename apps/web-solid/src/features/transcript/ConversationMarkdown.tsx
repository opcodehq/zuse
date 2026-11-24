import { createMemo } from "solid-js";
import { state } from "@/core/store";
import { openChat } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { openSubagent } from "@/features/workspace/agents";
import { workspace } from "@/features/workspace/store";
import { showPage } from "@/features/zappers/panel";
import { setInspectorOpen } from "@/features/zappers/state";
import { Markdown } from "./Markdown";
import { conversationReferences, type InlineReference } from "./references";

export function ConversationMarkdown(props: { text: string; live: boolean; chatId: string }) {
  const owner = createMemo(() => {
    const direct = zapperForChat(props.chatId);
    if (direct) return direct;
    const linked = state.zappers.filter(zapper => zapper.issues.some(issue => issue.threads.includes(props.chatId)));
    return linked.length === 1 ? linked[0] : undefined;
  });
  const references = createMemo(() => conversationReferences(owner(), state.chats, props.chatId, state.transcript?.chatId === props.chatId ? state.transcript.messages : [], workspace.changeRequests[props.chatId]?.changeRequest));
  const open = (reference: InlineReference) => {
    if (reference.kind === "thread") openChat(reference.target);
    else if (reference.kind === "agent") openSubagent(props.chatId, reference.target);
    else if (reference.kind === "issue" && owner()) {
      showPage(owner()!.id, { kind: "issue", id: reference.target });
      setInspectorOpen(true);
    }
  };
  return <Markdown text={props.text} live={props.live} references={references()} onReference={open} />;
}
