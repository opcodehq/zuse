// `rows_for_entry` for the DOM: an entry's parts become display items, keyed so streaming never remounts them.
import type { MessagePart, SessionMessageEntry } from "@/core/protocol";
import { callOf, isAgentItem, isWorkflowRun, type ToolItem, type ToolPart } from "./model";

type Part<K extends MessagePart["kind"]> = Extract<MessagePart, { kind: K }>;

export type Item =
  | { kind: "group"; key: string; tools: ToolItem[]; live: boolean }
  | { kind: "fold"; key: string; tools: ToolItem[]; body: Item[]; live: boolean }
  | { kind: "text"; key: string; part: Part<"text"> }
  | { kind: "image"; key: string; part: Part<"image"> }
  | { kind: "visual"; key: string; part: Part<"visual"> }
  | { kind: "uiBlock"; key: string; part: Part<"uiBlock"> }
  | { kind: "input"; key: string; part: Part<"input"> }
  | { kind: "error"; key: string; part: Part<"error"> }
  | { kind: "fork"; key: string; part: Part<"fork"> }
  | { kind: "workflow"; key: string; part: ToolPart };

const isApproval = (part: Part<"input">) =>
  (part.questions as { approvalPrompt?: unknown }[]).some((q) => q.approvalPrompt);

/** A settled assistant turn folds its work under one header, leaving only the reply. */
export const foldsTurn = (entry: SessionMessageEntry) => entry.role === "assistant" && entry.status !== "streaming";

/**
 * Build display items. `filled[i]` says whether text/reasoning part `i` has visible text; it is passed in
 * so this never reads streamed text and reruns only when parts change shape.
 */
export function buildItems(
  id: string,
  parts: readonly MessagePart[],
  filled: readonly boolean[],
  streaming: boolean,
  compact: boolean,
): Item[] {
  const items: Item[] = [];
  const last = parts.length - 1;
  let replyStart: number | null = null;
  if (compact && !streaming) {
    let start = -1;
    for (let i = last; i >= 0; i--) if (parts[i]!.kind === "text" && filled[i]) { start = i; break; }
    if (start >= 0) {
      while (start > 0 && parts[start - 1]!.kind === "text" && filled[start - 1]) start--;
      replyStart = start;
    }
  }
  let group: ToolItem[] = [];
  let groupIx = 0;
  let groupLast = 0;
  let foldAt: number | null = null;
  const flush = () => {
    if (!group.length) return;
    items.push({ kind: "group", key: `${id}#g${groupIx++}`, tools: group, live: streaming && groupLast === last });
    group = [];
  };
  parts.forEach((part, ix) => {
    const key = `${id}#${part.id}`;
    switch (part.kind) {
      case "tool": {
        const call = callOf(part);
        if (part.resolved && !part.isError && call.kind === "mcp" && call.server === "zuse" && call.tool === "show_ui") return;
        if (isWorkflowRun(call)) {
          if (!compact) flush();
          items.push({ kind: "workflow", key, part });
          return;
        }
        const item: ToolItem = { key: part.id, kind: "call", part };
        if (compact) foldAt ??= items.length;
        else if (group[0] && isAgentItem(group[0]) !== isAgentItem(item)) flush();
        group.push(item);
        groupLast = ix;
        return;
      }
      case "reasoning": {
        if (!filled[ix]) return;
        if (compact) foldAt ??= items.length;
        else if (group[0] && isAgentItem(group[0])) flush();
        group.push({ key: part.id, kind: "thought", part });
        groupLast = ix;
        return;
      }
      case "agentContext":
        return;
      default:
        break;
    }
    if (compact && part.kind === "text" && filled[ix] && (replyStart === null || ix < replyStart)) {
      foldAt ??= items.length;
      group.push({ key: part.id, kind: "note", part });
      groupLast = ix;
      return;
    }
    if (!compact) flush();
    switch (part.kind) {
      case "text":
        if (filled[ix]) items.push({ kind: "text", key, part });
        return;
      case "image": items.push({ kind: "image", key, part }); return;
      case "visual": items.push({ kind: "visual", key, part }); return;
      case "uiBlock": items.push({ kind: "uiBlock", key, part }); return;
      case "error": items.push({ kind: "error", key, part }); return;
      case "fork": items.push({ kind: "fork", key, part }); return;
      case "input":
        // Approvals have their own surface above the composer.
        if (!isApproval(part)) items.push({ kind: "input", key, part });
        return;
    }
  });
  if (compact) {
    if (group.length) {
      const work: MessagePart[] = [];
      const workFilled: boolean[] = [];
      parts.forEach((part, ix) => {
        const keep =
          (part.kind === "tool" && !isWorkflowRun(callOf(part))) ||
          (part.kind === "reasoning" && filled[ix]) ||
          (part.kind === "text" && filled[ix] && (replyStart === null || ix < replyStart));
        if (keep) {
          work.push(part);
          workFilled.push(filled[ix]!);
        }
      });
      const body = buildItems(id, work, workFilled, streaming, false);
      items.splice(foldAt ?? items.length, 0, { kind: "fold", key: `${id}#work`, tools: group, body, live: streaming });
    }
    // A folded turn's visuals follow its reply, outside the fold.
    const trailing = new Set<Item>(items.filter((item) => item.kind === "visual" || item.kind === "uiBlock"));
    if (trailing.size) return [...items.filter((item) => !trailing.has(item)), ...trailing];
  } else flush();
  return items;
}
