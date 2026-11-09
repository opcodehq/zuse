import Foundation
import Testing

@testable import ZuseCore

private func tool(
  _ call: [String: JSONValue], id: String = UUID().uuidString, resolved: Bool = true,
  error: Bool = false
) -> ChatMessage {
  ChatMessage(
    id: id, sessionId: "s", role: "assistant",
    content: .object([
      "_tag": .string("tool_use"), "tool": .string("x"), "input": .object(call),
      "resolved": .bool(resolved), "isError": .bool(error),
    ]), createdAt: "")
}
private func exec(_ command: String = "cargo test") -> [String: JSONValue] {
  ["kind": .string("exec"), "command": .string(command)]
}

@Test func typedCallsAreNamedLikeTheDesktop() {
  func step(_ call: [String: JSONValue]) -> ToolStep { ToolPresentation.step(tool(call)) }
  #expect(step(exec()).label == "Run")
  #expect(step(exec()).detail == "cargo test")
  #expect(step(["kind": .string("readFile"), "path": .string("a.rs")]).label == "Read")
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
}

@Test func commandsCollapseOntoOneLine() {
  #expect(ToolPresentation.step(tool(exec("a\n  b\tc"))).detail == "a b c")
}

@Test func legacyToolsAreNamedToo() {
  let message = ChatMessage(
    id: "m", sessionId: "s", role: "assistant",
    content: .object([
      "_tag": .string("tool_use"), "tool": .string("Edit"),
      "input": .object(["file_path": .string("a.rs")]),
    ]), createdAt: "")
  let step = ToolPresentation.step(message)
  #expect(step.label == "Edit")
  #expect(step.detail == "a.rs")
}

@Test func summaryMatchesTheDesktopLine() {
  let edit: [String: JSONValue] = ["kind": .string("editFile"), "path": .string("a.rs")]
  let write: [String: JSONValue] = ["kind": .string("writeFile"), "path": .string("a.rs")]
  let read: [String: JSONValue] = ["kind": .string("readFile"), "path": .string("b.rs")]
  let rows = TranscriptRow.project([
    tool(exec()), tool(exec(), error: true), tool(edit), tool(write), tool(read),
  ])
  #expect(
    ToolPresentation.summary(rows)
      == "Ran 2 commands · edited 1 file · read 1 file · 1 failed")
}

@Test func runningGroupsDescribeTheirCurrentTool() {
  let read: [String: JSONValue] = ["kind": .string("readFile"), "path": .string("a.rs")]
  let group = TranscriptGroup.project([tool(read), tool(exec(), resolved: false)]).first
  #expect(group?.hasRunningTool == true)
  #expect(group?.title == "Running cargo test")
  let settled = TranscriptGroup.project([tool(read)]).first
  #expect(settled?.hasRunningTool == false)
  #expect(settled?.title == "Read 1 file")
}
