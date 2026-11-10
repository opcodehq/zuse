import Foundation
import Testing

@testable import ZuseCore

private func activity(
  _ id: String, _ kind: String, item: String? = nil, parent: String? = nil, text: String = ""
) -> ChatMessage {
  var content: [String: JSONValue] = ["_tag": .string(kind), "text": .string(text)]
  if let item { content["itemId"] = .string(item) }
  if let parent { content["parentItemId"] = .string(parent) }
  return ChatMessage(
    id: id, sessionId: "session", role: "assistant", content: .object(content),
    createdAt: "2026-10-18T00:00:00Z")
}

@Test func transcriptRowsPairInterleavedToolsAndPreserveOrder() {
  let read = activity("read", "tool_use", item: "a")
  let run = activity("run", "tool_use", item: "b")
  let outB = activity("out-b", "tool_result", item: "b")
  let outA = activity("out-a", "tool_result", item: "a")
  let rows = TranscriptRow.project([read, run, outB, outA, activity("answer", "assistant")])
  #expect(rows.map(\.id) == ["read", "run", "answer"])
  #expect(rows[0].result == outA)
  #expect(rows[1].result == outB)
  #expect(!rows[0].joinsPrevious && rows[0].joinsNext)
  #expect(rows[1].joinsPrevious && !rows[1].joinsNext)
}

@Test func transcriptRowsRetainUnmatchedAndParentScopedOutput() {
  let call = activity("call", "tool_use", item: "a", parent: "parent-one")
  let other = activity("other", "tool_result", item: "a", parent: "parent-two")
  let orphan = activity("orphan", "tool_result")
  let rows = TranscriptRow.project([call, other, orphan])
  #expect(rows.map(\.id) == ["call", "other", "orphan"])
  #expect(rows[0].result == nil)
}

@Test func transcriptRowsKeepCallIdentityWhenStreamedOutputArrives() {
  let call = activity("call", "tool_use", item: "a")
  let before = TranscriptRow.project([call])
  let output = activity("output", "tool_result", item: "a", text: "done")
  let after = TranscriptRow.project([call, output])
  #expect(before[0].id == after[0].id)
  #expect(before[0] != after[0])
  #expect(after[0].result == output)
}

@Test func transcriptGroupCountsCallsAndSeparatesNarrative() {
  let calls = [
    activity("read", "tool_use", item: "a"), activity("out", "tool_result", item: "a"),
    activity("run", "tool_use", item: "b"),
  ]
  let groups = TranscriptGroup.project(
    [activity("intro", "assistant")] + calls + [activity("answer", "assistant")])
  #expect(groups.count == 3)
  #expect(groups[1].toolCount == 2)
  #expect(groups[1].rows.count == 2)
  #expect(groups[1].id == "read")
}

@Test func transcriptRowsOmitUsageMetadata() {
  let rows = TranscriptRow.project([
    activity("answer", "assistant"), activity("usage", "usage"),
    activity("context", "context_usage"), activity("limit", "usage_limit"),
  ])
  #expect(rows.map(\.id) == ["answer", "limit"])
}
