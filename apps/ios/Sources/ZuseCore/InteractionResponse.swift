import Foundation

/// A decision keeps its command identity and exact payload until the session records its resolution.
public struct InteractionResponse: Codable, Sendable, Equatable {
  public let commandID: String
  public let method: String
  public let itemID: String
  public let sessionID: String
  public let fields: [String: JSONValue]
  public var confirmed: Bool

  public init(method: String, itemID: String, sessionID: String, fields: [String: JSONValue]) {
    commandID = UUID().uuidString
    self.method = method
    self.itemID = itemID
    self.sessionID = sessionID
    self.fields = fields
    confirmed = false
  }
  public var key: String { Self.key(method: method, itemID: itemID) }
  public static func key(method: String, itemID: String) -> String { method + ":" + itemID }
  public var payload: [String: JSONValue] {
    var payload = fields
    payload["sessionId"] = .string(sessionID)
    payload["commandId"] = .string(commandID)
    payload[method == "session.plan.respond" ? "toolCallId" : "itemId"] = .string(itemID)
    return payload
  }
}
