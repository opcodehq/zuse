import assert from "node:assert/strict";
import { test } from "node:test";
import { threadMatches, threadTone } from "./threads-model";

test("queued workers keep the queued tone even when a session is working", () => {
  assert.equal(threadTone({ queuePosition: 2 }, "working"), "queued");
});

test("session status maps onto a thread tone", () => {
  assert.equal(threadTone({}, "working"), "working");
  assert.equal(threadTone({}, "awaitingInput"), "needs");
  assert.equal(threadTone({}, "errored"), "failed");
  assert.equal(threadTone({}, "idle"), "idle");
  assert.equal(threadTone({}, undefined), "idle");
});

test("thread search matches every word across title, objective, and repository", () => {
  const fields = ["Prepare release notes", "Ship the changelog", "zuse"];
  assert.equal(threadMatches("", fields), true);
  assert.equal(threadMatches("   ", fields), true);
  assert.equal(threadMatches("  release   changelog ", fields), true);
  assert.equal(threadMatches("zuse notes", fields), true);
  assert.equal(threadMatches("missing", fields), false);
  assert.equal(threadMatches("release missing", fields), false);
});
