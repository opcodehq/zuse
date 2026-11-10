// Dragging rows between Pinned, custom sections and the regular list (crates/ui shell/spaces.rs
// finish_sidebar_session_transfer). Only pin and section membership change; regular rows keep
// their activity order.
import { createSignal } from "solid-js";
import type { Zone } from "./model";
import { dragging, setDragging } from "./overlay";
import { changePins, changeSection, pinnedIds, placePin, sections } from "./pins";

const MIME = "application/x-zuse-chat";

export type DropTarget =
  | { kind: "row"; chatId: string; zone: Zone }
  | { kind: "pinned" }
  | { kind: "section"; id: string }
  | { kind: "regular" };

type Position = "before" | "after" | "into";
const [indicator, setIndicator] = createSignal<{ key: string; position: Position } | null>(null);

const keyOf = (target: DropTarget) => (target.kind === "row" ? `row:${target.chatId}` : target.kind === "section" ? `section:${target.id}` : target.kind);

export function startDrag(event: DragEvent, chatId: string) {
  if (!event.dataTransfer) return;
  event.dataTransfer.setData(MIME, chatId);
  event.dataTransfer.setData("text/plain", chatId);
  event.dataTransfer.effectAllowed = "move";
  // Chromium cancels a drag whose dragstart changes the DOM, so drop zones appear a frame later.
  pending = requestAnimationFrame(() => {
    pending = 0;
    setDragging(chatId);
  });
}

let pending = 0;

export function endDrag() {
  if (pending) cancelAnimationFrame(pending);
  pending = 0;
  setDragging(null);
  setIndicator(null);
}

function position(event: DragEvent, target: DropTarget): Position {
  if (target.kind !== "row" || target.zone.kind === "regular") return "into";
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  return event.clientY < rect.top + rect.height / 2 ? "before" : "after";
}

function accepts(target: DropTarget) {
  const id = dragging();
  return !!id && !(target.kind === "row" && target.chatId === id && target.zone.kind !== "regular");
}

/** Spread onto any element that takes drops. The indicator class follows the pointer. */
export function dropHandlers(target: () => DropTarget) {
  return {
    get classList() {
      const current = indicator();
      const mine = current && current.key === keyOf(target());
      return { "sb-drop-before": !!mine && current.position === "before", "sb-drop-after": !!mine && current.position === "after", "sb-drop-into": !!mine && current.position === "into" };
    },
    onDragOver: (event: DragEvent) => {
      const t = target();
      if (!accepts(t)) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
      const next = { key: keyOf(t), position: position(event, t) };
      const current = indicator();
      if (current?.key !== next.key || current.position !== next.position) setIndicator(next);
    },
    onDragLeave: (event: DragEvent) => {
      const related = event.relatedTarget as Node | null;
      if (related && (event.currentTarget as HTMLElement).contains(related)) return;
      if (indicator()?.key === keyOf(target())) setIndicator(null);
    },
    onDrop: (event: DragEvent) => {
      const t = target();
      const id = dragging();
      if (!id || !accepts(t)) return;
      event.preventDefault();
      const where = position(event, t);
      endDrag();
      drop(id, t, where);
    },
  };
}

function drop(chatId: string, target: DropTarget, where: Position) {
  const pins = pinnedIds();
  const section = sections().find((s) => s.sessionIds.includes(chatId));
  if (target.kind === "pinned") return placePin(chatId, null, pins.find((id) => id !== chatId) ?? null);
  if (target.kind === "section") {
    const first = sections().find((s) => s.id === target.id)?.sessionIds.find((id) => id !== chatId) ?? null;
    return changeSection({ action: "assign", sessionId: chatId, sectionId: target.id, before: first });
  }
  const zone = target.kind === "row" ? target.zone : ({ kind: "regular" } as Zone);
  const anchor = target.kind === "row" ? target.chatId : null;
  if (zone.kind === "pinned") {
    return placePin(chatId, where === "after" ? anchor : null, where === "before" ? anchor : null);
  }
  if (zone.kind === "section") {
    return changeSection({
      action: "assign",
      sessionId: chatId,
      sectionId: zone.id,
      after: where === "after" ? anchor : null,
      before: where === "before" ? anchor : null,
    });
  }
  if (pins.includes(chatId)) changePins({ action: "unpin", sessionId: chatId });
  if (section) changeSection({ action: "assign", sessionId: chatId, sectionId: null });
}
