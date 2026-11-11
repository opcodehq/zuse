// Applies host transcript frames in place, so a streamed token touches one text node.
import { produce, reconcile, unwrap, type SetStoreFunction } from "solid-js/store";
import type { MessagePart, SessionMessageEntry, TranscriptFrame } from "./protocol";

export class TranscriptDesync extends Error {}

const encoder = new TextEncoder();
const utf8Length = (text: string) => encoder.encode(text).length;
// UTF-8 lengths of streaming parts, so each token measures only itself.
const partBytes = new Map<string, number>();

/** Index of `id`, searched from the end where live entries are. */
function lastIndex(rows: readonly SessionMessageEntry[], id: string) {
  for (let i = rows.length - 1; i >= 0; i--) if (rows[i]!.id === id) return i;
  return -1;
}

type StreamPart = Extract<MessagePart, { kind: "text" | "reasoning" }>;
const findStreamPart = (entry: SessionMessageEntry | undefined, id: string) =>
  entry?.parts.find((p): p is StreamPart => (p.kind === "text" || p.kind === "reasoning") && p.id === id);

/**
 * Apply one frame. Throws TranscriptDesync when the host's tripwires disagree; the caller resubscribes.
 * Like the host's `apply_transcript_frame`, the whole frame is validated before anything is written: a throw from
 * inside a store write would leave Solid observers stale, freezing rows that later writes never repaint.
 */
export function applyTranscriptFrame(
  setMessages: SetStoreFunction<SessionMessageEntry[]>,
  current: readonly SessionMessageEntry[],
  frame: TranscriptFrame,
) {
  if ("reset" in frame) {
    partBytes.clear();
    setMessages(reconcile(frame.reset, { key: "id" }));
    return;
  }
  // Validate against raw rows: proxied reads over thousands of entries cost more than the token itself.
  const raw = unwrap(current) as SessionMessageEntry[];
  const remove = new Set(frame.remove ?? []);
  const upserts = frame.upsert ?? [];
  const appends = frame.append ?? [];
  const upserted = new Map<string, SessionMessageEntry>();
  let length = raw.length;
  if (remove.size || upserts.length) {
    // Project the frame like the host does (`validate_delta`), so later upserts may anchor on earlier ones.
    const ids = raw.map((entry) => entry.id).filter((id) => !remove.has(id));
    for (const { after, entry } of upserts) {
      const existing = ids.lastIndexOf(entry.id);
      if (existing >= 0) ids.splice(existing, 1);
      const at = after === null ? 0 : ids.lastIndexOf(after) + 1;
      if (after !== null && at === 0) throw new TranscriptDesync(`missing anchor ${after}`);
      ids.splice(at, 0, entry.id);
      upserted.set(entry.id, entry);
    }
    for (const append of appends)
      if (ids.lastIndexOf(append.entry) < 0) throw new TranscriptDesync(`missing append entry ${append.entry}`);
    length = ids.length;
  } else {
    for (const append of appends)
      if (lastIndex(raw, append.entry) < 0) throw new TranscriptDesync(`missing append entry ${append.entry}`);
  }
  if (length !== frame.count) throw new TranscriptDesync(`count mismatch: ${length} != ${frame.count}`);
  // Byte totals each appended part will have, checked against the host's `len` before the store changes.
  const totals = new Map<string, number>();
  for (const append of appends) {
    const fresh = upserted.get(append.entry);
    const part = findStreamPart(fresh ?? raw[lastIndex(raw, append.entry)], append.part);
    if (!part) throw new TranscriptDesync(`missing append part ${append.part}`);
    const key = `${append.entry}#${append.part}`;
    const before = totals.get(key) ?? (fresh ? undefined : partBytes.get(key)) ?? utf8Length(part.text);
    const bytes = before + utf8Length(append.text);
    if (bytes !== append.len) throw new TranscriptDesync(`append length mismatch on ${append.part}`);
    totals.set(key, bytes);
  }
  setMessages(
    produce((messages) => {
      const rows = unwrap(messages) as SessionMessageEntry[];
      if (remove.size) {
        for (let i = messages.length - 1; i >= 0; i--) if (remove.has(rows[i]!.id)) messages.splice(i, 1);
      }
      for (const { after, entry } of upserts) {
        for (const part of entry.parts) partBytes.delete(`${entry.id}#${part.id}`);
        const existing = lastIndex(rows, entry.id);
        if (existing >= 0) messages.splice(existing, 1);
        const at = after === null ? 0 : lastIndex(rows, after) + 1;
        messages.splice(at, 0, entry);
      }
      for (const append of appends) {
        const part = findStreamPart(messages[lastIndex(rows, append.entry)], append.part)!;
        part.text += append.text;
      }
    }),
  );
  for (const [key, bytes] of totals) partBytes.set(key, bytes);
}
