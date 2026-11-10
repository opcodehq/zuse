import Foundation
import Testing

@testable import ZuseCore

private func json(_ text: String) throws -> JSONValue {
  try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
}
private func message(_ id: String, text: String = "Hello") -> JSONValue {
  .object([
    "id": .string(id), "sessionId": .string("s"), "role": .string("assistant"),
    "content": .object(["_tag": .string("assistant"), "text": .string(text)]),
    "createdAt": .string("2026-10-18T00:00:00Z"),
  ])
}
private func snapshot(_ messages: [JSONValue] = [], version: Int = 0) -> JSONValue {
  .object([
    "kind": .string("snapshot"), "sessionId": .string("s"),
    "throughVersion": .number(Double(version)),
    "cursor": .object(["epoch": .string("epoch"), "version": .number(Double(version))]),
    "projection": .object(["messages": .array(messages), "status": .string("idle")]),
  ])
}
private func event(_ version: Int, _ value: JSONValue) -> JSONValue {
  .object([
    "kind": .string("event"), "sessionId": .string("s"), "streamVersion": .number(Double(version)),
    "event": value,
  ])
}
@Test func replayIsIdempotentAndReplacesMessages() throws {
  var transcript = Transcript()
  try transcript.apply(snapshot([message("a")]), sessionID: "s")
  let update = event(
    1, .object(["_tag": .string("MessagePersisted"), "message": message("a", text: "Updated")]))
  try transcript.apply(update, sessionID: "s")
  try transcript.apply(update, sessionID: "s")
  #expect(transcript.messages.count == 1)
  #expect(transcript.messages[0].text == "Updated")
  #expect(transcript.version == 1)
}
@Test func gapDoesNotAdvanceCursor() throws {
  var transcript = Transcript()
  try transcript.apply(snapshot(), sessionID: "s")
  #expect(throws: ClientError.self) {
    try transcript.apply(event(2, .object(["_tag": .string("Noop")])), sessionID: "s")
  }
  #expect(transcript.version == 0)
}
@Test func rejectsCrossSessionAndReset() throws {
  var transcript = Transcript()
  #expect(throws: ClientError.self) { try transcript.apply(snapshot(), sessionID: "another") }
  #expect(throws: ClientError.self) {
    try transcript.apply(json("{\"kind\":\"reset-required\",\"sessionId\":\"s\"}"), sessionID: "s")
  }
}
@Test func pagesAreChronologicalAndDeduplicated() throws {
  var transcript = Transcript()
  try transcript.apply(snapshot([message("c")]), sessionID: "s")
  let page: JSONValue = .object([
    "messages": .array([message("c"), message("b"), message("a")]),
    "olderMessageSequence": .number(50),
  ])
  try transcript.prependPage(page)
  try transcript.prependPage(page)
  #expect(transcript.messages.map(\.id) == ["a", "b", "c"])
  #expect(transcript.olderSequence == 50)
}
@Test func rollbackRemovesOnlySelectedMessages() throws {
  var transcript = Transcript()
  try transcript.apply(snapshot([message("a"), message("b")]), sessionID: "s")
  try transcript.apply(
    event(
      1,
      .object([
        "_tag": .string("TranscriptRolledBack"), "removedMessageIds": .array([.string("a")]),
      ])), sessionID: "s")
  #expect(transcript.messages.map(\.id) == ["b"])
}
@Test func largeHistoryRetainsStableIDs() throws {
  var transcript = Transcript()
  try transcript.apply(snapshot((0..<10_000).map { message(String($0)) }), sessionID: "s")
  try transcript.apply(
    event(
      1, .object(["_tag": .string("MessagePersisted"), "message": message("9999", text: "Latest")])),
    sessionID: "s")
  #expect(transcript.messages.count == 10_000)
  #expect(transcript.messages.last?.text == "Latest")
}
@Test func diffTracksBothLineNumbersAndFileHeaders() {
  let lines = DiffLine.parse(
    "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -8,2 +8,2 @@\n same\n-old\n+new")
  #expect(lines[2].kind == .header)
  #expect(lines[4].oldLine == 8 && lines[4].newLine == 8)
  #expect(lines[5].oldLine == 9 && lines[5].newLine == nil)
  #expect(lines[6].newLine == 9 && lines[6].oldLine == nil)
}
@Test func unknownContentIsPreserved() throws {
  let value = try json(
    "{\"id\":\"x\",\"sessionId\":\"s\",\"role\":\"assistant\",\"content\":{\"_tag\":\"future\",\"payload\":[1,2]},\"createdAt\":\"now\"}"
  )
  let decoded = try value.decode(ChatMessage.self)
  #expect(decoded.kind == "future")
  #expect(decoded.text.contains("payload"))
}

@Test func malformedHunkDoesNotCrash() {
  let lines = DiffLine.parse("@@ - + @@\n+new")
  #expect(lines.count == 2)
}

@Test func snapshotChunkRequiresMatchingVersion() throws {
  var transcript = Transcript()
  let chunk: JSONValue = .object([
    "kind": .string("snapshot-chunk"), "sessionId": .string("s"), "throughVersion": .number(1),
    "messages": .array([]),
  ])
  #expect(throws: ClientError.self) { try transcript.apply(chunk, sessionID: "s") }
  try transcript.apply(snapshot(), sessionID: "s")
  #expect(throws: ClientError.self) { try transcript.apply(chunk, sessionID: "s") }
}

@Test func questionLifecycleAndTurnSettlement() throws {
  var transcript = Transcript()
  try transcript.apply(snapshot(), sessionID: "s")
  let content: JSONValue = .object([
    "_tag": .string("user_question"), "itemId": .string("question"),
    "questions": .array([
      .object(["question": .string("Proceed?"), "options": .array([.string("Yes")])])
    ]),
  ])
  let row = message("q").setting("content", to: content)
  try transcript.apply(
    event(1, .object(["_tag": .string("MessagePersisted"), "message": row])), sessionID: "s")
  #expect(transcript.interactions.count == 1)
  try transcript.apply(
    event(2, .object(["_tag": .string("QuestionExpired"), "itemId": .string("question")])),
    sessionID: "s")
  #expect(transcript.interactions[0]["recoveryState"].string == "expired")
  try transcript.apply(
    event(3, .object(["_tag": .string("QuestionResolved"), "itemId": .string("question")])),
    sessionID: "s")
  #expect(transcript.interactions.isEmpty)
  try transcript.apply(
    event(4, .object(["_tag": .string("StatusSet"), "status": .string("running")])), sessionID: "s")
  try transcript.apply(event(5, .object(["_tag": .string("TurnSettled")])), sessionID: "s")
  #expect(transcript.status == "idle")
}

@Test func cachedHistoryDeduplicatesIDsAndKeepsTheLatestContent() {
  let old = ChatMessage(
    id: "same", sessionId: "s", role: "assistant",
    content: .object(["_tag": .string("assistant"), "text": .string("old")]), createdAt: "")
  var updated = old
  updated.content = .object(["_tag": .string("assistant"), "text": .string("new")])
  let transcript = Transcript(cached: [old, updated])
  #expect(transcript.messages.count == 1)
  #expect(transcript.messages[0].text == "new")
}

@Suite struct CanonicalTranscriptTests {
  @Test func unicodeAppendAndInvalidDeltaAreTransactional() throws {
    var fold = CanonicalTranscript()
    let entry: JSONValue = .object([
      "id": .string("m"), "role": .string("assistant"),
      "parts": .array([
        .object(["id": .string("p"), "kind": .string("text"), "text": .string("hello")])
      ]),
    ])
    try fold.apply(.object(["reset": .array([entry])]))
    try fold.apply(
      .object([
        "append": .array([
          .object([
            "entry": .string("m"), "part": .string("p"), "text": .string("🌍"), "len": .number(9),
          ])
        ]), "count": .number(1),
      ]))
    #expect(fold.messages(sessionID: "chat").first?.text == "hello🌍")
    #expect(throws: ClientError.self) {
      try fold.apply(.object(["remove": .array([.string("m")]), "count": .number(1)]))
    }
    #expect(fold.messages(sessionID: "chat").first?.text == "hello🌍")
    #expect(fold.messages(sessionID: "other").first?.sessionId == "other")
  }

  @Test func missingResetAndUnknownAnchorsAreRejected() throws {
    var fold = CanonicalTranscript()
    #expect(throws: ClientError.self) { try fold.apply(.object(["count": .number(0)])) }
    try fold.apply(.object(["reset": .array([])]))
    #expect(throws: ClientError.self) {
      try fold.apply(
        .object([
          "upsert": .array([
            .object(["after": .string("missing"), "entry": .object(["id": .string("m")])])
          ]), "count": .number(1),
        ]))
    }
    #expect(fold.messages(sessionID: "chat").isEmpty)
  }
}

@Suite struct GitPatchTests {
  @Test func exactQuotedAndRenamedPaths() {
    let patch =
      "diff --git a/notfile b/notfile\n\ndiff --git a/file b/file\n--- a/file\n+++ b/file\n"
    #expect(GitPatch.file(patch, path: "file")?.hasPrefix("diff --git a/file b/file") == true)
    #expect(GitPatch.file("diff --git a/notfile b/notfile\n", path: "file") == nil)
    let quoted = "diff --git \"a/\\303\\251.txt\" \"b/\\303\\251.txt\"\n"
    #expect(GitPatch.file(quoted, path: "é.txt") == quoted)
    let rename = "diff --git a/old name b/new name\nrename from old name\nrename to new name\n"
    #expect(GitPatch.file(rename, path: "new name", oldPath: "old name") == rename)
  }
}

@Suite struct CanonicalRuntimeTests {
  @Test func freshWorkingAndWaitingStatusesExpireAndNeverCrossChats() {
    var transcript = Transcript()
    let now = Date(timeIntervalSince1970: 1_700_000_000)
    let formatter = ISO8601DateFormatter()
    let timestamp = formatter.string(from: now)
    let working: JSONValue = .object([
      "chatId": .string("chat"), "status": .string("working"), "updatedAt": .string(timestamp),
    ])
    transcript.applyCanonicalRuntime(working, sessionID: "chat", now: now)
    #expect(transcript.activeTurnID != nil)
    #expect(transcript.status == "working")
    transcript.applyCanonicalRuntime(working, sessionID: "chat", now: now.addingTimeInterval(46))
    #expect(transcript.activeTurnID == nil)
    #expect(transcript.status == "unknown")
    transcript.applyCanonicalRuntime(working, sessionID: "other", now: now)
    #expect(transcript.activeTurnID == nil)
    let idle: JSONValue = .object([
      "chatId": .string("chat"), "status": .string("idle"), "updatedAt": .string(timestamp),
    ])
    transcript.applyCanonicalRuntime(idle, sessionID: "chat", now: now)
    #expect(transcript.activeTurnID == nil)
    #expect(transcript.status == "idle")
  }
}
