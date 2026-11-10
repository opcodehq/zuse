import Foundation

public struct ChatMessage: Codable, Identifiable, Sendable, Equatable {
  public let id: String
  public let sessionId: String
  public let role: String
  public var content: JSONValue
  public let createdAt: String
  public var kind: String { content["_tag"].string ?? role }
  public var text: String {
    content["text"].string ?? content["message"].string ?? content["output"].string
      ?? content.pretty
  }
  public init(id: String, sessionId: String, role: String, content: JSONValue, createdAt: String) {
    self.id = id
    self.sessionId = sessionId
    self.role = role
    self.content = content
    self.createdAt = createdAt
  }
}

public struct Transcript: Sendable {
  public private(set) var messages: [ChatMessage] = []
  private var indices: [String: Int] = [:]
  public private(set) var version: Int = 0
  public private(set) var epoch: String?
  public private(set) var hasSnapshot = false
  public private(set) var olderSequence: Int?
  public private(set) var status = "idle"
  public private(set) var activeTurnID: String?
  public private(set) var interactions: [JSONValue] = []
  public init() {}
  public init(cached messages: [ChatMessage]) {
    merge(messages)
  }

  public mutating func replaceCanonical(_ rows: [ChatMessage], interactions: [JSONValue]) {
    messages = rows
    indices = Dictionary(uniqueKeysWithValues: rows.enumerated().map { ($0.element.id, $0.offset) })
    self.interactions = interactions
    hasSnapshot = true
    olderSequence = nil
  }

  public mutating func applyCanonicalRuntime(
    _ row: JSONValue, sessionID: String, now: Date = Date()
  ) {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    var updated = row["updatedAt"].string.flatMap { formatter.date(from: $0) }
    if updated == nil {
      formatter.formatOptions = [.withInternetDateTime]
      updated = row["updatedAt"].string.flatMap { formatter.date(from: $0) }
    }
    guard row["chatId"].string == sessionID, let updated,
      now.timeIntervalSince(updated) >= -5, now.timeIntervalSince(updated) <= 45
    else {
      status = "unknown"
      activeTurnID = nil
      return
    }
    status = row["status"].string ?? "unknown"
    activeTurnID = ["working", "awaitingInput"].contains(status) ? "canonical:" + sessionID : nil
  }

  public mutating func apply(_ frame: JSONValue, sessionID: String) throws {
    guard frame["sessionId"].string == sessionID else {
      throw ClientError("Received a frame for a different session.")
    }
    let kind = frame["kind"].string
    if kind == "reset-required" {
      throw ClientError("The transcript changed on the server. Reconnecting…")
    }
    let incomingEpoch = frame["cursor"]["epoch"].string
    if kind != "snapshot", let epoch, let incomingEpoch, epoch != incomingEpoch {
      throw ClientError("The runtime restarted. Reloading the transcript…")
    }
    switch kind {
    case "snapshot":
      let p = frame["projection"]
      let decoded = try p["messages"].decode([ChatMessage].self)
      messages = []
      indices = [:]
      merge(decoded)
      version = frame["throughVersion"].int ?? 0
      epoch = incomingEpoch
      hasSnapshot = true
      olderSequence = frame["olderMessageSequence"].int ?? p["olderMessageSequence"].int
      status = p["status"].string ?? "idle"
      activeTurnID = p["activeTurn"]["id"].string
      interactions = p["interactions"].array
    case "snapshot-chunk":
      guard hasSnapshot, frame["throughVersion"].int == version else {
        throw ClientError("Snapshot history arrived without its matching snapshot.")
      }
      merge(try frame["messages"].decode([ChatMessage].self), prepend: true)
      olderSequence = frame["olderMessageSequence"].int
    case "event":
      guard hasSnapshot, let next = frame["streamVersion"].int else {
        throw ClientError("Missing transcript snapshot.")
      }
      if next <= version { return }
      guard next == version + 1 else {
        throw ClientError("A transcript event was missed. Reconnecting…")
      }
      let event = frame["event"]
      switch event["_tag"].string {
      case "MessagePersisted":
        let message = try event["message"].decode(ChatMessage.self)
        let existed = indices[message.id] != nil
        merge([message])
        let content = message.content
        if let itemID = content["itemId"].string {
          if message.kind == "user_question_answer" { removeInteraction("Question", id: itemID) }
          if message.kind == "tool_result" { removeInteraction("Plan", id: itemID) }
          if !existed && message.kind == "user_question" {
            upsertInteraction(
              .object([
                "_tag": .string("Question"), "id": .string(itemID),
                "questions": content["questions"], "requestedAt": .string(message.createdAt),
              ]))
          }
          if !existed && message.kind == "tool_use" && content["tool"].string == "ExitPlanMode",
            !messages.contains(where: {
              $0.kind == "tool_result" && $0.content["itemId"].string == itemID
            })
          {
            upsertInteraction(
              .object([
                "_tag": .string("Plan"), "id": .string(itemID),
                "requestedAt": .string(message.createdAt),
              ]))
          }
        }
      case "StatusSet": status = event["status"].string ?? status
      case "TurnStarted", "TurnUpdated": activeTurnID = event["turn"]["id"].string
      case "TurnSettled":
        activeTurnID = nil
        status = "idle"
      case "TranscriptRolledBack":
        let removed = Set(event["removedMessageIds"].array.compactMap(\.string))
        messages.removeAll { removed.contains($0.id) }
        reindex()
      case "PermissionRequested":
        let request = event["request"]
        upsertInteraction(
          .object(["_tag": .string("Permission"), "id": request["id"], "request": request]))
      case "PermissionResolved":
        removeInteraction("Permission", id: event["requestId"].string ?? "")
      case "PermissionExpired":
        interactions = interactions.map { item in
          item["_tag"].string == "Permission" && item["id"] == event["requestId"]
            ? item.setting(
              "request", to: item["request"].setting("recoveryState", to: .string("expired")))
            : item
        }
      case "QuestionResolved": removeInteraction("Question", id: event["itemId"].string ?? "")
      case "QuestionExpired", "PlanExpired":
        let tag = event["_tag"].string == "QuestionExpired" ? "Question" : "Plan"
        interactions = interactions.map { item in
          item["_tag"].string == tag && item["id"] == event["itemId"]
            ? item.setting(
              "recoveryState",
              to: tag == "Question"
                ? .string("expired")
                : event["reason"].string.map(JSONValue.string) ?? .string("expired")) : item
        }
      default: break
      }
      version = next
    case "synchronized":
      guard frame["throughVersion"].int == version else {
        throw ClientError("Transcript synchronization is incomplete.")
      }
    case "runtime": status = frame["observation"]["state"].string ?? status
    default: throw ClientError("This app needs an update to read this transcript frame.")
    }
  }
  public mutating func prependPage(_ page: JSONValue) throws {
    merge(try page["messages"].decode([ChatMessage].self).reversed(), prepend: true)
    olderSequence = page["olderMessageSequence"].int
  }
  private mutating func merge(_ incoming: [ChatMessage], prepend: Bool = false) {
    if !prepend {
      for message in incoming {
        if let index = indices[message.id] {
          messages[index] = message
        } else {
          indices[message.id] = messages.count
          messages.append(message)
        }
      }
      return
    }
    var added: [ChatMessage] = []
    var positions: [String: Int] = [:]
    for message in incoming {
      if let index = indices[message.id] {
        messages[index] = message
      } else if let index = positions[message.id] {
        added[index] = message
      } else {
        positions[message.id] = added.count
        added.append(message)
      }
    }
    messages.insert(contentsOf: added, at: 0)
    reindex()
  }
  private mutating func upsertInteraction(_ value: JSONValue) {
    if let index = interactions.firstIndex(where: {
      $0["_tag"] == value["_tag"] && $0["id"] == value["id"]
    }) {
      interactions[index] = value
    } else {
      interactions.append(value)
    }
  }
  private mutating func removeInteraction(_ kind: String, id: String) {
    interactions.removeAll { $0["_tag"].string == kind && $0["id"].string == id }
  }
  private mutating func reindex() {
    indices = Dictionary(
      uniqueKeysWithValues: messages.enumerated().map { ($0.element.id, $0.offset) })
  }
}
