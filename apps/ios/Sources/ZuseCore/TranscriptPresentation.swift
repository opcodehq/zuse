import Foundation

/// A file a message carries: an upload on a legacy host, a host path on a Rust host.
public struct TimelineAttachment: Identifiable, Equatable, Sendable {
  /// The attachment id (legacy) or host path (Rust), which `Attachments.read` accepts.
  public let id: String
  public let name: String
  public let mimeType: String
  public var isImage: Bool { mimeType.hasPrefix("image/") }
  /// The reference `AttachmentView` opens.
  public var reference: JSONValue {
    .object([
      "id": .string(id), "mimeType": .string(mimeType), "originalName": .string(name),
    ])
  }

  static func mimeType(for path: String) -> String {
    switch (path as NSString).pathExtension.lowercased() {
    case "png": "image/png"
    case "jpg", "jpeg": "image/jpeg"
    case "gif": "image/gif"
    case "heic": "image/heic"
    case "webp": "image/webp"
    case "pdf": "application/pdf"
    case "md", "txt", "log": "text/plain"
    case "json": "application/json"
    default: "application/octet-stream"
    }
  }

  /// Rust hosts list uploads after the prompt; the desktop hides that trailer the same way.
  static func split(_ text: String) -> (String, [TimelineAttachment]) {
    guard
      let start = text.range(
        of: "\n\nattached images (local files", options: [.caseInsensitive, .backwards])?
        .lowerBound
    else { return (text, []) }
    let trailer = text[start...].dropFirst(2)
    let lines = trailer.split(separator: "\n", omittingEmptySubsequences: true)
    guard let header = lines.first, header.trimmingCharacters(in: .whitespaces).hasSuffix("):")
    else { return (text, []) }
    var found: [TimelineAttachment] = []
    for line in lines.dropFirst() where !line.trimmingCharacters(in: .whitespaces).isEmpty {
      let trimmed = line.trimmingCharacters(in: .whitespaces)
      guard trimmed.hasPrefix("- ") else { return (text, []) }
      let path = String(trimmed.dropFirst(2)).trimmingCharacters(in: .whitespaces)
      guard !path.isEmpty else { return (text, []) }
      var name = (path as NSString).lastPathComponent
      // Uploads are stored as `<8 hex>-name`.
      if let dash = name.firstIndex(of: "-"), name.distance(from: name.startIndex, to: dash) == 8,
        name[..<dash].allSatisfy(\.isHexDigit)
      {
        name = String(name[name.index(after: dash)...])
      }
      found.append(TimelineAttachment(id: path, name: name, mimeType: mimeType(for: path)))
    }
    guard !found.isEmpty else { return (text, []) }
    let body = text[..<start].trimmingCharacters(in: .whitespacesAndNewlines)
    let transport = [
      "See the attached image(s).", "See the attached file(s).",
      "Please review the attached files.",
    ]
    return (transport.contains(body) ? "" : body, found)
  }
}

/// Something you sent: text and the files that went with it.
public struct TimelineUser: Equatable, Sendable {
  public let id: String
  public let text: String
  public let attachments: [TimelineAttachment]
  public let messages: [ChatMessage]
  public var sessionID: String { messages.first?.sessionId ?? "" }
}

/// One step of a turn's work, shown when its activity line is expanded.
public struct TimelineStep: Identifiable, Equatable, Sendable {
  public enum Kind: Equatable, Sendable { case thinking, narration, tool }
  public let kind: Kind
  public let message: ChatMessage
  /// A legacy host's separate output record.
  public let result: ChatMessage?
  /// The turn this step belongs to is still running.
  public let live: Bool
  public var id: String { message.id }
  private var content: JSONValue { message.content }

  /// Reasoning or narration text.
  public var text: String { content["text"].string ?? content["reasoning"].string ?? "" }
  public var redacted: Bool { content["redacted"].bool }
  public var tool: ToolStep { ToolPresentation.step(message, running: running) }
  public var running: Bool {
    guard kind == .tool else { return false }
    if subagent?.status == .running { return true }
    if result != nil { return false }
    return content["resolved"] == .bool(false) || (live && content["resolved"] == .null)
  }
  public var failed: Bool {
    guard kind == .tool else { return false }
    return content["isError"].bool || result?.content["isError"].bool == true
      || subagent?.status == .failed
  }
  /// The invocation, shown above the output: a command, a path, a query.
  public var invocation: String {
    let input = content["input"]
    if let command = input["command"].string { return command }
    if input["kind"].string != nil {
      let tool = tool
      return tool.detail
    }
    return input["file_path"].string ?? input["path"].string ?? input["pattern"].string
      ?? input["url"].string ?? input["query"].string ?? input["description"].string
      ?? (input == .null ? "" : input.pretty)
  }
  /// Output the host sent inline: a Rust summary line, or a legacy result.
  public var output: JSONValue? {
    if let result { return result.content["output"] }
    return content["output"].string.map(JSONValue.string)
  }
  /// The full output lives with the host (`FetchToolBlob`).
  public var outputRef: String? { content["outputRef"].string }
  public var outputBytes: Int? { content["outputBytes"].int }
  public var diffRef: String? { content["diffRef"].string }
  public var diffStats: [TimelineDiffStat] {
    content["diffStats"].array.compactMap { stat in
      stat["path"].string.map {
        TimelineDiffStat(
          path: $0, additions: stat["additions"].int ?? 0, deletions: stat["deletions"].int ?? 0)
      }
    }
  }
  public var additions: Int { diffStats.reduce(0) { $0 + $1.additions } }
  public var deletions: Int { diffStats.reduce(0) { $0 + $1.deletions } }
  /// An edit's before and after, when the host sent them inline.
  public var diff: TimelineDiff? {
    let inline = content["diff"]
    if let new = inline["newText"].string {
      return TimelineDiff(
        path: inline["path"].string ?? tool.detail, old: inline["oldText"].string, new: new)
    }
    let input = content["input"]
    if let new = input["new_string"].string ?? input["newString"].string {
      return TimelineDiff(
        path: input["file_path"].string ?? input["path"].string ?? tool.detail,
        old: input["old_string"].string ?? input["oldString"].string, new: new)
    }
    if let new = input["content"].string, tool.category == .write {
      return TimelineDiff(path: tool.detail, old: nil, new: new)
    }
    return nil
  }
  public var todos: [TimelineTodo] {
    let input = content["input"]
    let items = input["items"].array.isEmpty ? input["todos"].array : input["items"].array
    return items.compactMap { item in
      (item["text"].string ?? item["content"].string).map {
        TimelineTodo(
          text: $0, done: item["done"].bool || item["status"].string == "completed",
          active: item["status"].string == "in_progress")
      }
    }
  }
  /// A legacy plan-mode proposal.
  public var plan: String? { content["input"]["plan"].string }
  public var subagent: TimelineSubagent? {
    guard ToolPresentation.step(message).category == .agent else { return nil }
    let status: TimelineSubagent.Status =
      switch content["subagentStatus"].string {
      case "running": .running
      case "failed": .failed
      case "done": .done
      default:
        content["isError"].bool
          ? .failed : (content["resolved"].bool || result != nil) ? .done : .running
      }
    let tail = content["subagentTail"].string.map(ToolPresentation.oneLine)
    return TimelineSubagent(
      status: status, tail: tail?.isEmpty == false ? tail : nil,
      reference: content["subagentRef"].string)
  }
}

public struct TimelineDiffStat: Equatable, Sendable {
  public let path: String
  public let additions: Int
  public let deletions: Int
}

public struct TimelineDiff: Equatable, Sendable {
  public let path: String
  public let old: String?
  public let new: String
  /// The full diff the host keeps beside the transcript: one file or a list of them.
  public static func decode(_ text: String) -> [TimelineDiff] {
    guard let value = try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)) else {
      return []
    }
    let items = value.array.isEmpty ? [value] : value.array
    return items.compactMap { item in
      item["newText"].string.map {
        TimelineDiff(path: item["path"].string ?? "", old: item["oldText"].string, new: $0)
      }
    }
  }
  /// Changed lines with a little context, for a compact preview.
  public func lines(context: Int = 2) -> [DiffLine] {
    TextDiff.lines(old: old ?? "", new: new, context: context)
  }
}

public struct TimelineTodo: Equatable, Sendable {
  public let text: String
  public let done: Bool
  public let active: Bool
}

public struct TimelineSubagent: Equatable, Sendable {
  public enum Status: Equatable, Sendable { case running, done, failed }
  public let status: Status
  public let tail: String?
  public let reference: String?
}

/// One turn's work, folded behind a single line.
public struct TimelineActivity: Equatable, Sendable {
  public let id: String
  public let steps: [TimelineStep]
  public let live: Bool
  /// Settled turn length, from the host.
  public let durationMs: Int?
  /// When the turn began, for "Working for 12s".
  public let startedAt: Date?
  public var failedCount: Int { steps.filter(\.failed).count }
  public var toolCount: Int { steps.filter { $0.kind == .tool }.count }
  public var summary: String { ToolPresentation.summary(steps) }
  public var liveTitle: String { ToolPresentation.liveTitle(steps) }
  /// "Worked for 32s", when the host timed the turn.
  public var worked: String? {
    guard !live, let durationMs, durationMs > 0 else { return nil }
    return "Worked for " + ToolPresentation.duration(milliseconds: durationMs)
  }
  /// The settled line as a phone shows it: timing, the first two kinds of work, no failures
  /// (the view adds those so they never truncate).
  public var headline: String {
    if live { return liveTitle }
    let work = steps.isEmpty ? nil : ToolPresentation.summary(steps, limit: 2, failures: false)
    let parts = [worked, work].compactMap { $0 }
    return parts.isEmpty ? "Worked" : parts.joined(separator: " · ")
  }
  /// The settled line: timing first, then what was done.
  public var title: String {
    if live { return liveTitle }
    let parts = [worked, steps.isEmpty ? nil : summary].compactMap { $0 }
    return parts.isEmpty ? "Worked" : parts.joined(separator: " · ")
  }
}

/// The agent's answer, rendered as Markdown.
public struct TimelineReply: Equatable, Sendable {
  public let id: String
  public let text: String
  public let streaming: Bool
  public let messages: [ChatMessage]
}

/// A one-line event between messages.
public struct TimelineNotice: Equatable, Sendable {
  public enum Kind: Equatable, Sendable { case error, stopped, modelSwitch, fork, info }
  public let id: String
  public let kind: Kind
  public let title: String
  public let detail: String?
  public let message: ChatMessage
}

/// What the transcript shows, in order. Both host vocabularies (the Rust engine's canonical parts
/// and the legacy TypeScript records) normalize here, so the views have one model to draw.
public enum TimelineItem: Identifiable, Equatable, Sendable {
  case user(TimelineUser)
  case reply(TimelineReply)
  case activity(TimelineActivity)
  case notice(TimelineNotice)
  /// Images the agent produced.
  case images(id: String, attachments: [TimelineAttachment], sessionID: String)
  /// Anything with its own renderer (generated UI, usage limits, questions, model switches).
  case detail(ChatMessage)
  /// The agent hasn't produced anything for the running turn yet.
  case waiting(id: String, since: Date?)

  public var id: String {
    switch self {
    case .user(let value): value.id
    case .reply(let value): value.id
    case .activity(let value): value.id
    case .notice(let value): value.id
    case .images(let id, _, _): id
    case .detail(let message): message.id
    case .waiting(let id, _): id
    }
  }
}

public enum ChatTimeline {
  private enum Part {
    case user(ChatMessage)
    case text(ChatMessage)
    case thinking(ChatMessage)
    case tool(ChatMessage, result: ChatMessage?)
    case orphanResult(ChatMessage)
    case notice(TimelineNotice)
    case image(ChatMessage)
    case detail(ChatMessage)
    var message: ChatMessage {
      switch self {
      case .user(let m), .text(let m), .thinking(let m), .tool(let m, _), .orphanResult(let m),
        .image(let m), .detail(let m):
        m
      case .notice(let notice): notice.message
      }
    }
    var isActivity: Bool {
      switch self {
      case .thinking, .tool, .orphanResult: true
      default: false
      }
    }
  }

  /// Turns a transcript into display items: your messages, then per agent turn one activity line
  /// (reasoning, tools and narration), the reply, and anything else it produced.
  /// `running` says the host reports a turn in progress.
  public static func project(_ messages: [ChatMessage], running: Bool = false) -> [TimelineItem] {
    let parts = normalize(messages)
    var items: [TimelineItem] = []
    var turn: [Part] = []
    var turnKey: String?
    var userStart: Date?
    var index = 0
    func flush(last: Bool) {
      let live =
        last
        && (running
          || turn.contains { $0.message.content["_entryStatus"].string == "streaming" })
      items += layout(turn, key: turnKey, live: live, userStart: userStart)
      turn = []
    }
    while index < parts.count {
      if case .user(let first) = parts[index] {
        if turnKey != nil || !turn.isEmpty { flush(last: false) }
        // A Rust entry's text and images are separate parts of one message.
        var group = [first]
        let entry = first.content["_entryId"].string
        while entry != nil, index + 1 < parts.count, case .user(let next) = parts[index + 1],
          next.content["_entryId"].string == entry
        {
          group.append(next)
          index += 1
        }
        if let user = user(group) { items.append(.user(user)) }
        turnKey = first.id
        userStart = date(first)
      } else {
        if turnKey == nil { turnKey = parts[index].message.id }
        turn.append(parts[index])
      }
      index += 1
    }
    if turnKey != nil { flush(last: true) }
    return items
  }

  private static func layout(_ parts: [Part], key: String?, live: Bool, userStart: Date?)
    -> [TimelineItem]
  {
    guard let key else { return [] }
    let start = parts.compactMap { date($0.message) }.min() ?? userStart
    if parts.isEmpty {
      return live ? [.waiting(id: key + ":waiting", since: start)] : []
    }
    // Everything up to the last piece of work folds into the activity line; text after it is the
    // reply. Images, errors and generated UI follow the reply, as on the desktop.
    let lastWork = parts.lastIndex { $0.isActivity }
    var steps: [TimelineStep] = []
    var replies: [ChatMessage] = []
    var extras: [TimelineItem] = []
    var images: [ChatMessage] = []
    for (offset, part) in parts.enumerated() {
      let folded = lastWork.map { offset < $0 } ?? false
      switch part {
      case .thinking(let message):
        let text = message.content["text"].string ?? ""
        if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
          || message.content["redacted"].bool
        {
          steps.append(TimelineStep(kind: .thinking, message: message, result: nil, live: live))
        }
      case .tool(let message, let result):
        steps.append(TimelineStep(kind: .tool, message: message, result: result, live: live))
      case .orphanResult(let message):
        steps.append(TimelineStep(kind: .tool, message: message, result: nil, live: false))
      case .text(let message):
        if message.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { continue }
        if folded {
          steps.append(TimelineStep(kind: .narration, message: message, result: nil, live: live))
        } else {
          replies.append(message)
        }
      case .notice(let notice): extras.append(.notice(notice))
      case .image(let message): images.append(message)
      case .detail(let message): extras.append(.detail(message))
      case .user: break
      }
    }
    var items: [TimelineItem] = []
    let duration = durationMs(parts)
    if !steps.isEmpty || (live && replies.isEmpty) || (!live && duration != nil) {
      items.append(
        .activity(
          TimelineActivity(
            id: key + ":activity", steps: steps, live: live, durationMs: duration,
            startedAt: start)))
    }
    if !replies.isEmpty {
      items.append(
        .reply(
          TimelineReply(
            id: key + ":reply",
            text: replies.map(\.text).joined(separator: "\n\n"),
            streaming: live, messages: replies)))
    }
    if !images.isEmpty {
      items.append(
        .images(
          id: key + ":images", attachments: images.compactMap(attachment),
          sessionID: images[0].sessionId))
    }
    return items + extras
  }

  /// Sum of the turn's timed entries; the desktop shows the same total.
  private static func durationMs(_ parts: [Part]) -> Int? {
    var seen = Set<String>()
    var total = 0
    for part in parts {
      let content = part.message.content
      guard let entry = content["_entryId"].string, seen.insert(entry).inserted,
        let ms = content["_durationMs"].int, ms > 0
      else { continue }
      total += ms
    }
    return total > 0 ? total : nil
  }

  private static func date(_ message: ChatMessage) -> Date? {
    if let ms = message.content["_createdAt"].int {
      return Date(timeIntervalSince1970: Double(ms) / 1000)
    }
    return try? Date(message.createdAt, strategy: .iso8601)
  }

  private static func attachment(_ message: ChatMessage) -> TimelineAttachment? {
    let content = message.content
    guard let path = content["path"].string else { return nil }
    return TimelineAttachment(
      id: path, name: content["name"].string ?? (path as NSString).lastPathComponent,
      mimeType: content["mimeType"].string ?? TimelineAttachment.mimeType(for: path))
  }

  private static func user(_ group: [ChatMessage]) -> TimelineUser? {
    var texts: [String] = []
    var attachments: [TimelineAttachment] = []
    for message in group {
      let content = message.content
      switch message.kind {
      case "image":
        if let attachment = attachment(message) { attachments.append(attachment) }
      case "user_rich":
        texts.append(message.text)
        attachments += content["attachments"].array.compactMap { reference in
          reference["id"].string.map {
            TimelineAttachment(
              id: $0, name: reference["originalName"].string ?? "Attachment",
              mimeType: reference["mimeType"].string ?? "application/octet-stream")
          }
        }
      default:
        let (text, files) = TimelineAttachment.split(message.text)
        texts.append(text)
        attachments += files
      }
    }
    let text = texts.filter { !$0.isEmpty }.joined(separator: "\n\n")
    guard !text.isEmpty || !attachments.isEmpty else { return nil }
    var seen = Set<String>()
    return TimelineUser(
      id: group[0].id, text: text,
      attachments: attachments.filter { seen.insert($0.id).inserted }, messages: group)
  }

  /// Maps both vocabularies onto one set of parts and pairs legacy tool output with its call.
  private static func normalize(_ messages: [ChatMessage]) -> [Part] {
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
    return messages.compactMap { message -> Part? in
      if consumed.contains(message.id) { return nil }
      let content = message.content
      switch message.kind {
      case "user", "user_rich": return .user(message)
      case "text":
        return message.role == "user" ? .user(message) : .text(message)
      case "assistant": return .text(message)
      case "thinking", "reasoning": return .thinking(message)
      case "tool_use": return .tool(message, result: paired[message.id])
      case "tool_result": return .orphanResult(message)
      case "image":
        return message.role == "user" ? .user(message) : .image(message)
      case "error":
        return .notice(
          TimelineNotice(
            id: message.id, kind: .error,
            title: content["message"].string ?? content["text"].string
              ?? "The agent ran into an error.", detail: nil, message: message))
      case "interrupted":
        return .notice(
          TimelineNotice(
            id: message.id, kind: .stopped, title: "Stopped", detail: nil, message: message))
      case "fork":
        return .notice(
          TimelineNotice(
            id: message.id, kind: .fork, title: "Forked from",
            detail: content["sourceTitle"].string, message: message))
      case "agentContext":
        let from = AgentLabel.label(content["previousAgent"])
        let to = AgentLabel.label(content["agent"])
        guard from != to else { return nil }
        return .notice(
          TimelineNotice(
            id: message.id, kind: .modelSwitch, title: "Switched from \(from) to \(to)",
            detail: "Context handed off", message: message))
      case "model_switch":
        return .notice(
          TimelineNotice(
            id: message.id, kind: .modelSwitch,
            title: "Switched to \(content["toModel"].string ?? "another model")",
            detail: content["context"].string == "preserved"
              ? "Context preserved" : "Context handed off", message: message))
      case "visual":
        return .notice(
          TimelineNotice(
            id: message.id, kind: .info, title: content["title"].string ?? "Visual",
            detail: "Open this chat on your computer to see the visual.", message: message))
      case "usage", "context_usage", "messageSource": return nil
      default: return .detail(message)
      }
    }
  }
}

/// Names an agent the way the desktop does: "Sonnet 4.5", "GPT-5 Codex", "Claude Code".
public enum AgentLabel {
  public static func label(_ agent: JSONValue) -> String {
    let harness = agent["harness"].string ?? "agent"
    guard let full = agent["model"].string, !full.isEmpty else {
      return harness.split(separator: "-").map { $0.prefix(1).uppercased() + $0.dropFirst() }
        .joined(separator: " ")
    }
    let model = full.split(separator: "/").last.map(String.init) ?? full
    if model.hasPrefix("claude-") {
      let parts = model.dropFirst(7).split(separator: "-")
      guard let family = parts.first else { return model }
      let version = parts.dropFirst().prefix { $0.count <= 2 && $0.allSatisfy(\.isNumber) }
        .joined(separator: ".")
      let title = family.prefix(1).uppercased() + family.dropFirst()
      return version.isEmpty ? title : "\(title) \(version)"
    }
    if model.hasPrefix("gpt-") {
      let parts = model.dropFirst(4).split(separator: "-")
      let suffix = parts.dropFirst().map { $0.prefix(1).uppercased() + $0.dropFirst() }
        .joined(separator: " ")
      return "GPT-\(parts.first ?? "")" + (suffix.isEmpty ? "" : " \(suffix)")
    }
    return model
  }
}

/// A line diff between two texts, trimmed to the changes and a little context.
public enum TextDiff {
  public static func lines(old: String, new: String, context: Int = 2) -> [DiffLine] {
    let before = old.isEmpty ? [] : old.components(separatedBy: "\n")
    let after = new.components(separatedBy: "\n")
    // Edits from agents are small; very large texts fall back to "all removed, all added".
    var rows: [(DiffLine.Kind, String)] = []
    if before.count * after.count > 4_000_000 {
      rows = before.map { (.deletion, $0) } + after.map { (.addition, $0) }
    } else {
      let difference = after.difference(from: before)
      var removed = Set<Int>()
      var inserted = Set<Int>()
      for change in difference {
        switch change {
        case .remove(let offset, _, _): removed.insert(offset)
        case .insert(let offset, _, _): inserted.insert(offset)
        }
      }
      var i = 0
      var j = 0
      while i < before.count || j < after.count {
        if i < before.count, removed.contains(i) {
          rows.append((.deletion, before[i]))
          i += 1
        } else if j < after.count, inserted.contains(j) {
          rows.append((.addition, after[j]))
          j += 1
        } else {
          if j < after.count { rows.append((.context, after[j])) }
          i += 1
          j += 1
        }
      }
    }
    let changed = rows.indices.filter { rows[$0].0 != .context }
    var keep = Set<Int>()
    for index in changed {
      for nearby in max(0, index - context)...min(rows.count - 1, index + context) {
        keep.insert(nearby)
      }
    }
    var result: [DiffLine] = []
    var previous: Int?
    for index in rows.indices where keep.contains(index) {
      if let previous, index > previous + 1 {
        result.append(
          DiffLine(id: result.count, text: "…", kind: .header, oldLine: nil, newLine: nil))
      }
      result.append(
        DiffLine(
          id: result.count, text: rows[index].1, kind: rows[index].0, oldLine: nil, newLine: nil))
      previous = index
    }
    return result
  }
}
