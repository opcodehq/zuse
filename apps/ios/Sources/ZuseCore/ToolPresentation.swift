import Foundation

/// What kind of work one tool call did, for summaries ("Ran 3 commands · edited 2 files").
public enum ToolCategory: String, Equatable, Sendable {
  case command, read, write, edit, search, webSearch, fetch, plan, agent, tool
}

/// One line of agent activity: what kind of work, and what it touched.
public struct ToolStep: Equatable, Sendable {
  /// Past tense once settled ("Ran", "Read"); present tense while running ("Running").
  public let label: String
  public let detail: String
  /// SF Symbol for this kind of work.
  public let symbol: String
  public let category: ToolCategory
}

/// Names work the way the desktop does, so a turn reads the same on every device. Rust hosts
/// send a typed call; legacy hosts send a tool name and its raw input.
public enum ToolPresentation {
  static func oneLine(_ text: String) -> String {
    text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
  }
  private static func step(
    _ label: String, _ detail: String, _ symbol: String, _ category: ToolCategory
  ) -> ToolStep {
    ToolStep(label: label, detail: oneLine(detail), symbol: symbol, category: category)
  }
  private static let terminal = "terminal"
  private static let read = "doc.text"
  private static let edit = "pencil"
  private static let search = "magnifyingglass"
  private static let web = "globe"
  private static let agent = "person.2"

  /// A tool call's verb and subject. `running` picks the present tense.
  public static func step(_ message: ChatMessage, running: Bool = false) -> ToolStep {
    let input = message.content["input"]
    return input["kind"].string != nil
      ? typed(input, running: running)
      : named(message.content["tool"].string ?? "Tool", input, running: running)
  }

  private static func typed(_ call: JSONValue, running: Bool) -> ToolStep {
    func text(_ key: String) -> String { call[key].string ?? "" }
    switch call["kind"].string {
    case "exec": return step(running ? "Running" : "Ran", text("command"), terminal, .command)
    case "readFile": return step(running ? "Reading" : "Read", text("path"), read, .read)
    case "writeFile": return step(running ? "Writing" : "Wrote", text("path"), edit, .write)
    case "editFile": return step(running ? "Editing" : "Edited", text("path"), edit, .edit)
    case "applyPatch":
      return step(
        running ? "Editing" : "Edited", call["path"].string ?? "workspace", edit, .edit)
    case "search":
      let pattern = text("pattern")
      return step(
        "Search", call["path"].string.map { "\(pattern) in \($0)" } ?? pattern, search, .search)
    case "glob": return step("Find files", text("pattern"), search, .search)
    case "webFetch": return step("Fetch", text("url"), web, .fetch)
    case "webSearch": return step("Search web", text("query"), web, .webSearch)
    case "todo":
      let items = call["items"].array
      return step(
        "Plan", "\(items.filter { $0["done"].bool }.count)/\(items.count) done", "checklist",
        .plan)
    case "mcp":
      return step("MCP", "\(text("server")) · \(text("tool"))", "puzzlepiece.extension", .tool)
    default:
      let name = text("name")
      if name.hasPrefix("Agent: ") {
        return step("Agent", String(name.dropFirst(7)), agent, .agent)
      }
      return name == "Agent" ? step("Agent", "", agent, .agent) : step("Tool", name, "wrench", .tool)
    }
  }

  private static func named(_ name: String, _ input: JSONValue, running: Bool) -> ToolStep {
    func text(_ key: String) -> String { input[key].string ?? "" }
    let path = input["file_path"].string ?? text("path")
    switch name {
    case "Bash", "Run": return step(running ? "Running" : "Ran", text("command"), terminal, .command)
    case "Read": return step(running ? "Reading" : "Read", path, read, .read)
    case "Write": return step(running ? "Writing" : "Wrote", path, edit, .write)
    case "Edit", "MultiEdit": return step(running ? "Editing" : "Edited", path, edit, .edit)
    case "Grep": return step("Search", text("pattern"), search, .search)
    case "Glob": return step("Find files", text("pattern"), search, .search)
    case "WebFetch": return step("Fetch", text("url"), web, .fetch)
    case "WebSearch": return step("Search web", text("query"), web, .webSearch)
    case "TodoWrite": return step("Plan", "", "checklist", .plan)
    case "ExitPlanMode": return step("Plan", "", "checklist", .plan)
    case "Task": return step("Agent", text("description"), agent, .agent)
    default: return step("Tool", name, "wrench", .tool)
    }
  }

  private static func plural(_ count: Int, _ one: String, _ many: String) -> String {
    "\(count) \(count == 1 ? one : many)"
  }

  /// "Ran 3 commands · edited 2 files · 1 failed": categories in order of first use.
  public static func summary(_ steps: [TimelineStep], limit: Int = 4) -> String {
    let tools = steps.filter { $0.kind == .tool }
    if tools.isEmpty {
      return steps.contains { $0.kind == .thinking } ? "Thought" : "Worked"
    }
    var order: [ToolCategory] = []
    var counts: [ToolCategory: Int] = [:]
    var paths: [ToolCategory: Set<String>] = [:]
    for step in tools {
      let tool = step.tool
      // Unsettled or failed writes didn't change a file, so they count as tool calls.
      var category = tool.category
      if [.write, .edit].contains(category), step.failed { category = .tool }
      if !order.contains(category) { order.append(category) }
      if [.read, .write, .edit].contains(category), !tool.detail.isEmpty {
        let edited = step.diffStats.map(\.path)
        for path in edited.isEmpty ? [tool.detail] : edited {
          paths[category, default: []].insert(path)
        }
      } else {
        counts[category, default: 0] += 1
      }
    }
    func count(_ category: ToolCategory) -> Int {
      (counts[category] ?? 0) + (paths[category]?.count ?? 0)
    }
    var segments = order.map { category -> String in
      let n = count(category)
      switch category {
      case .command: return "ran \(plural(n, "command", "commands"))"
      case .read: return "read \(plural(n, "file", "files"))"
      case .write: return "wrote \(plural(n, "file", "files"))"
      case .edit: return "edited \(plural(n, "file", "files"))"
      case .search: return "ran \(plural(n, "search", "searches"))"
      case .webSearch: return "ran \(plural(n, "web search", "web searches"))"
      case .fetch: return "fetched \(plural(n, "page", "pages"))"
      case .plan: return "updated the plan"
      case .agent: return "started \(plural(n, "agent", "agents"))"
      case .tool: return "used \(plural(n, "tool", "tools"))"
      }
    }
    if segments.count > limit {
      let hidden = segments.count - limit
      segments = Array(segments.prefix(limit)) + ["+\(hidden)"]
    }
    let failed = tools.filter(\.failed).count
    if failed > 0 { segments.append("\(failed) failed") }
    let line = segments.joined(separator: " · ")
    return line.prefix(1).uppercased() + line.dropFirst()
  }

  /// What the agent is doing right now: its newest unsettled call, else what it last did.
  public static func liveTitle(_ steps: [TimelineStep]) -> String {
    guard let current = steps.last(where: \.running) ?? steps.last else { return "Thinking…" }
    switch current.kind {
    case .thinking: return "Thinking…"
    case .narration: return "Working…"
    case .tool:
      let tool = current.tool
      switch tool.category {
      case .command: return tool.detail.isEmpty ? "Running command…" : "Running \(tool.detail)"
      case .read: return "Reading files…"
      case .write: return "Writing files…"
      case .edit: return "Editing files…"
      case .search: return "Searching files…"
      case .webSearch: return "Searching the web…"
      case .fetch: return "Fetching page…"
      case .plan: return "Updating the plan…"
      case .agent: return "Running an agent…"
      case .tool: return tool.detail.isEmpty ? "Working…" : "Using \(tool.detail)…"
      }
    }
  }

  /// "9s", "1m 5s", "1h 0m", "1d 2h", as the desktop formats turn lengths.
  public static func duration(milliseconds: Int) -> String {
    let seconds = max(milliseconds / 1000, 1)
    if seconds < 60 { return "\(seconds)s" }
    if seconds < 3600 { return "\(seconds / 60)m \(seconds % 60)s" }
    if seconds < 86_400 { return "\(seconds / 3600)h \(seconds % 3600 / 60)m" }
    return "\(seconds / 86_400)d \(seconds % 86_400 / 3600)h"
  }
}
