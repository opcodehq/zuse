import { state } from "@/core/store";
import type { ZapperWorker, Zapper } from "@/core/zapper-types";

/** Color of a thread row. Queued wins over a live session status. */
export type ThreadTone = "working" | "needs" | "failed" | "queued" | "idle";

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
export function threadTone(worker: Pick<ZapperWorker, "queuePosition">, status: string | undefined): ThreadTone {
  if (worker.queuePosition !== undefined) return "queued";
  if (status === "working") return "working";
  if (status === "awaitingInput") return "needs";
  if (status === "errored") return "failed";
  return "idle";
}
/** Every word must appear somewhere in the joined fields. An empty query matches. */
export function threadMatches(query: string, fields: string[]) {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const haystack = fields.join("\n").toLowerCase();
  return words.every((word) => haystack.includes(word));
}
export function finishedThreads(zapper: Zapper) {
  return [...new Set([...zapper.issues].reverse().flatMap(issue=>[...issue.threads].reverse()))].filter(id=> /^(zapper|zot)-worker-/.test(id) && !zapper.workers.some(worker=>worker.chatId===id) && !state.chats[id]?.archived);
}
