import { describe, test } from "node:test";
import assert from "node:assert/strict";
import type { SessionMessageEntry } from "@/core/protocol";
import { agentHeadline, agentSummary, chatAgents } from "./agents";

const entry = (parts: SessionMessageEntry["parts"]): SessionMessageEntry => ({id:"a",role:"assistant",deviceId:"d",createdAt:0,parts});
const agent = (doc: string, status?: string) => ({kind:"tool" as const,id:doc,call:{kind:"unknown" as const,name:`Agent: ${doc}`},subagentRef:doc,subagentStatus:status});

describe("subagent presentation", () => {
  test("empty and non-navigation tools have no summary", () => {
    assert.deepEqual(chatAgents([entry([{kind:"tool",id:"ordinary",call:{kind:"unknown",name:"Read"},subagentRef:"not-agent"}, {...agent("missing"),subagentRef:undefined}])]), []);
    assert.equal(agentSummary([]), "");
  });
  test("latest status wins without duplicating a document or changing spawn order", () => {
    const agents = chatAgents([entry([agent("a","running"),agent("b","failed")]),entry([agent("a","done")])]);
    assert.deepEqual(agents.map(agent => agent.docId), ["a","b"]);
    assert.equal(agentSummary(agents), "1 failed · 1 complete");
    assert.deepEqual(agentHeadline(agents), { title: "2 subagents", detail: "1 failed · 1 complete" });
    assert.deepEqual(agentHeadline(agents.slice(0, 1)), { title: "Subagent", detail: "a · Complete" });
  });
  test("fallback status follows error and resolution, not the parent session", () => {
    const agents = chatAgents([entry([agent("running"),{...agent("done"),resolved:true},{...agent("error"),isError:true}])]);
    assert.equal(agentSummary(agents), "1 working · 1 failed · 1 complete");
  });
});
