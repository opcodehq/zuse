import Foundation

/// Transactional fold of the Rust host's reset, ordered upsert, removal and UTF-8 append frames.
public struct CanonicalTranscript: Sendable {
  private var entries: [JSONValue] = []
  private var initialized = false
  public init() {}

  public mutating func apply(_ update: JSONValue) throws {
    var next = entries
    if case .array(let reset) = update["reset"] {
      let ids = reset.compactMap { $0["id"].string }
      guard ids.count == reset.count, Set(ids).count == ids.count else {
        throw ClientError("Invalid transcript reset")
      }
      next = reset
    } else {
      guard initialized else { throw ClientError("Missing transcript reset") }
      let removed = Set(update["remove"].array.compactMap(\.string))
      next.removeAll { removed.contains($0["id"].string ?? "") }
      for upsert in update["upsert"].array {
        let entry = upsert["entry"]
        guard let id = entry["id"].string else {
          throw ClientError("Missing transcript entry identity")
        }
        next.removeAll { $0["id"].string == id }
        var position = 0
        if let anchor = upsert["after"].string {
          guard let index = next.firstIndex(where: { $0["id"].string == anchor }) else {
            throw ClientError("Missing transcript anchor")
          }
          position = index + 1
        }
        next.insert(entry, at: position)
      }
      for append in update["append"].array {
        guard let index = next.firstIndex(where: { $0["id"].string == append["entry"].string }),
          case .object(var entry) = next[index],
          case .array(var parts) = entry["parts"],
          let partIndex = parts.firstIndex(where: { $0["id"].string == append["part"].string }),
          case .object(var part) = parts[partIndex],
          ["text", "reasoning"].contains(part["kind"]?.string ?? ""),
          let previous = part["text"]?.string, let suffix = append["text"].string
        else { throw ClientError("Missing transcript append target") }
        let text = previous + suffix
        guard text.utf8.count == append["len"].int else {
          throw ClientError("Transcript append length mismatch")
        }
        part["text"] = .string(text)
        parts[partIndex] = .object(part)
        entry["parts"] = .array(parts)
        next[index] = .object(entry)
      }
      guard next.count == update["count"].int else {
        throw ClientError("Transcript count mismatch")
      }
    }
    entries = next
    initialized = true
  }

  public var interactions: [JSONValue] {
    entries.flatMap { $0["parts"].array }.filter {
      $0["kind"].string == "input" && !$0["resolved"].bool
    }.map {
      .object(["_tag": .string("Question"), "id": $0["requestId"], "questions": $0["questions"]])
    }
  }

  public func messages(sessionID: String) -> [ChatMessage] {
    entries.flatMap { entry in
      entry["parts"].array.compactMap { part -> ChatMessage? in
        guard let kind = part["kind"].string, !["agentContext", "input"].contains(kind),
          let entryID = entry["id"].string, let partID = part["id"].string,
          case .object(var content) = part
        else { return nil }
        content["_tag"] = .string(kind)
        if kind == "tool" {
          let call = part["call"]
          content["_tag"] = .string("tool_use")
          content["tool"] = .string(
            call["name"].string ?? call["tool"].string ?? call["kind"].string ?? "Tool")
          content["input"] = call
          let text = [
            call["command"].string, call["path"].string, call["pattern"].string, call["url"].string,
            call["query"].string, part["output"].string,
          ].compactMap { $0 }.joined(separator: "\n")
          content["text"] = .string(text.isEmpty ? call.pretty : text)
        }
        return ChatMessage(
          id: "\(entryID):\(partID)", sessionId: sessionID,
          role: entry["role"].string ?? "assistant", content: .object(content), createdAt: "")
      }
    }
  }
}
