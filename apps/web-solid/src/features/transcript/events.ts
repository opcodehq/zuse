import type { Zapper } from "@/core/zapper-types";
import { issueLabel, parsePrUrl } from "./references";

export type ActivityActor = { kind: "zapper"; id: string; name: string } | { kind: "thread"; chatId: string } | { kind: "user" };

export interface ActivityEvent {
  id: string;
  at: number;
  issueId: string;
  actor: ActivityActor;
  /** Plain text whose issue and pull request codes the transcript turns into chips. */
  text: string;
  closed?: "merged" | "dropped";
}

function prLabel(url: string) {
  const pr = parsePrUrl(url.trim());
  return pr ? `${pr.owner}/${pr.repo}#${pr.number}` : null;
}

function describe(kind: string, text: string, code: string): { text: string; closed?: "merged" | "dropped" }[] {
  switch (kind) {
    case "created": return [{ text: `created ${code}` }];
    case "comment": return [{ text: `commented on ${code}` }];
    case "question": return [{ text: `asked you about ${code}` }];
    case "updated":
      return text.split(" · ").flatMap(part => {
        const status = /^Status changed to (.+)$/.exec(part);
        if (status) return [{ text: `moved ${code} to ${status[1]}` }];
        const owner = /^Owner: (.+)$/.exec(part);
        if (owner) return [{ text: `assigned ${code} to ${owner[1]}` }];
        if (part === "Removed owner") return [{ text: `unassigned ${code}` }];
        const priority = /^Priority changed to (.+)$/.exec(part);
        if (priority) return [{ text: `set ${code} priority to ${priority[1]!.toLowerCase()}` }];
        if (part === "Decision received") return [];
        return [{ text: `updated ${code}` }];
      });
    case "thread":
      if (text.startsWith("Started thread")) return [{ text: `started work on ${code}` }];
      if (text.startsWith("Unlinked")) return [{ text: `unlinked a thread from ${code}` }];
      return [{ text: `linked a thread to ${code}` }];
    case "pullRequest": {
      const url = /^Linked pull request (\S+)/.exec(text)?.[1];
      const label = url ? prLabel(url) : null;
      if (label) return [{ text: `linked ${label} to ${code}` }];
      return [{ text: text.startsWith("Unlinked") ? `unlinked a pull request from ${code}` : `linked a pull request to ${code}` }];
    }
    case "closed": {
      const merged = text.startsWith("Merged");
      return [{ text: `closed ${code} as ${merged ? "merged" : "dropped"}`, closed: merged ? "merged" : "dropped" }];
    }
    case "workerReport": {
      const first = text.split("\n", 1)[0]!.trim().toLowerCase();
      if (first === "worker failed") return [{ text: `failed on ${code}` }];
      if (first === "worker interrupted") return [{ text: `was interrupted on ${code}` }];
      if (first === "worker needs input") return [{ text: `needs input on ${code}` }];
      return [{ text: `reported progress on ${code}` }];
    }
    default: return [{ text: `updated ${code}` }];
  }
}

function actorFor(zapper: Zapper, actor: string | null | undefined): ActivityActor {
  if (!actor) return { kind: "user" };
  if (actor === zapper.coordinatorChatId || zapper.previousCoordinatorChats.includes(actor) || actor === zapper.id) return { kind: "zapper", id: zapper.id, name: zapper.config.name };
  return { kind: "thread", chatId: actor };
}

/** Recorded issue activity the chat should see: every issue for the coordinator, linked issues for a worker. */
export function activityEvents(zapper: Zapper | undefined, chatId: string): ActivityEvent[] {
  if (!zapper) return [];
  const coordinator = zapper.coordinatorChatId === chatId || zapper.previousCoordinatorChats.includes(chatId);
  const events: ActivityEvent[] = [];
  for (const issue of zapper.issues) {
    if (!coordinator && !issue.threads.includes(chatId)) continue;
    for (const activity of issue.activity) {
      describe(activity.kind, activity.text, issueLabel(issue.number)).forEach((row, index) => {
        events.push({ id: index ? `${activity.id}:${index}` : activity.id, at: activity.createdAt, issueId: issue.id, actor: actorFor(zapper, activity.actor), ...row });
      });
    }
  }
  return events.sort((a, b) => a.at - b.at || a.id.localeCompare(b.id));
}

/** Places each event after the last message created at or before it; events older than the first loaded message are left out. */
export function slotEvents(events: readonly ActivityEvent[], messages: readonly { id: string; createdAt: number }[]): Map<string, ActivityEvent[]> {
  const slots = new Map<string, ActivityEvent[]>();
  let index = 0;
  for (const event of events) {
    while (index + 1 < messages.length && messages[index + 1]!.createdAt <= event.at) index++;
    const message = messages[index];
    if (!message || message.createdAt > event.at) continue;
    const slot = slots.get(message.id);
    if (slot) slot.push(event);
    else slots.set(message.id, [event]);
  }
  return slots;
}

export function summarize(events: readonly ActivityEvent[], issueCode: (issueId: string) => string | undefined): string {
  const parts: string[] = [];
  const updates = events.filter(event => !event.closed).length;
  if (updates) parts.push(`${updates} issue ${updates === 1 ? "update" : "updates"}`);
  for (const event of events) if (event.closed) parts.push(`${issueCode(event.issueId) ?? "Issue"} closed as ${event.closed}`);
  return parts.join(", ");
}

export const COLLAPSE_AFTER = 3;
