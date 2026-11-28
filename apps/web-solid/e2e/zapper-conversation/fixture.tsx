// The issue conversation from the real component over an in-memory store, with commands held
// until the test resolves them and a fake clock. No engine, no paid services.
import type { IssueActivity, Zapper } from "@/core/zapper-types";
import { Conversation } from "@/features/zappers/Conversation";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { setState, state } from "@/core/store";
import { rpc } from "@/core/connection";
import "@/ui/tokens.css";
import "@/ui/primitives.css";

const worker = "zapper-worker-deploy";
const at = (minutes: number) => Date.now() - minutes * 60_000;
const entry = (id: string, kind: string, text: string, actor: string | null, minutes: number, extra: Partial<IssueActivity> = {}): IssueActivity => ({ id, kind, text, actor, createdAt: at(minutes), ...extra });
const zapper: Zapper = {
  id: "release", revision: 1, activityRevision: 0, status: "active", coordinatorChatId: "coordinator", previousCoordinatorChats: [],
  config: { name: "Release", purpose: "Ship the Solid web client.", homeDeviceId: "local", workerDeviceId: "local", repositories: [], coordinatorModel: { harness: "claude-code", model: null }, maxConcurrentWorkers: 3, maxFollowups: 2 },
  workers: [{ chatId: worker, deviceId: "local", spaceId: "repo", objective: "Deploy the Solid web client.", followups: 0, queuePosition: null }],
  decisions: [], folders: [],
  issues: [{
    id: "deploy", number: 1, createdAt: at(60), updatedAt: at(1), threads: [worker], pullRequests: [],
    details: { title: "Deploy the Solid web client", description: "", status: "inProgress", priority: "high", assigned: true, owner: { type: "thread", chatId: worker }, dueDate: null, labels: [], relations: [], doneCriteria: "", parent: null },
    activity: [
      entry("created", "created", "Created the issue", "coordinator", 50),
      entry("report-1", "workerReport", "Worker update\nShould code.zuse.sh use the proxy (A) or tokens (B)?", worker, 30),
    ],
  }],
} as unknown as Zapper;

let hold: ((value: Zapper) => void) | null = null;
const sent: unknown[] = [];
Object.assign(rpc, {
  call: (method: string, params: { action?: { type: string; text?: string; replyTo?: string } }) => {
    if (!method.includes("Command")) return Promise.reject(new Error("Fixture has no " + method));
    sent.push(params.action);
    return new Promise<Zapper>(resolve => { hold = resolve; });
  },
  subscribe: () => ({ cancel: () => {} }),
});
const mutate = (fn: (z: Zapper) => void) => { setState(draft => { fn(draft.zappers[0]!); draft.zappers[0]!.revision++; }); flush(); };
setState(draft => { draft.zappers = [structuredClone(zapper)]; draft.sessions = {}; });
flush();
Object.assign(window, {
  conv: {
    // Unrelated activity arriving, as a live update does.
    arrive: (id: string) => mutate(z => { z.issues[0]!.activity.push(entry(id, "updated", "Priority changed to High", "coordinator", 0, { changes: [{ field: "priority", before: "normal", after: "high" }] })); }),
    // The engine confirms the held command with the comment stored.
    confirm: () => {
      const action = sent.at(-1) as { text: string; replyTo?: string };
      const next = JSON.parse(JSON.stringify(state.zappers[0]!)) as Zapper;
      next.revision++;
      next.issues[0]!.activity.push(entry("comment-" + sent.length, "comment", action.text, null, 0, action.replyTo ? { replyTo: action.replyTo } : {}));
      hold?.(next); hold = null;
    },
    pending: () => hold !== null,
    sent: () => sent,
    session: (status: string) => { setState(draft => { draft.sessions[worker] = { chatId: worker, status, updatedAt: Date.now() } as never; }); flush(); },
  },
});
render(() => <Conversation zapper={state.zappers[0]!} issue={state.zappers[0]!.issues[0]!} />, document.getElementById("root")!);
