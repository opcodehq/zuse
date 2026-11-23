import { createSignal } from "solid-js";
import { openPanel } from "./store";

export const [selectedSubagent, setSelectedSubagent] = createSignal<{ chatId: string; docId: string } | null>(null);
export function openSubagent(chatId: string, docId: string) {
  setSelectedSubagent({ chatId, docId });
  openPanel("agents");
}
