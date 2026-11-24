import type { IssueDetails, Zapper } from "@/core/zapper-types";

export function issueAgentWorking(details: IssueDetails, zapper: Zapper, connected: boolean, sessionStatus: (chatId: string) => string | undefined, online: (deviceId: string) => boolean): boolean {
  if (!connected || ["blocked", "needsYou", "done", "canceled"].includes(details.status)) return false;
  const owner = details.owner ?? (details.assigned ? { type: "zapper" as const } : undefined);
  if (owner?.type !== "thread") return false;
  const worker = zapper.workers.find(worker => worker.chatId === owner.chatId);
  return !!worker && worker.queuePosition === undefined && online(worker.deviceId) && sessionStatus(owner.chatId) === "working";
}
