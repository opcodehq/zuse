import Foundation
import Testing

@testable import ZuseCore

/// Records every envelope and replays scripted replies, like an engine with receipts.
private actor Engine {
  var sent: [[String: JSONValue]] = []
  var replies: [Result<JSONValue, ClientError>]
  init(_ replies: [Result<JSONValue, ClientError>]) { self.replies = replies }
  func call(_ method: String, _ payload: [String: JSONValue]) throws -> JSONValue {
    #expect(method == "ZapperCommand")
    sent.append(payload)
    return try replies.removeFirst().get()
  }
}

private let zapperJSON: JSONValue = .object([
  "id": .string("z"), "revision": .number(8),
  "config": .object([
    "name": .string("Ops"), "purpose": .string(""), "homeDeviceId": .string("d"),
    "repositories": .array([]),
    "coordinatorModel": .object(["harness": .string("codex"), "model": .null]),
    "workerDeviceId": .string("d"), "maxConcurrentWorkers": .number(1),
    "maxFollowups": .number(0),
  ]),
  "status": .string("paused"), "coordinatorChatId": .string("zapper-z-0"),
  "previousCoordinatorChats": .array([]), "workers": .array([]), "decisions": .array([]),
])

/// No waiting between retries in tests.
private let instant: @Sendable (Int) -> Duration = { _ in .zero }

private func command() -> ZapperCommand {
  ZapperCommand(zapperId: "z", expectedRevision: 7, action: .setStatus(.paused))
}

@Test func ambiguousFailuresRetryTheIdenticalEnvelope() async throws {
  let engine = Engine([
    .failure(
      ClientError("The request timed out. Its outcome may be unknown; reconnect before retrying.")),
    .failure(ClientError("transport closed")),
    .success(zapperJSON),
  ])
  let result = try await ZapperCommands.send(command(), backoff: instant) {
    try await engine.call($0, $1)
  }
  #expect(result?.status == .paused && result?.revision == 8)
  let sent = await engine.sent
  #expect(sent.count == 3)
  #expect(Set(sent.compactMap { $0["commandId"]?.string }).count == 1)
  #expect(sent.allSatisfy { $0 == sent[0] })
  #expect(sent[0]["expectedRevision"] == .number(7))
}

@Test func definiteRejectionsAreNotRetried() async throws {
  let engine = Engine([
    .failure(
      ClientError(
        "Zapper revision conflict: expected 7, current revision is 9. setStatus needs the exact current revision."
      ))
  ])
  await #expect(throws: ZapperCommandError.self) {
    _ = try await ZapperCommands.send(command(), backoff: instant) {
      try await engine.call($0, $1)
    }
  }
  #expect(await engine.sent.count == 1)
  do {
    _ = try await ZapperCommands.send(command(), backoff: instant) { _, _ in
      throw ClientError("Zapper is not active")
    }
    Issue.record("expected a rejection")
  } catch {
    #expect(error == .rejected("Zapper is not active"))
    #expect(error.errorDescription == "Zapper is not active.")
  }
}

@Test func unconfirmedAfterEveryAttemptKeepsTheEnvelopeRetryable() async throws {
  let engine = Engine(
    Array(repeating: .failure(ClientError("The connection is unavailable.")), count: 3))
  let pending = command()
  do {
    _ = try await ZapperCommands.send(pending, backoff: instant) {
      try await engine.call($0, $1)
    }
    Issue.record("expected an unconfirmed outcome")
  } catch {
    guard case .unconfirmed = error else {
      Issue.record("expected unconfirmed, got \(error)")
      return
    }
  }
  #expect(await engine.sent.count == 3)
  // Deleting returns no Zapper.
  let deleted = try await ZapperCommands.send(pending, backoff: instant) { _, _ in .null }
  #expect(deleted == nil)
}

@Test func outcomeClassificationMatchesTheRustSignals() {
  #expect(ZapperCommands.outcomeUnknown(ClientError("Connection closed by peer")))
  #expect(ZapperCommands.outcomeUnknown(URLError(.networkConnectionLost)))
  #expect(ZapperCommands.outcomeUnknown(CancellationError()))
  #expect(!ZapperCommands.outcomeUnknown(ClientError("worker conversation does not exist")))
  #expect(
    ZapperCommands.classify(ClientError("Zapper revision conflict: decision d1 changed"))
      == .staleRevision("Zapper revision conflict: decision d1 changed"))
}
