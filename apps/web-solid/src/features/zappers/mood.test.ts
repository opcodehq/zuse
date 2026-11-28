import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { Zapper } from "@/core/zapper-types";
import { moodFor } from "./mood";

const zapper = (over: Partial<Zapper> = {}) => ({ status: "active", decisions: [], issues: [], ...over }) as unknown as Zapper;

describe("Zapper mood", () => {
  test("follows the same order as desktop", () => {
    const active = zapper();
    assert.equal(moodFor(active, true, undefined, []), "awake");
    assert.equal(moodFor(active, true, "idle", ["idle", "completed"]), "awake");
    assert.equal(moodFor(active, true, "working", []), "working");
    assert.equal(moodFor(active, true, "idle", ["working"]), "delegating");
    assert.equal(moodFor(active, true, "working", ["working"]), "working");
    assert.equal(moodFor(active, true, "completed", []), "done");
    assert.equal(moodFor(active, true, "completed", ["working"]), "delegating");
    assert.equal(moodFor(active, true, "errored", ["working"]), "blocked");
    assert.equal(moodFor(active, true, "working", ["awaitingInput"]), "needsYou");
    assert.equal(moodFor(active, true, "errored", ["awaitingInput"]), "needsYou");
    const asking = zapper({ decisions: [{ id: "q", workerChatId: null, question: "Ship it?", answer: null }] as Zapper["decisions"] });
    assert.equal(moodFor(asking, true, "idle", []), "needsYou");
  });

  test("paused and offline Zappers never look busy", () => {
    assert.equal(moodFor(zapper({ status: "paused" }), true, "working", ["working"]), "resting");
    assert.equal(moodFor(zapper(), false, "working", ["awaitingInput"]), "offline");
    assert.equal(moodFor(zapper({ status: "stopped" }), false, undefined, []), "resting");
  });
});
