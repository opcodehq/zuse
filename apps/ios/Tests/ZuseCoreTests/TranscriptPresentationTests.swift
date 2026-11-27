import Foundation
import Testing

@testable import ZuseCore

// MARK: - Legacy records

private func legacy(
  _ id: String, _ kind: String, role: String = "assistant", item: String? = nil,
  parent: String? = nil, text: String = ""
) -> ChatMessage {
  var content: [String: JSONValue] = ["_tag": .string(kind), "text": .string(text)]
  if let item { content["itemId"] = .string(item) }
  if let parent { content["parentItemId"] = .string(parent) }
  return ChatMessage(
    id: id, sessionId: "session", role: role, content: .object(content),
    createdAt: "2026-10-18T00:00:00Z")
}

private func activity(_ items: [TimelineItem]) -> TimelineActivity? {
  for item in items { if case .activity(let value) = item { return value } }
  return nil
}
private func reply(_ items: [TimelineItem]) -> TimelineReply? {
  for item in items { if case .reply(let value) = item { return value } }
  return nil
}
private func users(_ items: [TimelineItem]) -> [TimelineUser] {
  items.compactMap { if case .user(let value) = $0 { value } else { nil } }
}

@Test func legacyToolOutputPairsByItemAndFoldsIntoOneActivity() {
  let items = ChatTimeline.project([
    legacy("ask", "user", role: "user", text: "Check it"),
    legacy("intro", "assistant", text: "Looking."),
    legacy("read", "tool_use", item: "a"), legacy("run", "tool_use", item: "b"),
    legacy("out-b", "tool_result", item: "b"), legacy("out-a", "tool_result", item: "a"),
    legacy("answer", "assistant", text: "All good."),
    legacy("usage", "usage"),
  ])
  #expect(items.map(\.id) == ["ask", "ask:activity", "ask:reply"])
  let work = activity(items)
  #expect(work?.steps.map(\.id) == ["intro", "read", "run"])
  #expect(work?.steps.map(\.kind) == [.narration, .tool, .tool])
  #expect(work?.steps[1].result?.id == "out-a")
  #expect(work?.steps[2].result?.id == "out-b")
  #expect(reply(items)?.text == "All good.")
}

@Test func legacyOutputScopedToAnotherParentStaysUnpaired() {
  let items = ChatTimeline.project([
    legacy("call", "tool_use", item: "a", parent: "one"),
    legacy("other", "tool_result", item: "a", parent: "two"),
  ])
  let work = activity(items)
  #expect(work?.steps.map(\.id) == ["call", "other"])
  #expect(work?.steps[0].result == nil)
}

@Test func streamedOutputKeepsTheRowIdentity() {
  let ask = legacy("ask", "user", role: "user", text: "Go")
  let call = legacy("call", "tool_use", item: "a")
  let before = ChatTimeline.project([ask, call], running: true)
  let after = ChatTimeline.project(
    [ask, call, legacy("out", "tool_result", item: "a", text: "done")], running: true)
  #expect(before.map(\.id) == after.map(\.id))
  #expect(before != after)
  #expect(activity(before)?.steps[0].running == true)
  #expect(activity(after)?.steps[0].running == false)
}

@Test func legacyUserAttachmentsAndGeneratedUISurvive() {
  var rich = legacy("rich", "user_rich", role: "user", text: "Look")
  rich.content = rich.content.setting(
    "attachments",
    to: .array([
      .object([
        "id": .string("att-1"), "mimeType": .string("image/png"),
        "originalName": .string("screen.png"),
      ])
    ]))
  let items = ChatTimeline.project([rich, legacy("spec", "ui_spec")])
  #expect(users(items).first?.attachments.first?.isImage == true)
  #expect(items.last == .detail(legacy("spec", "ui_spec")))
}

// MARK: - Canonical entries

private func entry(
  _ id: String, _ role: String, _ parts: [JSONValue], status: String = "complete",
  duration: Int? = nil, created: Int = 1_795_765_131_000
) -> JSONValue {
  var value: [String: JSONValue] = [
    "id": .string(id), "role": .string(role), "parts": .array(parts),
    "createdAt": .number(Double(created)), "deviceId": .string("device"),
    "status": .string(status),
  ]
  if let duration { value["durationMs"] = .number(Double(duration)) }
  return .object(value)
}
private func part(_ id: String, _ kind: String, _ fields: [String: JSONValue] = [:]) -> JSONValue {
  var value = fields
  value["id"] = .string(id)
  value["kind"] = .string(kind)
  return .object(value)
}
private func exec(_ id: String, _ command: String, resolved: Bool = true, error: Bool = false)
  -> JSONValue
{
  part(
    id, "tool",
    [
      "call": .object(["kind": .string("exec"), "command": .string(command)]),
      "resolved": .bool(resolved), "isError": .bool(error),
      "outputRef": .string("chat/\(id)"), "outputBytes": .number(440),
    ])
}
private func project(_ entries: [JSONValue], running: Bool = false) throws -> [TimelineItem] {
  var fold = CanonicalTranscript()
  try fold.apply(.object(["reset": .array(entries)]))
  return ChatTimeline.project(fold.messages(sessionID: "chat"), running: running)
}

/// The shape a real Claude Code run produced through `WatchDocMessages` (anonymized).
@Test func canonicalRunBecomesBubbleActivityAndReply() throws {
  let items = try project([
    entry(
      "u1", "user",
      [
        part("t0", "text", ["text": .string("List the files and add a line to README.md.")]),
        part(
          "agent-context", "agentContext",
          [
            "agent": .object(["harness": .string("claude-code"), "model": .string("claude-x-5")]),
            "previousAgent": .null,
          ]),
      ]),
    entry(
      "a1", "assistant",
      [
        exec("tool-1", "ls -la; cat README.md"),
        exec("tool-2", "printf 'line' >> README.md"),
        part("t2", "text", ["text": .string("Done.\n\n| File | Change |\n|---|---|\n| a | b |")]),
      ], duration: 13_229),
  ])
  #expect(items.map(\.id) == ["u1:t0", "u1:t0:activity", "u1:t0:reply"])
  #expect(users(items).first?.text == "List the files and add a line to README.md.")
  let work = try #require(activity(items))
  #expect(!work.live)
  #expect(work.title == "Worked for 13s · Ran 2 commands")
  #expect(work.steps.map(\.tool.label) == ["Ran", "Ran"])
  #expect(work.steps[0].outputRef == "chat/tool-1")
  #expect(work.steps[0].outputBytes == 440)
  #expect(reply(items)?.text.hasPrefix("Done.") == true)
  #expect(reply(items)?.streaming == false)
}

@Test func reasoningAndNarrationFoldWhileTheFinalTextIsTheReply() throws {
  let items = try project([
    entry("u1", "user", [part("t", "text", ["text": .string("Fix it")])]),
    entry(
      "a1", "assistant",
      [
        part("r1", "reasoning", ["text": .string("Consider the parser.")]),
        part("r-empty", "reasoning", ["text": .string("  ")]),
        part("n1", "text", ["text": .string("Let me look.")]),
        part(
          "e1", "tool",
          [
            "call": .object(["kind": .string("editFile"), "path": .string("src/a.rs")]),
            "resolved": .bool(true), "isError": .bool(false),
            "diffStats": .array([
              .object([
                "path": .string("src/a.rs"), "additions": .number(3), "deletions": .number(1),
              ])
            ]),
          ]),
        part("t2", "text", ["text": .string("Fixed the parser.")]),
        part("err", "error", ["message": .string("Rate limited")]),
      ], duration: 65_000),
  ])
  let work = try #require(activity(items))
  #expect(work.steps.map(\.kind) == [.thinking, .narration, .tool])
  #expect(work.steps[2].additions == 3 && work.steps[2].deletions == 1)
  #expect(work.title == "Worked for 1m 5s · Edited 1 file")
  #expect(reply(items)?.text == "Fixed the parser.")
  guard case .notice(let notice) = items.last else {
    Issue.record("Expected the error after the reply")
    return
  }
  #expect(notice.kind == .error && notice.title == "Rate limited")
}

@Test func runningTurnStreamsTheReplyAndNamesTheCurrentStep() throws {
  let user = entry("u1", "user", [part("t", "text", ["text": .string("Test it")])])
  let waiting = try project([user], running: true)
  #expect(waiting.map(\.id) == ["u1:t", "u1:t:waiting"])

  let items = try project(
    [
      user,
      entry(
        "a1", "assistant",
        [
          exec("done", "swift build"),
          exec("live", "swift test", resolved: false),
        ], status: "streaming"),
    ])
  let work = try #require(activity(items))
  #expect(work.live)
  #expect(work.title == "Running swift test")
  #expect(work.steps.last?.running == true)
  #expect(work.steps.last?.tool.label == "Running")
  #expect(work.worked == nil)

  let streaming = try project(
    [
      user,
      entry(
        "a1", "assistant",
        [exec("done", "swift build"), part("t", "text", ["text": .string("All tests")])],
        status: "streaming"),
    ])
  #expect(reply(streaming)?.streaming == true)
  #expect(reply(streaming)?.text == "All tests")
}

@Test func userAttachmentsComeFromTheTrailerAndImageParts() throws {
  let items = try project([
    entry(
      "u1", "user",
      [
        part(
          "t", "text",
          [
            "text": .string(
              "What is this?\n\nAttached images (local files — open them to view):\n- /uploads/0a1b2c3d-cat photo.png\n- /uploads/notes.md"
            )
          ]),
        part(
          "i", "image",
          [
            "path": .string("/uploads/shot.jpg"), "name": .string("shot.jpg"),
            "mimeType": .string("image/jpeg"),
          ]),
      ])
  ])
  let user = try #require(users(items).first)
  #expect(users(items).count == 1)
  #expect(user.text == "What is this?")
  #expect(user.attachments.map(\.name) == ["cat photo.png", "notes.md", "shot.jpg"])
  #expect(user.attachments.map(\.isImage) == [true, false, true])
  #expect(user.attachments[0].id == "/uploads/0a1b2c3d-cat photo.png")
}

@Test func transportOnlyPromptsHideAndProseTrailersStay() {
  let (hidden, files) = TimelineAttachment.split(
    "Please review the attached files.\n\nAttached images (local files — open them to view):\n- /u/a.png"
  )
  #expect(hidden.isEmpty && files.count == 1)
  let prose = "Notes\n\nAttached images (local files — see):\nnot a list"
  #expect(TimelineAttachment.split(prose).0 == prose)
}

@Test func agentChangesForksAndSubagentsAreNamed() throws {
  let items = try project([
    entry(
      "f", "system",
      [part("fork", "fork", ["sourceChatId": .string("c"), "sourceTitle": .string("Old chat")])]),
    entry(
      "u1", "user",
      [
        part(
          "ctx", "agentContext",
          [
            "agent": .object([
              "harness": .string("codex"), "model": .string("gpt-5-codex"),
            ]),
            "previousAgent": .object([
              "harness": .string("claude-code"), "model": .string("claude-sonnet-4-5-20250929"),
            ]),
          ]),
        part("t", "text", ["text": .string("Continue")]),
      ]),
    entry(
      "a1", "assistant",
      [
        part(
          "spawn", "tool",
          [
            "call": .object(["kind": .string("unknown"), "name": .string("Agent: scan repo")]),
            "resolved": .bool(true), "isError": .bool(false),
            "subagentStatus": .string("running"), "subagentTail": .string("Reading\n files"),
            "subagentRef": .string("doc-1"),
          ])
      ], status: "streaming"),
  ])
  let notices = items.compactMap { if case .notice(let n) = $0 { n } else { nil } }
  #expect(notices.map(\.kind) == [.fork, .modelSwitch])
  #expect(notices[0].detail == "Old chat")
  #expect(notices[1].title == "Switched from Sonnet 4.5 to GPT-5 Codex")
  let step = try #require(activity(items)?.steps.first)
  #expect(step.subagent == TimelineSubagent(status: .running, tail: "Reading files", reference: "doc-1"))
  #expect(step.running)
  #expect(step.tool.detail == "scan repo")
}

@Test func timedTextOnlyTurnStillShowsHowLongItWorked() throws {
  let items = try project([
    entry("u1", "user", [part("t", "text", ["text": .string("Hi")])]),
    entry("a1", "assistant", [part("t", "text", ["text": .string("Hello")])], duration: 900),
  ])
  #expect(activity(items)?.title == "Worked for 1s")
  #expect(activity(items)?.steps.isEmpty == true)
}

@Test func textDiffShowsChangesWithContext() {
  let lines = TextDiff.lines(
    old: "a\nb\nc\nd\ne\nf\ng", new: "a\nb\nc\nD\ne\nf\ng", context: 1)
  #expect(lines.map(\.kind) == [.context, .deletion, .addition, .context])
  #expect(lines.filter { $0.kind == .addition }.map(\.text) == ["D"])
  #expect(TextDiff.lines(old: "", new: "x\ny").map(\.kind) == [.addition, .addition])
}
