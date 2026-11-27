import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { IssueActivity } from "@/core/zapper-types";
import { deliveryNote, phrase, runState, summary, timeline } from "./timeline";

const event = (id: string, kind: string, changes?: IssueActivity["changes"]): IssueActivity => ({ id, kind, text: id, createdAt: 0, actor: null, changes });
const status = (id: string, to: string) => event(id, "updated", [{ field: "status", before: "todo", after: to }]);

describe("issue timeline", () => {
  test("folds quiet runs but never discussion or attention", () => {
    const items = timeline([
      event("created", "created"),
      status("s1", "inProgress"),
      event("t1", "thread", [{ field: "thread", after: "w" }]),
      event("c1", "comment"),
      status("s2", "inReview"),
      status("blocked", "blocked"),
      status("s3", "inProgress"),
    ]);
    assert.deepEqual(items.map(i => i.kind === "group" ? "group:" + i.events.length : i.kind + ":" + i.event.id), ["group:3", "card:c1", "event:s2", "event:blocked", "event:s3"]);
  });
  test("summaries count only recorded facts", () => {
    const text = summary([event("created", "created"), status("s", "inProgress"), event("pr", "pullRequest", [{ field: "pullRequest", after: "https://github.com/a/zuse/pull/12" }])], () => "zuse#12");
    assert.equal(text, "2 issue updates, linked zuse#12, now In progress");
  });
  test("phrases treat stored values as text", () => {
    const names: Record<string, string> = { user: "Swaraj", w: "Deploy thread" };
    const name = (id: string) => names[id] ?? id;
    assert.equal(phrase(event("e", "updated", [{ field: "owner", before: "user", after: "w" }]), name, u => u)?.action, "assigned Deploy thread and unassigned Swaraj");
    assert.equal(phrase(event("o", "updated", [{ field: "status", after: "<b>x</b>" }]), name, u => u)?.action, "moved the issue to an unknown status");
    assert.equal(phrase(event("old", "updated"), name, u => u), undefined);
    assert.equal(phrase(event("l", "updated", [{ field: "labels", before: "web", after: "web, deploy" }]), name, u => u)?.action, "set labels to web, deploy");
  });
  test("only a fresh session confirms a run", () => {
    const now = 1_000_000;
    assert.equal(runState({ status: "working", updatedAt: now - 1000 }, now), "running");
    assert.equal(runState({ status: "working", updatedAt: now - 60_000 }, now), "unconfirmed");
    assert.equal(runState({ status: "idle", updatedAt: now }, now), undefined);
    assert.equal(runState(undefined, now), undefined);
  });
  test("sent is not answered", () => {
    assert.equal(deliveryNote({ chatId: "w", state: "sent" }, "W").text, "Sent to W");
    assert.deepEqual(deliveryNote({ chatId: "w", state: "held", detail: "This issue is closed" }, "W"), { text: "Not sent to W · This issue is closed", attention: true });
  });
});
