import Foundation
import Testing

@testable import ZuseCore

/// Acceptance against an isolated engine that already hosts a Zapper with an open decision.
/// Set `ZUSE_ZAPPER_PAIRING_LINK` to a fresh `zuse pair --json` app link to run it. It pauses
/// the first Zapper so no coordinator turn starts, answers one decision and leaves it paused.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["ZUSE_ZAPPER_PAIRING_LINK"] != nil))
struct ZapperLiveTests {
  @Test func pairedClientWatchesAndCommandsZappers() async throws {
    let link = try #require(ProcessInfo.processInfo.environment["ZUSE_ZAPPER_PAIRING_LINK"])
    let rpc = try RPCClient(pairingLink: link)
    try await rpc.connect()
    defer { Task { await rpc.close() } }
    let info = try await rpc.call("ControlInfo")
    #expect(info["capabilities"].array.contains(.string(ZapperProtocol.capability)))

    var updates = try await rpc.stream("WatchZappers").makeAsyncIterator()
    let first = Zapper.snapshot(try #require(try await updates.next()))
    var zapper = try #require(first.first)
    let call: @Sendable (String, [String: JSONValue]) async throws -> JSONValue = {
      try await rpc.call($0, $1)
    }

    // Pausing first keeps the rest of the check from waking the coordinator.
    if zapper.status != .paused {
      let pause = ZapperCommand(
        zapperId: zapper.id, expectedRevision: zapper.revision, action: .setStatus(.paused))
      zapper = try #require(try await ZapperCommands.send(pause, call: call))
      #expect(zapper.status == .paused)
      // An identical retry returns the original result from the engine's receipt.
      let replay = try #require(try await ZapperCommands.send(pause, call: call))
      #expect(replay.revision == zapper.revision)
    }
    let pushed = Zapper.snapshot(try #require(try await updates.next()))
    #expect(pushed.first { $0.id == zapper.id }?.status == .paused)

    // A whole-Zapper command at an old revision is a definite, stale rejection.
    do {
      _ = try await ZapperCommands.send(
        ZapperCommand(
          zapperId: zapper.id, expectedRevision: zapper.revision - 1, action: .setStatus(.paused)),
        call: call)
      Issue.record("expected a revision conflict")
    } catch {
      guard case .staleRevision = error else {
        Issue.record("expected staleRevision, got \(error)")
        return
      }
    }

    if var decision = zapper.decisions.first(where: { $0.answer == nil }) {
      decision.answer = "Yes, link it from the README."
      let answered = try #require(
        try await ZapperCommands.send(
          ZapperCommand(
            zapperId: zapper.id, expectedRevision: zapper.revision, action: .decide(decision)),
          call: call))
      #expect(answered.decisions.first { $0.id == decision.id }?.answer == decision.answer)
      #expect(answered.needsYouCount == zapper.needsYouCount - 1)
    }

    let files = try await rpc.call(
      "ZapperFiles", ["zapperId": .string(zapper.id), "history": .bool(true)]
    ).decode([ZapperFile].self)
    #expect(files.contains { $0.path == "INSTRUCTIONS.md" && $0.version == 1 })
    let events = try await rpc.call("ZapperEvents", ["zapperId": .string(zapper.id)])
      .decode([ZapperEvent].self)
    let ack = try await rpc.call(
      "AckZapperEvents",
      ["zapperId": .string(zapper.id), "eventIds": .array(events.map { .string($0.id) })])
    #expect(ack["ok"] == .bool(true))
  }
}
