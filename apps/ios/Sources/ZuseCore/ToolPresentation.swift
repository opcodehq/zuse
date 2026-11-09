import Foundation

/// One line of agent activity: what kind of work, and what it touched.
public struct ToolStep: Equatable, Sendable {
  public let label: String
  public let detail: String
  /// SF Symbol for this kind of work.
  public let symbol: String
}

/// Names work the way the desktop does (`zuse_protocol::view`), so a turn reads the same on every
/// device. Rust hosts send a typed call; legacy hosts send a tool name and its raw input.
public enum ToolPresentation {
  private static func oneLine(_ text: String) -> String {
    text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
  }
  private static func step(_ label: String, _ detail: String, _ symbol: String) -> ToolStep {
    ToolStep(label: label, detail: oneLine(detail), symbol: symbol)
  }
  private static let terminal = "terminal"
  private static let read = "doc.text"
  private static let edit = "pencil"
  private static let search = "magnifyingglass"
  private static let web = "globe"

  public static func step(_ message: ChatMessage) -> ToolStep {
    if message.kind == "thinking" || message.kind == "reasoning" {
      return step("Thinking", "", "brain")
    }
    if message.kind == "tool_result" { return step("Output", "", "text.alignleft") }
    let input = message.content["input"]
    return input["kind"].string != nil
      ? typed(input) : named(message.content["tool"].string ?? "Tool", input)
  }

  private static func typed(_ call: JSONValue) -> ToolStep {
    func text(_ key: String) -> String { call[key].string ?? "" }
    switch call["kind"].string {
    case "exec": return step("Run", text("command"), terminal)
    case "readFile": return step("Read", text("path"), read)
    case "writeFile": return step("Write", text("path"), edit)
    case "editFile": return step("Edit", text("path"), edit)
    case "applyPatch": return step("Patch", call["path"].string ?? "workspace", edit)
    case "search":
      let pattern = text("pattern")
      return step(
        "Search", call["path"].string.map { "\(pattern) in \($0)" } ?? pattern, search)
    case "glob": return step("Glob", text("pattern"), search)
    case "webFetch": return step("Fetch", text("url"), web)
    case "webSearch": return step("Web", text("query"), web)
    case "todo":
      let items = call["items"].array
      return step(
        "Todo", "\(items.filter { $0["done"].bool }.count)/\(items.count) done", "checklist")
    case "mcp":
      return step("MCP", "\(text("server")) · \(text("tool"))", "puzzlepiece.extension")
    default:
      let name = text("name")
      if name.hasPrefix("Agent: ") {
        return step("Agent", String(name.dropFirst(7)), "person.2")
      }
      return name == "Agent" ? step("Agent", "", "person.2") : step("Tool", name, "wrench")
    }
  }

  private static func named(_ name: String, _ input: JSONValue) -> ToolStep {
    func text(_ key: String) -> String { input[key].string ?? "" }
    let path = input["file_path"].string ?? text("path")
    switch name {
    case "Bash", "Run": return step("Run", text("command"), terminal)
    case "Read": return step("Read", path, read)
    case "Write": return step("Write", path, edit)
    case "Edit", "MultiEdit": return step("Edit", path, edit)
    case "Grep": return step("Search", text("pattern"), search)
    case "Glob": return step("Glob", text("pattern"), search)
    case "WebFetch": return step("Fetch", text("url"), web)
    case "WebSearch": return step("Web", text("query"), web)
    case "TodoWrite": return step("Todo", "", "checklist")
    case "Task": return step("Agent", text("description"), "person.2")
    default: return step("Tool", name, "wrench")
    }
  }

  private static func plural(_ count: Int, _ one: String, _ many: String) -> String {
    "\(count) \(count == 1 ? one : many)"
  }

  /// "Ran 3 commands · edited 2 files · 1 failed", the line settled work folds behind.
  public static func summary(_ rows: [TranscriptRow]) -> String {
    let tools = rows.filter { $0.message.kind == "tool_use" }
    if tools.isEmpty { return rows.contains { $0.isThinking } ? "Thought" : "Activity" }
    var edited = Set<String>()
    var commands = 0
    var reads = 0
    var searches = 0
    var fetches = 0
    var todos = 0
    var other = 0
    for row in tools {
      let step = step(row.message)
      switch step.label {
      case "Run": commands += 1
      case "Write", "Edit", "Patch": edited.insert(step.detail)
      case "Read": reads += 1
      case "Search", "Glob", "Web": searches += 1
      case "Fetch": fetches += 1
      case "Todo": todos += 1
      default: other += 1
      }
    }
    var segments: [String] = []
    if commands > 0 { segments.append("ran \(plural(commands, "command", "commands"))") }
    if !edited.isEmpty { segments.append("edited \(plural(edited.count, "file", "files"))") }
    if reads > 0 { segments.append("read \(plural(reads, "file", "files"))") }
    if searches > 0 { segments.append("searched \(plural(searches, "time", "times"))") }
    if fetches > 0 { segments.append("fetched \(plural(fetches, "page", "pages"))") }
    if todos > 0 { segments.append("updated todos") }
    if other > 0 { segments.append("called \(plural(other, "tool", "tools"))") }
    let failed = tools.filter(\.failed).count
    if failed > 0 { segments.append("\(failed) failed") }
    let line = segments.joined(separator: " · ")
    return line.prefix(1).uppercased() + line.dropFirst()
  }

  /// What the agent is doing right now, for the line above work still in progress.
  public static func liveTitle(_ rows: [TranscriptRow]) -> String {
    guard let current = rows.last(where: \.running) ?? rows.last, !current.isThinking else {
      return "Thinking…"
    }
    let step = step(current.message)
    let verb: String
    switch step.label {
    case "Run": verb = "Running"
    case "Read": verb = "Reading"
    case "Write": verb = "Writing"
    case "Edit", "Patch": verb = "Editing"
    case "Search", "Glob", "Web": verb = "Searching"
    case "Fetch": verb = "Fetching"
    case "Todo": verb = "Updating todos"
    case "Agent": verb = "Running agent"
    default: verb = "Working"
    }
    return step.label == "Todo" || step.detail.isEmpty ? "\(verb)…" : "\(verb) \(step.detail)"
  }
}

extension TranscriptRow {
  public var isThinking: Bool { ["thinking", "reasoning"].contains(message.kind) }
  public var failed: Bool {
    message.content["isError"].bool || result?.content["isError"].bool == true
  }
  /// Rust hosts mark a call resolved; legacy hosts send a separate result.
  public var running: Bool {
    message.kind == "tool_use" && result == nil && message.content["resolved"] == .bool(false)
  }
  public var step: ToolStep { ToolPresentation.step(message) }
}

extension TranscriptGroup {
  public var hasRunningTool: Bool { rows.contains(where: \.running) }
  public var failed: Bool { rows.contains(where: \.failed) }
  /// The line the group folds behind, or what it is doing while it runs.
  public var title: String {
    hasRunningTool ? ToolPresentation.liveTitle(rows) : ToolPresentation.summary(rows)
  }
}
