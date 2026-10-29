import SwiftUI
import Textual
import ZuseCore

struct MessageDetails: View {
  let message: ChatMessage
  var respond: ((String) async throws -> Void)?
  private var content: JSONValue { message.content }
  var body: some View {
    switch message.kind {
    case "thinking":
      DisclosureGroup {
        if content["redacted"].bool {
          Text("Reasoning is private.").foregroundStyle(.secondary)
        } else {
          markdown(content["text"].string ?? "")
        }
      } label: {
        Label("Reasoning", systemImage: "brain").font(.subheadline).foregroundStyle(.secondary)
      }
    case "tool_use":
      DisclosureGroup {
        if let plan = content["input"]["plan"].string {
          markdown(plan)
        } else {
          code(content["input"].string ?? content["input"].pretty)
        }
      } label: {
        HStack(spacing: 8) {
          Image(systemName: "terminal").foregroundStyle(.secondary)
          Text(content["tool"].string ?? "Tool").fontWeight(.medium)
          Text(toolSummary).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
        }.font(.footnote)
      }
    case "tool_result":
      DisclosureGroup {
        ToolOutput(value: content["output"])
      } label: {
        Label(
          content["isError"].bool ? "Tool failed" : "Completed",
          systemImage: content["isError"].bool ? "exclamationmark.triangle" : "checkmark.circle"
        )
        .font(.caption).foregroundStyle(content["isError"].bool ? .red : .secondary)
      }
    case "error":
      Label(
        content["message"].string ?? "The agent encountered an error.",
        systemImage: "exclamationmark.triangle"
      ).foregroundStyle(.red).textSelection(.enabled)
    case "interrupted":
      Label("Response stopped", systemImage: "stop.circle").foregroundStyle(.secondary)
    case "model_switch":
      Label(
        "Switched to \(content["toModel"].string ?? "another model")",
        systemImage: "arrow.triangle.swap"
      ).font(.subheadline)
      Text(
        content["context"].string == "preserved"
          ? "Conversation context preserved" : "Conversation handed off"
      ).font(.caption).foregroundStyle(.secondary)
    case "subagent_summary":
      DisclosureGroup {
        markdown(content["summary"].string ?? "")
      } label: {
        Label(content["agentName"].string ?? "Agent summary", systemImage: "person.2").font(
          .subheadline)
      }
    case "subagent_progress":
      Label("Agent · \(content["status"].string ?? "Working")", systemImage: "person.2")
      Text(
        "\(content["turns"].int ?? 0) turns · \(content["toolCalls"].int ?? 0) tool calls · \(content["tokens"].int ?? 0) tokens"
      ).font(.caption).foregroundStyle(.secondary)
    case "usage":
      DisclosureGroup("Usage · \(content["model"].string ?? "Model")") {
        LabeledContent("Input tokens", value: String(content["inputTokens"].int ?? 0))
        LabeledContent("Output tokens", value: String(content["outputTokens"].int ?? 0))
        LabeledContent("Cached input", value: String(content["cacheReadTokens"].int ?? 0))
        LabeledContent("Cache created", value: String(content["cacheCreationTokens"].int ?? 0))
      }.font(.caption).foregroundStyle(.secondary)
    case "context_usage":
      if let used = content["usedTokens"].int, let total = content["windowTokens"].int, total > 0 {
        ProgressView(value: min(Double(used) / Double(total), 1)) {
          Text("Context · \(used.formatted()) / \(total.formatted()) tokens")
        }.font(.caption)
      } else {
        Label("Context usage unavailable", systemImage: "chart.bar").font(.caption).foregroundStyle(
          .secondary)
      }
    case "context_compaction":
      Label(
        "Context compaction · \(content["status"].string ?? "completed")",
        systemImage: "arrow.down.right.and.arrow.up.left"
      ).font(.caption).foregroundStyle(.secondary)
    case "usage_limit":
      Text(content["label"].string ?? "Usage limit").font(.subheadline)
      if let percent = content["usedPercent"].int {
        ProgressView(value: Double(max(0, min(percent, 100))), total: 100) {
          Text("\(percent)% used")
        }.font(.caption)
      }
      if let reset = content["resetsAt"].string {
        Text("Resets \(reset)").font(.caption).foregroundStyle(.secondary)
      }
    case "user_question":
      ForEach(Array(content["questions"].array.enumerated()), id: \.offset) { _, question in
        Label(question["question"].string ?? "Agent question", systemImage: "questionmark.bubble")
          .font(.subheadline)
        ForEach(Array(question["options"].array.enumerated()), id: \.offset) { _, option in
          Text("• " + (option.string ?? "")).foregroundStyle(.secondary)
        }
      }
    case "user_question_answer":
      Label(
        content["resolution"].string.map { "Question \($0)" } ?? "Answers submitted",
        systemImage: "checkmark.bubble"
      ).font(.subheadline)
      ForEach(Array(content["answers"].array.enumerated()), id: \.offset) { _, answer in
        if let other = answer["other"].string, !other.isEmpty {
          Text(other).textSelection(.enabled)
        } else {
          Text(
            "Selected options: "
              + answer["selected"].array.compactMap { $0.int.map { String($0 + 1) } }.joined(
                separator: ", ")
          ).font(.caption)
        }
      }
    case "ui_spec":
      UISpecView(spec: content["spec"].string ?? "", respond: respond)
    default:
      DisclosureGroup(message.kind.replacingOccurrences(of: "_", with: " ").capitalized) {
        code(message.text)
      }
    }
  }
  private var toolSummary: String {
    let input = content["input"]
    return input["command"].string ?? input["file_path"].string ?? input["path"].string
      ?? input["description"].string ?? ""
  }
  private func markdown(_ text: String) -> some View {
    ChatMarkdown(text: text)
  }
  private func code(_ text: String) -> some View {
    ScrollView(.horizontal) {
      Text(text).font(.system(.caption, design: .monospaced)).textSelection(.enabled).fixedSize(
        horizontal: true, vertical: true)
    }
  }
}

private struct ToolOutput: View {
  let value: JSONValue
  var body: some View {
    if let text = value.string {
      Text(text).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    } else if case .array(let blocks) = value {
      ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in blockView(block) }
    } else if !value["content"].array.isEmpty {
      ForEach(Array(value["content"].array.enumerated()), id: \.offset) { _, block in
        blockView(block)
      }
    } else {
      Text(value.pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
    }
  }
  @ViewBuilder private func blockView(_ block: JSONValue) -> some View {
    if let text = block["text"].string {
      ChatMarkdown(text: text)
    } else if block["type"].string == "image", let encoded = block["data"].string,
      encoded.utf8.count < 28_000_000, let data = Data(base64Encoded: encoded),
      let image = NativeImage(data: data)
    {
      Image(nativeImage: image).resizable().scaledToFit().frame(maxHeight: 400).accessibilityLabel(
        "Tool result image")
    } else {
      Text(block.pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
    }
  }
}

/// Explicit disclosure controls also work inside recycled UIKit-hosted rows.
struct TranscriptDisclosureStyle: DisclosureGroupStyle {
  func makeBody(configuration: Configuration) -> some View {
    VStack(alignment: .leading, spacing: 6) {
      Button {
        configuration.isExpanded.toggle()
      } label: {
        HStack(spacing: 8) {
          configuration.label
          Spacer(minLength: 4)
          Image(systemName: configuration.isExpanded ? "chevron.down" : "chevron.right")
            .font(.system(size: 11, weight: .semibold)).foregroundStyle(.tertiary)
        }
        .frame(minHeight: 32).contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityValue(configuration.isExpanded ? "Expanded" : "Collapsed")
      if configuration.isExpanded {
        configuration.content.padding(.leading, 22).padding(.bottom, 4)
      }
    }
  }
}

struct ToolTreeRow: View {
  let row: TranscriptRow
  var toggleDisclosure: ((() -> Void) -> Void)? = nil
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var expanded = false
  private var message: ChatMessage { row.message }
  private var title: String {
    message.kind == "thinking" ? "Reasoning" : message.content["tool"].string ?? "Tool output"
  }
  private var summary: String {
    let input = message.content["input"]
    return input["command"].string ?? input["file_path"].string ?? input["path"].string
      ?? input["description"].string ?? ""
  }
  var body: some View {
    HStack(alignment: .top, spacing: 7) {
      TreeConnector(previous: row.joinsPrevious, next: row.joinsNext)
        .stroke(Color.secondary.opacity(0.23), lineWidth: 1)
        .frame(width: 14).accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 0) {
        Button {
          changeExpansion()
        } label: {
          HStack(spacing: 7) {
            Image(systemName: message.kind == "thinking" ? "brain" : "terminal")
              .font(.system(size: 14)).frame(width: 18).foregroundStyle(.secondary)
            Text(title).font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
            if !summary.isEmpty {
              Text(summary).font(.system(size: 13, design: .monospaced))
                .lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                .padding(.horizontal, 6).padding(.vertical, 3)
                .background(.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 5))
            }
            if let result = row.result, result.content["isError"].bool {
              Image(systemName: "exclamationmark.circle").font(.system(size: 13)).foregroundStyle(
                .red)
            }
            Image(systemName: "chevron.right")
              .font(.system(size: 10, weight: .medium)).foregroundStyle(.tertiary)
              .rotationEffect(.degrees(expanded ? 90 : 0))
              .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: expanded)
          }.frame(minHeight: 30, alignment: .leading).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel([title, summary].filter { !$0.isEmpty }.joined(separator: ", "))
        .accessibilityIdentifier("activity-\(message.id)")
        .accessibilityValue(expanded ? "Expanded" : "Collapsed")
        if expanded {
          ViewThatFits(in: .vertical) {
            detailContent.fixedSize(horizontal: false, vertical: true)
            ScrollView { detailContent }
          }
          .frame(maxHeight: 200)
          .background(.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 8))
          .padding(.leading, 22).padding(.bottom, 5)
        }
      }
    }
    .fixedSize(horizontal: false, vertical: true)
    .padding(.horizontal, 20)
    .frame(maxWidth: 720, alignment: .leading).frame(maxWidth: .infinity)
  }
  private var detailContent: some View {
    VStack(alignment: .leading, spacing: 10) {
      if message.kind == "thinking" {
        if message.content["redacted"].bool {
          Text("Reasoning is private.")
        } else {
          ChatMarkdown(text: message.text)
        }
      } else if message.kind == "tool_result" {
        ToolOutput(value: message.content["output"])
      } else {
        Text(message.content["input"].string ?? message.content["input"].pretty)
          .font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
        if let result = row.result {
          Divider()
          ToolOutput(value: result.content["output"])
        }
      }
    }.frame(maxWidth: .infinity, alignment: .leading).padding(10)
  }
  private func changeExpansion() {
    if let toggleDisclosure {
      toggleDisclosure { expanded.toggle() }
    } else {
      var transaction = Transaction(animation: nil)
      transaction.disablesAnimations = true
      withTransaction(transaction) { expanded.toggle() }
    }
  }

}

private struct TreeConnector: Shape {
  let previous: Bool
  let next: Bool
  func path(in rect: CGRect) -> Path {
    var path = Path()
    let y: CGFloat = 15
    path.move(to: CGPoint(x: 2, y: previous ? 0 : y - 10))
    path.addLine(to: CGPoint(x: 2, y: next ? rect.height : y - 5))
    path.move(to: CGPoint(x: 2, y: y - 5))
    path.addQuadCurve(to: CGPoint(x: 12, y: y), control: CGPoint(x: 2, y: y))
    return path
  }
}

struct ToolActivityGroup: View {
  let group: TranscriptGroup
  var toggleDisclosure: ((() -> Void) -> Void)? = nil
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var expanded = true
  @State private var page: Int
  private let pageSize = 12
  init(group: TranscriptGroup, toggleDisclosure: ((() -> Void) -> Void)? = nil) {
    self.group = group
    self.toggleDisclosure = toggleDisclosure
    _page = State(initialValue: max(0, (group.rows.count - 1) / 12))
  }
  private var heading: String {
    group.toolCount == 0
      ? "Activity" : "\(group.toolCount) tool \(group.toolCount == 1 ? "call" : "calls")"
  }
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Button {
        if let toggleDisclosure {
          toggleDisclosure { expanded.toggle() }
        } else {
          expanded.toggle()
        }
      } label: {
        HStack(spacing: 7) {
          Image(systemName: "chevron.right")
            .font(.system(size: 11, weight: .medium))
            .rotationEffect(.degrees(expanded ? 90 : 0))
            .animation(reduceMotion ? nil : .easeOut(duration: 0.15), value: expanded)
          Text(heading).font(.system(size: 14)).monospacedDigit()
        }.foregroundStyle(.secondary).frame(minHeight: 34).contentShape(Rectangle())
      }.buttonStyle(.plain).padding(.horizontal, 20)
        .accessibilityLabel(heading).accessibilityValue(expanded ? "Expanded" : "Collapsed")
        .accessibilityIdentifier("tool-group-\(group.id)")
      if expanded {
        let start = min(page * pageSize, max(0, group.rows.count - 1))
        let end = min(start + pageSize, group.rows.count)
        ForEach(Array(group.rows[start..<end])) { row in
          ToolTreeRow(row: row, toggleDisclosure: toggleDisclosure)
        }
        if group.rows.count > pageSize {
          HStack(spacing: 12) {
            Button("Earlier") { page = max(0, page - 1) }.disabled(page == 0)
            Text("\(start + 1)–\(end) of \(group.rows.count)").monospacedDigit().foregroundStyle(
              .secondary)
            Button("Later") { page += 1 }.disabled(end >= group.rows.count)
          }.font(.caption2).padding(.leading, 42).padding(.vertical, 6)
        }
      }
    }.frame(maxWidth: 720, alignment: .leading).frame(maxWidth: .infinity)
      .onChange(of: group.rows.count) { old, new in
        if page == max(0, (old - 1) / pageSize) { page = max(0, (new - 1) / pageSize) }
      }
  }
}
