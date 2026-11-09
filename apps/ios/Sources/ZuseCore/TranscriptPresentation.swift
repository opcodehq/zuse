import Foundation

/// Pair tool output by the protocol item identity while retaining unmatched output.
public struct TranscriptRow: Identifiable, Equatable, Sendable {
  public let message: ChatMessage
  public let result: ChatMessage?
  public var id: String { message.id }
  public let joinsPrevious: Bool
  public let joinsNext: Bool
  public var isActivity: Bool { ["thinking", "tool_use", "tool_result"].contains(message.kind) }

  public static func project(_ messages: [ChatMessage]) -> [TranscriptRow] {
    func key(_ message: ChatMessage) -> String? {
      guard let item = message.content["itemId"].string else { return nil }
      return (message.content["parentItemId"].string ?? "") + ":" + item
    }
    var outputs: [String: (Int, ChatMessage)] = [:]
    for (index, message) in messages.enumerated() where message.kind == "tool_result" {
      if let key = key(message) { outputs[key] = (index, message) }
    }
    var paired: [String: ChatMessage] = [:]
    var consumed = Set<String>()
    for (index, message) in messages.enumerated() where message.kind == "tool_use" {
      if let key = key(message), let (outputIndex, output) = outputs[key], outputIndex > index,
        consumed.insert(output.id).inserted
      {
        paired[message.id] = output
      }
    }
    // Usage and context accounting are metadata, not conversation content.
    let visible = messages.filter {
      !consumed.contains($0.id) && !["usage", "context_usage"].contains($0.kind)
    }
    func activity(_ index: Int) -> Bool {
      visible.indices.contains(index)
        && ["thinking", "tool_use", "tool_result"].contains(visible[index].kind)
    }
    return visible.enumerated().map { index, message in
      TranscriptRow(
        message: message, result: paired[message.id],
        joinsPrevious: activity(index) && activity(index - 1),
        joinsNext: activity(index) && activity(index + 1))
    }
  }
}

/// Consecutive activity shares a heading; narrative messages remain independently recycled.
public struct TranscriptGroup: Identifiable, Equatable, Sendable {
  public let rows: [TranscriptRow]
  public var id: String { rows[0].id }
  public var isActivity: Bool { rows[0].isActivity }
  public var toolCount: Int { rows.filter { $0.message.kind == "tool_use" }.count }
  public static func project(_ messages: [ChatMessage]) -> [TranscriptGroup] {
    var groups: [[TranscriptRow]] = []
    for row in TranscriptRow.project(messages) {
      if row.isActivity, groups.last?.last?.isActivity == true {
        groups[groups.count - 1].append(row)
      } else {
        groups.append([row])
      }
    }
    return groups.map { TranscriptGroup(rows: $0) }
  }
}
