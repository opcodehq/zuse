import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { SessionMessageEntry } from "@/core/protocol";
import { isImageNote } from "./imageNote";

const user = (text: string): SessionMessageEntry =>
  ({ id: "u1", role: "user", parts: [{ kind: "text", id: "t0", text }], createdAt: 0, deviceId: "dev" }) as SessionMessageEntry;

describe("isImageNote", () => {
  test("matches every spelling Claude Code ships", () => {
    for (const note of [
      "[Image source: /tmp/a.png]",
      "[Image: source: /tmp/a.png]",
      "[Image: original 1290x2796, displayed at 923x2000. Multiply coordinates by 1.40 to map to original image.]",
      "[Image: source: /tmp/a.png, original 1576x2152, displayed at 1465x2000. Multiply coordinates by 1.08 to map to original image.]",
    ]) assert.equal(isImageNote(user(note)), true, note);
  });

  test("leaves real messages alone", () => {
    for (const text of ["Look at this [Image: source: /tmp/a.png]", "[Image: source: /tmp/a.png]\n\nfix the header", "[Image: the logo]", ""])
      assert.equal(isImageNote(user(text)), false, text);
    assert.equal(isImageNote({ ...user("[Image: source: /tmp/a.png]"), role: "assistant" } as SessionMessageEntry), false);
  });
});
