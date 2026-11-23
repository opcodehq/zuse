import { state } from "@/core/store";
import type { ZapperWorker, Zapper } from "@/core/zapper-types";
export function workerStatus(worker: ZapperWorker) {
  if (worker.queuePosition !== undefined) return `Queued · ${worker.queuePosition}`;
  const status=state.sessions[worker.chatId]?.status;
  return status === "working" ? "Working" : status === "awaitingInput" ? "Waiting for input" : status === "errored" ? "Needs attention" : state.chats[worker.chatId]?.lastMessageAt || state.sessions[worker.chatId]?.startedAt ? "Idle" : "Starting";
}
export function workerRank(worker: ZapperWorker) {
  const status=state.sessions[worker.chatId]?.status;
  if(worker.queuePosition !== undefined) return 2;
  return status === "working" ? 0 : ["awaitingInput","errored"].includes(status ?? "") ? 1 : 3;
}
export function finishedThreads(zapper: Zapper) {
  return [...new Set([...zapper.issues].reverse().flatMap(issue=>[...issue.threads].reverse()))].filter(id=> /^(zapper|zot)-worker-/.test(id) && !zapper.workers.some(worker=>worker.chatId===id) && !state.chats[id]?.archived);
}
