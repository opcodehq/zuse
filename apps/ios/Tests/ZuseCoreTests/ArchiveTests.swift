import Foundation
import Testing

@testable import ZuseCore

struct ArchiveTests {
  func directory() -> URL {
    FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  }
  let key = Data(repeating: 42, count: 32)

  @Test func restartPreservesDraftAndExactCommandIdentity() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let first = try ArchiveStore(directory: directory, key: key)
    let message = PendingMessage(sessionID: "session", text: "Do the work")
    try await first.enqueue(message)
    _ = try await first.markAttempted(message.id, sessionID: "session")
    try await first.saveDraft("Next message", sessionID: "session")
    let restarted = try ArchiveStore(directory: directory, key: key)
    let restored = try await restarted.load("session")
    #expect(restored.draft == "Next message")
    #expect(restored.pending.first?.id == message.id)
    #expect(restored.pending.first?.messageID == message.messageID)
    #expect(restored.pending.first?.delivery == .uncertain)
  }
  @Test func submittedAttachmentAndNewChatComposerRemainSeparateAfterRestart() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    let oldAttachment: JSONValue = .object(["id": .string("old-upload")])
    try await store.saveComposer("Send this", attachments: [oldAttachment], sessionID: "old")
    let message = PendingMessage(
      sessionID: "old", text: "Send this",
      input: .object(["text": .string("Send this"), "attachments": .array([oldAttachment])]))
    try await store.enqueue(message)
    let newAttachment: JSONValue = .object(["id": .string("new-upload")])
    try await store.saveComposer("Another task", attachments: [newAttachment], sessionID: "new")
    let restarted = try ArchiveStore(directory: directory, key: key)
    let old = try await restarted.load("old")
    let new = try await restarted.load("new")
    #expect(old.draft.isEmpty)
    #expect(old.attachments?.isEmpty == true)
    #expect(old.pending.first?.payload == message.payload)
    #expect(new.draft == "Another task")
    #expect(new.attachments == [newAttachment])
    #expect(new.pending.isEmpty)
  }
  @Test func wrongKeyCannotReadArchive() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let first = try ArchiveStore(directory: directory, key: key)
    try await first.saveDraft("private content", sessionID: "session")
    let other = try ArchiveStore(directory: directory, key: Data(repeating: 43, count: 32))
    await #expect(throws: (any Error).self) { _ = try await other.load("session") }
    let bytes = try Data(
      contentsOf: #require(
        FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
          .first))
    #expect(!String(decoding: bytes, as: UTF8.self).contains("private content"))
  }
  @Test func cancelledDraftAndCheckpointTasksCannotOverwriteNewerSavedState() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    try await store.saveDraft("Current draft", sessionID: "session")
    try await store.saveMetadata(.string("Current session"), sessionID: "session")
    for operation in 0..<3 {
      let cancelled = Task {
        withUnsafeCurrentTask { $0?.cancel() }
        switch operation {
        case 0: try await store.saveDraft("Obsolete draft", sessionID: "session")
        case 1: try await store.saveMetadata(.string("Obsolete session"), sessionID: "session")
        default:
          try await store.saveMessages(
            [
              ChatMessage(
                id: "obsolete", sessionId: "session", role: "user",
                content: .string("Obsolete checkpoint"), createdAt: "now")
            ], sessionID: "session")
        }
      }
      await #expect(throws: CancellationError.self) { try await cancelled.value }
    }
    let restarted = try ArchiveStore(directory: directory, key: key)
    let record = try await restarted.load("session")
    #expect(record.draft == "Current draft")
    #expect(record.metadata == .string("Current session"))
    #expect(record.messages.isEmpty)
  }
  @Test func observedMessageSettlesUncertainOutboxEntry() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    let pending = PendingMessage(sessionID: "session", text: "Hello")
    try await store.enqueue(pending)
    let message = ChatMessage(
      id: pending.messageID, sessionId: "session", role: "user",
      content: .object(["_tag": .string("user"), "text": .string("Hello")]), createdAt: "now")
    try await store.saveMessages([message], sessionID: "session")
    #expect(try await store.load("session").pending.isEmpty)
  }
  @Test func checkpointConfirmationDoesNotBlockTheRemainingOfflineQueue() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    let first = PendingMessage(sessionID: "session", text: "First")
    let second = PendingMessage(sessionID: "session", text: "Second")
    try await store.enqueue(first)
    try await store.enqueue(second)
    let queue = try await store.load("session").pending
    try await store.saveMessages(
      [
        ChatMessage(
          id: first.messageID, sessionId: "session", role: "user",
          content: .object(["_tag": .string("user"), "text": .string("First")]), createdAt: "now")
      ], sessionID: "session")
    var attempted: [PendingMessage] = []
    for message in queue {
      if let attempt = try await store.markAttempted(message.id, sessionID: "session") {
        attempted.append(attempt)
      }
    }
    #expect(attempted.count == 1)
    #expect(attempted.first?.payload == second.payload)
    #expect(attempted.first?.delivery == .uncertain)
    #expect(try await store.load("session").pending.map(\.id) == [second.id])
  }
  @Test func erasedArchiveCannotBeResurrectedByAnOldTask() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    try await store.saveDraft("private", sessionID: "session")
    try await store.erase()
    await #expect(throws: ClientError.self) {
      try await store.saveDraft("stale", sessionID: "session")
    }
    #expect(!FileManager.default.fileExists(atPath: directory.path))
  }
  @Test func formResponsesAreDurableAndPreserveComposerDraft() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    try await store.saveDraft("My draft", sessionID: "session")
    let message = PendingMessage(sessionID: "session", text: "answer")
    try await store.enqueue(message, responseTo: "form")
    let reopened = try ArchiveStore(directory: directory, key: key)
    #expect(try await reopened.load("session").draft == "My draft")
    await #expect(throws: ClientError.self) {
      try await reopened.enqueue(
        PendingMessage(sessionID: "session", text: "another answer"), responseTo: "form")
    }
  }
  @Test func editingDraftDoesNotRewriteLargeHistory() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    let messages = (0..<10_000).map { index in
      ChatMessage(
        id: String(index), sessionId: "session", role: "assistant",
        content: .object(["_tag": .string("assistant"), "text": .string("Message \(index)")]),
        createdAt: "now")
    }
    try await store.saveMessages(messages, sessionID: "session")
    let history = directory.appendingPathComponent(
      ArchiveStore.storageID("session") + ".sealed.history")
    let before = try Data(contentsOf: history)
    try await store.saveDraft("Next prompt", sessionID: "session")
    #expect(try Data(contentsOf: history) == before)
    let metadata = try Data(
      contentsOf: directory.appendingPathComponent(ArchiveStore.storageID("session") + ".sealed"))
    #expect(metadata.count < 1024)
    let restarted = try ArchiveStore(directory: directory, key: key)
    #expect(try await restarted.load("session").messages.count == 10_000)
    #expect(try await restarted.load("session").draft == "Next prompt")
  }
}

extension ArchiveTests {
  @Test func interactionRetryAfterRestartKeepsExactCommandAndDecision() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let first = try ArchiveStore(directory: directory, key: key)
    let fields: [String: JSONValue] = [
      "outcome": .string("approved"), "feedback": .string("Use this plan"),
    ]
    let response = try await first.prepareInteractionResponse(
      method: "session.plan.respond", itemID: "plan", sessionID: "session", fields: fields)
    let restarted = try ArchiveStore(directory: directory, key: key)
    let retry = try await restarted.prepareInteractionResponse(
      method: response.method, itemID: response.itemID, sessionID: response.sessionID,
      fields: fields)
    #expect(retry.payload == response.payload)
    #expect(retry.payload["toolCallId"]?.string == "plan")
    #expect(retry.payload["commandId"]?.string == response.commandID)
    await #expect(throws: ClientError.self) {
      _ = try await restarted.prepareInteractionResponse(
        method: response.method, itemID: response.itemID,
        sessionID: response.sessionID, fields: ["outcome": .string("cancelled")])
    }
    try await restarted.confirmInteractionResponse(retry)
    let reopened = try ArchiveStore(directory: directory, key: key)
    #expect(
      try await reopened.interactionResponse(
        method: response.method, itemID: response.itemID, sessionID: response.sessionID)?.confirmed
        == true)
  }

  @Test func questionResponsesStayEncryptedAndSeparateFromDraftWrites() async throws {
    let directory = directory()
    defer { try? FileManager.default.removeItem(at: directory) }
    let store = try ArchiveStore(directory: directory, key: key)
    let fields: [String: JSONValue] = [
      "answers": .array([
        .object([
          "questionIndex": .number(0),
          "selected": .array([.number(1)]), "other": .string("private response"),
        ])
      ])
    ]
    let response = try await store.prepareInteractionResponse(
      method: "session.answerQuestion", itemID: "question", sessionID: "session", fields: fields)
    #expect(response.payload["itemId"]?.string == "question")
    #expect(response.payload["toolCallId"] == nil)
    let files = try FileManager.default.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: nil)
    let file = try #require(files.first)
    let ciphertext = try Data(contentsOf: file)
    #expect(!String(decoding: ciphertext, as: UTF8.self).contains("private response"))
    try await store.saveDraft("New composer draft", sessionID: "session")
    #expect(try Data(contentsOf: file) == ciphertext)
    let other = try ArchiveStore(directory: directory, key: Data(repeating: 11, count: 32))
    await #expect(throws: (any Error).self) {
      _ = try await other.interactionResponse(
        method: response.method, itemID: response.itemID, sessionID: response.sessionID)
    }
    try await store.erase()
    await #expect(throws: ClientError.self) {
      _ = try await store.prepareInteractionResponse(
        method: response.method, itemID: response.itemID, sessionID: response.sessionID,
        fields: fields)
    }
    #expect(!FileManager.default.fileExists(atPath: directory.path))
  }
}
