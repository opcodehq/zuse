import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { issueAgentWorking } from "./issue-activity";
import type { IssueDetails, Zapper } from "@/core/zapper-types";

const details = { status: "inProgress", owner: { type: "thread", chatId: "worker" } } as IssueDetails;
const zapper = { workers: [{ chatId: "worker", deviceId: "device" }] } as Zapper;
describe("issue agent activity", () => {
  test("follows running to idle and completed transitions, not issue status", () => {
    for (const status of ["working", "idle", "completed", "awaitingInput", "errored", undefined]) {
      assert.equal(issueAgentWorking(details, zapper, true, () => status, () => true), status === "working");
    }
    assert.equal(issueAgentWorking({ ...details, status: "todo" }, zapper, true, () => "working", () => true), true);
  });
  test("suppresses blocked, attention and terminal issues even with stale activity", () => {
    for (const status of ["blocked", "needsYou", "done", "canceled"] as const) {
      assert.equal(issueAgentWorking({ ...details, status }, zapper, true, () => "working", () => true), false);
    }
  });
  test("requires a connected, online, assigned, nonqueued worker", () => {
    assert.equal(issueAgentWorking(details, zapper, false, () => "working", () => true), false);
    assert.equal(issueAgentWorking(details, zapper, true, () => "working", () => false), false);
    assert.equal(issueAgentWorking(details, { workers: [] } as unknown as Zapper, true, () => "working", () => true), false);
    assert.equal(issueAgentWorking(details, { workers: [{ ...zapper.workers[0]!, queuePosition: 1 }] } as Zapper, true, () => "working", () => true), false);
    assert.equal(issueAgentWorking({ ...details, owner: { type: "user" } }, zapper, true, () => "working", () => true), false);
    assert.equal(issueAgentWorking({ ...details, owner: { type: "zapper" } }, zapper, true, () => "working", () => true), false);
  });
});
