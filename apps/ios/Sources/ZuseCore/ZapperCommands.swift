import Foundation

/// One `ZapperCommand` envelope. Its ID is minted once by the caller and reused verbatim for
/// every retry, so the engine's receipt makes an ambiguous retry idempotent. A changed action
/// always needs a new envelope: reusing an ID with different input fails.
public struct ZapperCommand: Sendable, Hashable, Identifiable {
  public let commandId: String
  public let zapperId: String
  public let expectedRevision: Int?
  public let action: ZapperAction
  public var id: String { commandId }
  public init(
    zapperId: String, expectedRevision: Int?, action: ZapperAction,
    commandId: String = UUID().uuidString.lowercased()
  ) {
    self.commandId = commandId
    self.zapperId = zapperId
    self.expectedRevision = expectedRevision
    self.action = action
  }
  public func payload() throws -> [String: JSONValue] {
    var payload: [String: JSONValue] = [
      "commandId": .string(commandId), "zapperId": .string(zapperId), "action": try action.json(),
    ]
    if let expectedRevision { payload["expectedRevision"] = .number(Double(expectedRevision)) }
    return payload
  }
}

public enum ZapperCommandError: Error, LocalizedError, Equatable {
  /// The Zapper changed after the revision the command was based on. Nothing was written;
  /// act again on the refreshed state with a new command.
  case staleRevision(String)
  /// The engine definitely refused the command. Nothing was written.
  case rejected(String)
  /// The request or its answer was lost. It may have been applied; resend the same envelope.
  case unconfirmed(String)

  public var errorDescription: String? {
    switch self {
    case .staleRevision:
      return
        "This Zapper changed while you were looking at it. Check the latest state and try again."
    case .rejected(let message): return ZapperCommandError.readable(message)
    case .unconfirmed:
      return "Couldn’t confirm the change. Your computer may already have applied it."
    }
  }
  public var detail: String {
    switch self {
    case .staleRevision(let message), .rejected(let message), .unconfirmed(let message):
      return message
    }
  }
  /// Engine messages are written for agents; keep the first sentence and capitalize it.
  static func readable(_ message: String) -> String {
    let trimmed = message.trimmingCharacters(in: .whitespacesAndNewlines)
    let sentence = trimmed.split(separator: ":", maxSplits: 1).first.map(String.init) ?? trimmed
    guard let first = sentence.first else { return "The Zapper refused this change." }
    let text = first.uppercased() + sentence.dropFirst()
    return text.hasSuffix(".") ? text : text + "."
  }
}

public enum ZapperCommands {
  /// Mirrors `command_outcome_unknown` in the Rust protocol, plus the native client's own
  /// connection failures. Anything other than a reply from the engine counts as unknown.
  public static func outcomeUnknown(_ error: Error) -> Bool {
    guard let failure = error as? ClientError else { return true }
    let message = failure.message.lowercased()
    return [
      "transport", "connection closed", "host unavailable", "timed out", "timeout",
      "disconnected", "connection", "network", "offline", "outcome may be unknown",
    ].contains { message.contains($0) }
  }
  public static func classify(_ error: Error) -> ZapperCommandError {
    if let known = error as? ZapperCommandError { return known }
    let message = (error as? ClientError)?.message ?? error.localizedDescription
    if outcomeUnknown(error) { return .unconfirmed(message) }
    if message.lowercased().contains("revision conflict") { return .staleRevision(message) }
    return .rejected(message)
  }

  /// Sends `command`, retrying the identical envelope while the outcome is unknown. Returns
  /// the Zapper after the command (`nil` after `delete`).
  public static func send(
    _ command: ZapperCommand, attempts: Int = 3,
    backoff: @Sendable (Int) -> Duration = { .milliseconds(400 * (1 << min($0, 4))) },
    call: @Sendable (String, [String: JSONValue]) async throws -> JSONValue
  ) async throws(ZapperCommandError) -> Zapper? {
    let payload: [String: JSONValue]
    do { payload = try command.payload() } catch {
      throw .rejected("This change couldn’t be prepared: \(error.localizedDescription)")
    }
    var attempt = 0
    while true {
      do {
        let result = try await call("ZapperCommand", payload)
        if result == .null { return nil }
        do { return try result.decode(Zapper.self) } catch {
          // Applied, but this client can't read the reply; the watch stream catches up.
          return nil
        }
      } catch {
        let failure = classify(error)
        attempt += 1
        guard case .unconfirmed = failure, attempt < attempts, !Task.isCancelled else {
          throw failure
        }
        try? await Task.sleep(for: backoff(attempt))
      }
    }
  }
}
