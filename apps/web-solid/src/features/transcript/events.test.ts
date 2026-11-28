import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { Zapper, ZapperIssue } from "@/core/zapper-types";
import { activityEvents, slotEvents, summarize } from "./events";

const act = (id: string, createdAt: number, kind: string, text: string, actor: string | null = "coord") => ({ id, createdAt, kind, text, actor });
const issue = (id: string, number: number, threads: string[], activity: ZapperIssue["activity"]): ZapperIssue => ({
  id, number, createdAt: 0, updatedAt: 0, threads, pullRequests: [], activity,
  details: { title: id, description: "", status: "todo", priority: "normal", assigned: true, dueDate: null, labels: [], relations: [] },
});
const zapper: Zapper = {
  id: "z", revision: 1, activityRevision: 1, status: "active", coordinatorChatId: "coord", previousCoordinatorChats: [], decisions: [],
  workers: [{ chatId: "worker", deviceId: "d", spaceId: "s", objective: "", followups: 0 }],
  config: { name: "Website", purpose: "", homeDeviceId: "d", workerDeviceId: "d", repositories: [], coordinatorModel: { harness: "codex", model: null }, maxConcurrentWorkers: 1, maxFollowups: 1 },
  issues: [
    issue("a", 18, ["worker"], [
      act("1", 10, "created", "Created the issue", null),
      act("2", 20, "updated", "Owner: Website · Status changed to In progress · Priority changed to High"),
      act("3", 30, "thread", "Started thread · In progress"),
      act("4", 40, "pullRequest", "Linked pull request https://github.com/opcodehq/zuse/pull/378"),
      act("5", 50, "comment", "Looks good", "worker"),
      act("6", 60, "question", "Merge?"),
      act("7", 70, "updated", "Decision received · Status changed to Done", null),
      act("8", 80, "closed", "Merged · shipped"),
      act("9", 90, "workerReport", "Worker failed\nboom", "worker"),
    ]),
    issue("b", 19, [], [act("10", 15, "comment", "Other")]),
  ],
};

describe("activityEvents", () => {
  test("describes each recorded activity kind in words with issue and PR codes", () => {
    assert.deepEqual(activityEvents(zapper, "worker").map(event => [event.actor.kind, event.text]), [
      ["user", "created ZAP-18"],
      ["zapper", "assigned ZAP-18 to Website"],
      ["zapper", "moved ZAP-18 to In progress"],
      ["zapper", "set ZAP-18 priority to high"],
      ["zapper", "started work on ZAP-18"],
      ["zapper", "linked opcodehq/zuse#378 to ZAP-18"],
      ["thread", "commented on ZAP-18"],
      ["zapper", "asked you about ZAP-18"],
      ["user", "moved ZAP-18 to Done"],
      ["zapper", "closed ZAP-18 as merged"],
      ["thread", "failed on ZAP-18"],
    ]);
  });
  test("the coordinator sees every issue, a worker only its own, others nothing", () => {
    assert.equal(activityEvents(zapper, "coord").length, 12);
    assert.equal(activityEvents(zapper, "coord")[1]!.text, "commented on ZAP-19");
    assert.deepEqual(activityEvents(zapper, "elsewhere"), []);
    assert.deepEqual(activityEvents(undefined, "coord"), []);
  });
  test("split rows keep unique ids", () => {
    const ids = activityEvents(zapper, "coord").map(event => event.id);
    assert.equal(new Set(ids).size, ids.length);
  });
});

describe("slotEvents and summarize", () => {
  const events = activityEvents(zapper, "coord");
  test("events follow the last message at or before them; older ones are dropped", () => {
    const slots = slotEvents(events, [{ id: "m1", createdAt: 12 }, { id: "m2", createdAt: 45 }, { id: "m3", createdAt: 1000 }]);
    assert.deepEqual([...slots.keys()], ["m1", "m2"]);
    assert.deepEqual(slots.get("m1")!.map(event => event.at), [15, 20, 20, 20, 30, 40]);
    assert.equal(slots.get("m2")!.length, 5);
    assert.equal(slotEvents(events, []).size, 0);
    const outOfOrder = slotEvents(events.filter(event => event.at === 30), [{ id: "m10", createdAt: 10 }, { id: "m35", createdAt: 35 }, { id: "m20", createdAt: 20 }]);
    assert.deepEqual([...outOfOrder.keys()], ["m20"]);
  });
  test("the divider counts updates and names closures", () => {
    const code = (id: string) => (id === "a" ? "ZAP-18" : undefined);
    assert.equal(summarize(events, code), "11 issue updates, ZAP-18 closed as merged");
    assert.equal(summarize(events.slice(0, 1), code), "1 issue update");
  });
});
