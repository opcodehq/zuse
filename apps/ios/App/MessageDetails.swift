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
    case "ui_spec", "uiBlock":
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

struct ToolOutput: View {
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
        // No disclosure arrow, matching desktop: the whole row is the toggle.
        HStack(spacing: 8) {
          configuration.label
          Spacer(minLength: 4)
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
