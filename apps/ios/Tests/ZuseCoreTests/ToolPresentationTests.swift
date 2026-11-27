import Foundation
import Testing

@testable import ZuseCore

private func tool(
  _ call: [String: JSONValue], id: String = UUID().uuidString, resolved: Bool = true,
  error: Bool = false, extra: [String: JSONValue] = [:]
) -> ChatMessage {
  var content: [String: JSONValue] = [
    "_tag": .string("tool_use"), "tool": .string("x"), "input": .object(call),
    "resolved": .bool(resolved), "isError": .bool(error),
  ]
  content.merge(extra) { $1 }
  return ChatMessage(
    id: id, sessionId: "s", role: "assistant", content: .object(content), createdAt: "")
}
private func steps(_ messages: [ChatMessage], live: Bool = false) -> [TimelineStep] {
  messages.map { TimelineStep(kind: .tool, message: $0, result: nil, live: live) }
}
private func exec(_ command: String = "cargo test") -> [String: JSONValue] {
  ["kind": .string("exec"), "command": .string(command)]
}

@Test func typedCallsAreNamedLikeTheDesktop() {
  func step(_ call: [String: JSONValue], running: Bool = false) -> ToolStep {
    ToolPresentation.step(tool(call), running: running)
  }
  #expect(step(exec()).label == "Ran")
  #expect(step(exec(), running: true).label == "Running")
  #expect(step(exec()).detail == "cargo test")
  #expect(step(["kind": .string("readFile"), "path": .string("a.rs")]).label == "Read")
  #expect(step(["kind": .string("writeFile"), "path": .string("a.rs")]).label == "Wrote")
  #expect(step(["kind": .string("glob"), "pattern": .string("*.rs")]).label == "Find files")
  #expect(
    step(["kind": .string("search"), "pattern": .string("foo"), "path": .string("src")]).detail
      == "foo in src")
  #expect(step(["kind": .string("applyPatch")]).detail == "workspace")
  #expect(
    step(["kind": .string("mcp"), "server": .string("linear"), "tool": .string("find")]).detail
      == "linear · find")
  let agent = step(["kind": .string("unknown"), "name": .string("Agent: scan repo")])
  #expect(agent.label == "Agent")
  #expect(agent.detail == "scan repo")
  #expect(agent.category == .agent)
}

@Test func commandsCollapseOntoOneLine() {
  #expect(ToolPresentation.step(tool(exec("a\n  b\tc"))).detail == "a b c")
}

@Test func legacyToolsAreNamedToo() {
  let message = ChatMessage(
    id: "m", sessionId: "s", role: "assistant",
    content: .object([
      "_tag": .string("tool_use"), "tool": .string("Edit"),
      "input": .object([
        "file_path": .string("a.rs"), "old_string": .string("a"), "new_string": .string("b"),
      ]),
    ]), createdAt: "")
  let step = ToolPresentation.step(message)
  #expect(step.label == "Edited")
  #expect(step.detail == "a.rs")
  let diff = TimelineStep(kind: .tool, message: message, result: nil, live: false).diff
  #expect(diff == TimelineDiff(path: "a.rs", old: "a", new: "b"))
}

@Test func summaryMatchesTheDesktopLine() {
  let edit: [String: JSONValue] = ["kind": .string("editFile"), "path": .string("a.rs")]
  let write: [String: JSONValue] = ["kind": .string("writeFile"), "path": .string("b.rs")]
  let read: [String: JSONValue] = ["kind": .string("readFile"), "path": .string("b.rs")]
  let line = ToolPresentation.summary(
    steps([
      tool(exec()), tool(exec(), error: true), tool(edit), tool(edit), tool(write), tool(read),
      tool(read),
    ]))
  #expect(line == "Ran 2 commands · edited 1 file · wrote 1 file · read 1 file · 1 failed")
}

@Test func summaryCapsCategoriesAndCountsFailedEditsAsTools() {
  let calls: [[String: JSONValue]] = [
    exec(), ["kind": .string("readFile"), "path": .string("a")],
    ["kind": .string("search"), "pattern": .string("x")],
    ["kind": .string("webFetch"), "url": .string("u")],
    ["kind": .string("webSearch"), "query": .string("q")],
  ]
  #expect(
    ToolPresentation.summary(steps(calls.map { tool($0) }))
      == "Ran 1 command · read 1 file · ran 1 search · fetched 1 page · +1")
  let failedEdit = tool(["kind": .string("editFile"), "path": .string("a")], error: true)
  #expect(ToolPresentation.summary(steps([failedEdit])) == "Used 1 tool · 1 failed")
}

@Test func liveTitleNamesTheNewestUnsettledCall() {
  let read: [String: JSONValue] = ["kind": .string("readFile"), "path": .string("a.rs")]
  let live = steps([tool(read), tool(exec(), resolved: false)], live: true)
  #expect(ToolPresentation.liveTitle(live) == "Running cargo test")
  let reading = steps([tool(read, resolved: false)], live: true)
  #expect(ToolPresentation.liveTitle(reading) == "Reading files…")
  #expect(ToolPresentation.liveTitle([]) == "Thinking…")
}

@Test func durationsReadLikeTheDesktop() {
  #expect(ToolPresentation.duration(milliseconds: 300) == "1s")
  #expect(ToolPresentation.duration(milliseconds: 9_400) == "9s")
  #expect(ToolPresentation.duration(milliseconds: 60_000) == "1m 0s")
  #expect(ToolPresentation.duration(milliseconds: 3_600_000) == "1h 0m")
  #expect(ToolPresentation.duration(milliseconds: 90_000_000) == "1d 1h")
}

@Test func agentLabelsMatchTheDesktop() {
  func label(_ harness: String, _ model: String?) -> String {
    AgentLabel.label(
      .object(["harness": .string(harness), "model": model.map(JSONValue.string) ?? .null]))
  }
  #expect(label("claude-code", "claude-opus-4-5") == "Opus 4.5")
  #expect(label("claude-code", "anthropic/claude-sonnet-4-5-20250929") == "Sonnet 4.5")
  #expect(label("codex", "gpt-5-codex") == "GPT-5 Codex")
  #expect(label("claude-code", nil) == "Claude Code")
  #expect(label("cursor", "composer-1") == "composer-1")
}
