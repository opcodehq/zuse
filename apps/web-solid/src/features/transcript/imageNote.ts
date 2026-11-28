import type { SessionMessageEntry } from "@/core/protocol";

/**
 * The note Claude Code adds after every image the model reads (`[Image: original WxH, displayed at …]` and its
 * `source:` spellings). Older subagent transcripts saved it as a user message; it is the CLI talking to the model,
 * so it draws nothing. Mirrors `zuse_protocol::presentation::image_note`.
 */
export function isImageNote(entry: SessionMessageEntry) {
  if (entry.role !== "user" || !entry.parts.length || entry.parts.some((p) => p.kind !== "text")) return false;
  const text = entry.parts.map((p) => (p.kind === "text" ? p.text : "")).join("\n\n").trim();
  if (text.includes("\n") || !text.endsWith("]")) return false;
  if (text.startsWith("[Image source: ")) return true;
  if (!text.startsWith("[Image: ")) return false;
  const body = text.slice("[Image: ".length);
  return body.startsWith("source: ")
    || (body.startsWith("original ") && body.includes(", displayed at ") && body.endsWith(" to map to original image.]"));
}
