import { zapperForChat } from "@/core/zappers";
import { setView } from "@/core/view";
import { showPage } from "@/features/zappers/panel";
import { setInspectorOpen } from "@/features/zappers/state";
import { createSignal } from "solid-js";
import { openPanel } from "./store";

export const [selectedSubagent, setSelectedSubagent] = createSignal<{ chatId: string; docId: string } | null>(null);
export function openSubagent(chatId: string, docId: string, title?: string) {
  setSelectedSubagent({ chatId, docId });
  const owner=zapperForChat(chatId);
  if(owner) { showPage(owner.id,{kind:"agent",docId,chatId,title});setInspectorOpen(true);setView(draft=>{draft.rightPanel=null;}); }
  else openPanel("agents");
}
