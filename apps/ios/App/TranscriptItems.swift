import SwiftUI
import ZuseCore

/// Runs a change that alters a recycled row's height, so the list can re-measure it in step.
typealias RowResize = (() -> Void) -> Void

/// Screenshot switches for previews: open every activity line, or every step too.
enum TranscriptPreviewFlags {
  #if DEBUG
    static let expandActivity = ProcessInfo.processInfo.arguments.contains("--expand-activity")
    static let expandSteps = ProcessInfo.processInfo.arguments.contains("--expand-steps")
    static let startAtTop = ProcessInfo.processInfo.arguments.contains("--transcript-top")
  #else
    static let expandActivity = false
    static let expandSteps = false
    static let startAtTop = false
  #endif
}

/// The content column every transcript row shares.
private let columnWidth: CGFloat = 720
private let gutter: CGFloat = 18

/// One transcript row: a message you sent, the agent's work or reply, or an event between them.
struct TimelineItemView: View {
  let item: TimelineItem
  let rpc: RPCClient?
  var respond: ((ChatMessage, String) async throws -> Void)?
  var resize: RowResize?
  var body: some View {
    content
      .padding(.horizontal, gutter)
      .frame(maxWidth: columnWidth, alignment: .leading).frame(maxWidth: .infinity)
      // Cells are hosted outside the window's SwiftUI tree, so they route PR links themselves.
      .pullRequestLinks()
  }
  @ViewBuilder private var content: some View {
    switch item {
    case .user(let user): UserMessageView(user: user, rpc: rpc, resize: resize)
    case .reply(let reply): ReplyView(reply: reply)
    case .activity(let activity): ActivityView(activity: activity, rpc: rpc, resize: resize)
    case .notice(let notice): NoticeView(notice: notice)
    case .images(_, let attachments, let sessionID):
      AttachmentRow(attachments: attachments, sessionID: sessionID, rpc: rpc, alignment: .leading)
        .padding(.vertical, 6)
    case .detail(let message):
      MessageDetails(
        message: message,
        respond: respond.map { callback in { text in try await callback(message, text) } }
      )
      .font(.subheadline)
      .disclosureGroupStyle(TranscriptDisclosureStyle())
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.vertical, 6)
    case .waiting(_, let since): WaitingView(since: since)
    }
  }
}

// MARK: - Your messages

private struct UserMessageView: View {
  let user: TimelineUser
  let rpc: RPCClient?
  var resize: RowResize?
  @State private var showsAll = false
  /// Pasted logs and long prompts fold so one message can't fill the screen.
  private var folds: Bool { user.text.count > 900 || user.text.filter { $0 == "\n" }.count > 14 }
  var body: some View {
    VStack(alignment: .trailing, spacing: 6) {
      if !user.attachments.isEmpty {
        AttachmentRow(
          attachments: user.attachments, sessionID: user.sessionID, rpc: rpc, alignment: .trailing)
      }
      if !user.text.isEmpty {
        VStack(alignment: .leading, spacing: 6) {
          Text(user.text)
            .font(.system(size: 16))
            .lineSpacing(2)
            .lineLimit(folds && !showsAll ? 12 : nil)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
          if folds {
            Button(showsAll ? "Show less" : "Show more") {
              if let resize { resize { showsAll.toggle() } } else { showsAll.toggle() }
            }
            .font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
            .buttonStyle(.plain)
          }
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .background(AppTheme.bubble, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .contextMenu {
          Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(user.text) }
          ShareLink(item: user.text)
        }
      }
    }
    .frame(maxWidth: .infinity, alignment: .trailing)
    .padding(.leading, 44)
    .padding(.top, 18).padding(.bottom, 6)
    .accessibilityElement(children: .contain)
  }
}

/// Thumbnails and file pills, wrapping onto more rows when needed.
private struct AttachmentRow: View {
  let attachments: [TimelineAttachment]
  let sessionID: String
  let rpc: RPCClient?
  let alignment: HorizontalAlignment
  var body: some View {
    let images = attachments.filter(\.isImage)
    let files = attachments.filter { !$0.isImage }
    VStack(alignment: alignment, spacing: 6) {
      if !images.isEmpty {
        WrappingRow(spacing: 6, trailing: alignment == .trailing) {
          ForEach(images) { attachment in
            AttachmentView(
              attachment: attachment, sessionID: sessionID, rpc: rpc, large: images.count == 1)
          }
        }
      }
      ForEach(files) { attachment in
        AttachmentView(attachment: attachment, sessionID: sessionID, rpc: rpc)
      }
    }
    .frame(maxWidth: .infinity, alignment: alignment == .trailing ? .trailing : .leading)
  }
}

/// A flow layout: children left to right (or right to left), wrapping at the edge.
private struct WrappingRow: Layout {
  var spacing: CGFloat
  var trailing: Bool
  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let rows = arrange(proposal.width ?? .infinity, subviews)
    let height = rows.map(\.height).reduce(0, +) + spacing * CGFloat(max(0, rows.count - 1))
    let width = rows.map(\.width).max() ?? 0
    return CGSize(width: proposal.width ?? width, height: height)
  }
  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    var y = bounds.minY
    for row in arrange(bounds.width, subviews) {
      var x = trailing ? bounds.maxX - row.width : bounds.minX
      for index in row.indices {
        let size = subviews[index].sizeThatFits(.unspecified)
        subviews[index].place(at: CGPoint(x: x, y: y), proposal: .unspecified)
        x += size.width + spacing
      }
      y += row.height + spacing
    }
  }
  private struct Row {
    var indices: [Int] = []
    var width: CGFloat = 0
    var height: CGFloat = 0
  }
  private func arrange(_ width: CGFloat, _ subviews: Subviews) -> [Row] {
    var rows: [Row] = [Row()]
    for index in subviews.indices {
      let size = subviews[index].sizeThatFits(.unspecified)
      if !rows[rows.count - 1].indices.isEmpty,
        rows[rows.count - 1].width + spacing + size.width > width
      {
        rows.append(Row())
      }
      var row = rows[rows.count - 1]
      row.width += (row.indices.isEmpty ? 0 : spacing) + size.width
      row.height = max(row.height, size.height)
      row.indices.append(index)
      rows[rows.count - 1] = row
    }
    return rows.filter { !$0.indices.isEmpty }
  }
}

// MARK: - The agent's reply

private struct ReplyView: View {
  let reply: TimelineReply
  var body: some View {
    VStack(alignment: .leading, spacing: 2) {
      ChatMarkdown(text: reply.text, size: .reply)
        .frame(maxWidth: .infinity, alignment: .leading)
      if reply.streaming {
        PulsingDots().padding(.top, 6)
      } else {
        HStack(spacing: 0) {
          CopyButton(label: "Copy reply") { NativePlatform.copy(reply.text) }
          ShareLink(item: reply.text) {
            Image(systemName: "square.and.arrow.up")
              .font(.system(size: 13, weight: .medium)).foregroundStyle(.secondary)
              .frame(width: 30, height: 30).contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .accessibilityLabel("Share reply")
        }
        .padding(.leading, -7)
      }
    }
    .padding(.top, 4).padding(.bottom, 10)
    .contextMenu {
      Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(reply.text) }
      ShareLink(item: reply.text)
    }
  }
}

/// Three dots that breathe while the agent writes.
private struct PulsingDots: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  var body: some View {
    TimelineView(.animation(minimumInterval: 1 / 20, paused: reduceMotion)) { context in
      let phase = context.date.timeIntervalSinceReferenceDate * 2.4
      HStack(spacing: 4) {
        ForEach(0..<3) { index in
          Circle()
            .frame(width: 6, height: 6)
            .opacity(reduceMotion ? 0.5 : 0.25 + 0.6 * max(0, sin(phase - Double(index) * 0.7)))
        }
      }
      .foregroundStyle(.secondary)
    }
    .frame(height: 10)
    .accessibilityLabel("Writing")
  }
}

/// Before the agent has said or done anything in a running turn.
private struct WaitingView: View {
  let since: Date?
  var body: some View {
    HStack(spacing: 10) {
      PulsingDots()
      Text("Thinking").font(.system(size: 14)).foregroundStyle(.secondary)
      if let since { Elapsed(since: since).foregroundStyle(.tertiary) }
    }
    .padding(.vertical, 10)
    .frame(maxWidth: .infinity, alignment: .leading)
    .accessibilityElement(children: .combine)
  }
}

/// "12s", ticking.
private struct Elapsed: View {
  let since: Date
  var body: some View {
    TimelineView(.periodic(from: .now, by: 1)) { context in
      Text(
        ToolPresentation.duration(
          milliseconds: max(0, Int(context.date.timeIntervalSince(since) * 1000)))
      )
      .font(.system(size: 13)).monospacedDigit()
    }
  }
}

// MARK: - Events

private struct NoticeView: View {
  let notice: TimelineNotice
  var body: some View {
    switch notice.kind {
    case .error:
      HStack(alignment: .firstTextBaseline, spacing: 10) {
        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red)
        VStack(alignment: .leading, spacing: 2) {
          Text("Error").font(.system(size: 14, weight: .semibold))
          Text(notice.title).font(.system(size: 14)).foregroundStyle(.secondary)
            .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        }
        Spacer(minLength: 0)
      }
      .padding(12)
      .background(
        Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 14, style: .continuous)
      )
      .overlay(
        RoundedRectangle(cornerRadius: 14, style: .continuous)
          .strokeBorder(Color.red.opacity(0.18), lineWidth: 1)
      )
      .padding(.vertical, 6)
      .contextMenu {
        Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(notice.title) }
      }
    case .stopped:
      Label(notice.title, systemImage: "stop.circle")
        .font(.system(size: 13)).foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 6)
    case .modelSwitch, .fork:
      HStack(spacing: 10) {
        rule
        VStack(spacing: 1) {
          Text(notice.title).font(.system(size: 12, weight: .medium))
          if let detail = notice.detail {
            Text(detail).font(.system(size: 12)).foregroundStyle(.tertiary)
          }
        }
        .multilineTextAlignment(.center).foregroundStyle(.secondary)
        .layoutPriority(1)
        rule
      }
      .padding(.vertical, 12)
      .accessibilityElement(children: .combine)
    case .info:
      HStack(spacing: 10) {
        Image(systemName: "sparkles.rectangle.stack").foregroundStyle(.secondary)
        VStack(alignment: .leading, spacing: 2) {
          Text(notice.title).font(.system(size: 14, weight: .medium))
          if let detail = notice.detail {
            Text(detail).font(.system(size: 13)).foregroundStyle(.secondary)
          }
        }
        Spacer(minLength: 0)
      }
      .padding(12)
      .background(
        AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 14, style: .continuous)
      )
      .padding(.vertical, 6)
    }
  }
  private var rule: some View {
    Rectangle().fill(Color.primary.opacity(0.1)).frame(height: 1)
  }
}

// MARK: - The agent's work

/// One quiet line per turn ("Worked for 32s · Ran 3 commands"), opening onto every step.
struct ActivityView: View {
  let activity: TimelineActivity
  let rpc: RPCClient?
  var resize: RowResize?
  /// Set only by a tap. Otherwise running work stays open and settled work folds.
  @State private var choice: Bool? = TranscriptPreviewFlags.expandActivity ? true : nil
  @State private var showsEarlier = false
  private var expanded: Bool { choice ?? activity.live }
  private var expandable: Bool { !activity.steps.isEmpty }
  /// Long turns open on their latest steps.
  private let recentLimit = 8
  private var hiddenCount: Int {
    showsEarlier ? 0 : max(0, activity.steps.count - recentLimit)
  }
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
      if expanded && expandable {
        VStack(alignment: .leading, spacing: 0) {
          if hiddenCount > 0 {
            Button {
              change { showsEarlier = true }
            } label: {
              Text("Show \(hiddenCount) earlier \(hiddenCount == 1 ? "step" : "steps")")
                .font(.system(size: 13, weight: .medium)).foregroundStyle(.secondary)
                .frame(minHeight: 30).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
          }
          ForEach(activity.steps.suffix(activity.steps.count - hiddenCount)) { step in
            StepView(step: step, rpc: rpc, resize: resize)
          }
        }
        .padding(.leading, 12)
        .overlay(alignment: .leading) {
          RoundedRectangle(cornerRadius: 1).fill(Color.primary.opacity(0.1)).frame(width: 2)
            .padding(.vertical, 4)
        }
        .padding(.leading, 6).padding(.bottom, 6)
      }
    }
    .padding(.top, 2).padding(.bottom, 2)
    .frame(maxWidth: .infinity, alignment: .leading)
  }
  private var header: some View {
    Button {
      guard expandable else { return }
      change { choice = !expanded }
    } label: {
      HStack(spacing: 7) {
        if activity.live {
          ProgressView().controlSize(.mini)
          Text(activity.liveTitle)
            .font(.system(size: 14)).foregroundStyle(.secondary)
            .lineLimit(1).truncationMode(.middle)
          if let since = activity.startedAt {
            Elapsed(since: since).foregroundStyle(.tertiary)
          }
        } else {
          Text(activity.headline)
            .font(.system(size: 14)).foregroundStyle(.secondary)
            .lineLimit(1).truncationMode(.tail)
          if activity.failedCount > 0 {
            // Failures never truncate away.
            Text("· \(activity.failedCount) failed")
              .font(.system(size: 14)).foregroundStyle(.red.opacity(0.85))
              .lineLimit(1).fixedSize()
          }
        }
        if expandable {
          Image(systemName: "chevron.right")
            .font(.system(size: 10, weight: .semibold)).foregroundStyle(.tertiary)
            .rotationEffect(.degrees(expanded ? 90 : 0))
        }
        Spacer(minLength: 0)
      }
      .frame(minHeight: 34).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .disabled(!expandable)
    .accessibilityLabel(activity.live ? activity.liveTitle : activity.title)
    .accessibilityValue(expandable ? (expanded ? "Expanded" : "Collapsed") : "")
    .accessibilityIdentifier("tool-group-\(activity.id)")
  }
  private func change(_ update: () -> Void) {
    if let resize {
      resize(update)
    } else {
      var transaction = Transaction(animation: nil)
      transaction.disablesAnimations = true
      withTransaction(transaction, update)
    }
  }
}

/// One step: a thought, a line of narration, or a tool call that opens onto its details.
private struct StepView: View {
  let step: TimelineStep
  let rpc: RPCClient?
  var resize: RowResize?
  @State private var expanded = TranscriptPreviewFlags.expandSteps
  var body: some View {
    switch step.kind {
    case .narration:
      ChatMarkdown(text: step.text, size: .compact)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 6)
    case .thinking:
      VStack(alignment: .leading, spacing: 0) {
        row(symbol: "brain", label: "Thinking", detail: expanded ? "" : preview, mono: false)
        if expanded {
          Group {
            if step.redacted {
              Text("Reasoning is private.").font(.system(size: 14))
            } else {
              ChatMarkdown(text: step.text, size: .compact)
            }
          }
          .foregroundStyle(.secondary)
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(.leading, 25).padding(.bottom, 8)
        }
      }
    case .tool:
      let tool = step.tool
      VStack(alignment: .leading, spacing: 0) {
        if let subagent = step.subagent {
          row(
            symbol: tool.symbol, label: tool.detail.isEmpty ? "Agent" : tool.detail,
            detail: subagent.tail ?? status(subagent.status), mono: false)
        } else {
          row(symbol: tool.symbol, label: tool.label, detail: shortPath(tool), mono: true)
        }
        if expanded {
          ToolDetailView(step: step, rpc: rpc, resize: resize)
            .padding(.leading, 25).padding(.top, 2).padding(.bottom, 8)
        }
      }
    }
  }
  private var preview: String {
    step.redacted
      ? "" : ToolPresentation.oneLineSummary(step.text)
  }
  /// Absolute paths keep their last two parts in the row; the full path shows when expanded.
  private func shortPath(_ tool: ToolStep) -> String {
    guard [.read, .write, .edit].contains(tool.category), tool.detail.hasPrefix("/") else {
      return tool.detail
    }
    let parts = tool.detail.split(separator: "/")
    return parts.count > 2 ? parts.suffix(2).joined(separator: "/") : tool.detail
  }
  private func status(_ status: TimelineSubagent.Status) -> String {
    switch status {
    case .running: "Running"
    case .done: "Done"
    case .failed: "Failed"
    }
  }
  private func row(symbol: String, label: String, detail: String, mono: Bool) -> some View {
    Button {
      if let resize { resize { expanded.toggle() } } else { expanded.toggle() }
    } label: {
      HStack(spacing: 7) {
        Group {
          if step.running {
            ProgressView().controlSize(.mini)
          } else if let subagent = step.subagent {
            Image(
              systemName: subagent.status == .failed
                ? "xmark.circle.fill" : "checkmark.circle.fill"
            )
            .foregroundStyle(subagent.status == .failed ? Color.red : Color.green)
          } else {
            Image(systemName: symbol).foregroundStyle(.secondary)
          }
        }
        .font(.system(size: 12)).frame(width: 18)
        Text(label).font(.system(size: 14, weight: .medium)).foregroundStyle(.primary)
          .lineLimit(1).layoutPriority(1)
        if !detail.isEmpty {
          Text(detail)
            .font(mono ? .system(size: 13, design: .monospaced) : .system(size: 13))
            .foregroundStyle(.secondary)
            .lineLimit(1).truncationMode(mono ? .middle : .tail)
        }
        if step.additions > 0 {
          Text("+\(step.additions)").font(.system(size: 12, weight: .medium)).monospacedDigit()
            .foregroundStyle(.green)
        }
        if step.deletions > 0 {
          Text("−\(step.deletions)").font(.system(size: 12, weight: .medium)).monospacedDigit()
            .foregroundStyle(.red)
        }
        if step.failed {
          Text("error").font(.system(size: 12, weight: .medium)).foregroundStyle(.red)
        }
        Spacer(minLength: 0)
      }
      .frame(minHeight: 30).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(
      [label, detail, step.failed ? "failed" : ""].filter { !$0.isEmpty }
        .joined(separator: ", ")
    )
    .accessibilityValue(expanded ? "Expanded" : "Collapsed")
    .accessibilityIdentifier("activity-\(step.id)")
  }
}

/// What a tool call did: the invocation, then its output, diff or checklist.
private struct ToolDetailView: View {
  let step: TimelineStep
  let rpc: RPCClient?
  var resize: RowResize?
  @State private var output: String?
  @State private var outputTotal: Int?
  @State private var outputError: String?
  @State private var fetchedDiffs: [TimelineDiff]?
  @State private var showsAllOutput = false
  private let outputLines = 24
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      if showsInvocation {
        let invocation = step.invocation
        Text(step.tool.category == .command ? "$ " + invocation : invocation)
          .font(.system(size: 12, design: .monospaced))
          .foregroundStyle(.primary.opacity(0.85))
          .textSelection(.enabled)
          .lineLimit(8)
          .fixedSize(horizontal: false, vertical: true)
          .frame(maxWidth: .infinity, alignment: .leading)
          .padding(10)
      }
      if let plan = step.plan {
        ChatMarkdown(text: plan, size: .compact).padding(10)
      }
      if !step.todos.isEmpty {
        VStack(alignment: .leading, spacing: 6) {
          ForEach(Array(step.todos.enumerated()), id: \.offset) { _, todo in
            HStack(alignment: .firstTextBaseline, spacing: 8) {
              Image(
                systemName: todo.done
                  ? "checkmark.circle.fill" : todo.active ? "circle.dotted" : "circle"
              )
              .foregroundStyle(todo.done ? Color.green : Color.secondary)
              Text(todo.text).strikethrough(todo.done).foregroundStyle(
                todo.done ? .secondary : .primary)
            }
            .font(.system(size: 13))
          }
        }
        .padding(10)
      }
      if let subagent = step.subagent, let tail = subagent.tail {
        Text(tail).font(.system(size: 13)).foregroundStyle(.secondary).padding(10)
      }
      ForEach(Array(diffs.enumerated()), id: \.offset) { index, diff in
        if hasHeader || index > 0 { divider }
        DiffPreview(diff: diff)
      }
      if !diffs.isEmpty || (step.diffRef != nil && fetchedDiffs == nil && rpc != nil) {
        // An edit's diff says more than its "file updated" output.
      } else if let result = step.result {
        if hasHeader { divider }
        ToolOutput(value: result.content["output"]).padding(10)
      } else if let text = output ?? step.output?.string, !text.isEmpty {
        if hasHeader { divider }
        outputText(text)
      } else if let outputError {
        if hasHeader { divider }
        Text(outputError).font(.system(size: 12)).foregroundStyle(.secondary).padding(10)
      } else if step.outputRef != nil, rpc != nil {
        if hasHeader { divider }
        ProgressView().controlSize(.small).padding(10)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: 12, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.07), lineWidth: 1)
    )
    .task(id: step.id) { await load() }
  }
  /// The row already names the subject; commands and absolute paths repeat in full, unclipped.
  private var showsInvocation: Bool {
    let invocation = step.invocation
    guard !invocation.isEmpty, step.tool.category != .plan else { return false }
    return step.tool.category == .command || invocation.hasPrefix("/")
      || invocation != step.tool.detail
  }
  /// Something sits above the output, so a rule separates them.
  private var hasHeader: Bool {
    showsInvocation || step.plan != nil || !step.todos.isEmpty || step.subagent?.tail != nil
  }
  private var diffs: [TimelineDiff] {
    if let diff = step.diff { return [diff] }
    return fetchedDiffs ?? []
  }
  private var divider: some View { Divider().opacity(0.7) }
  private func outputText(_ text: String) -> some View {
    let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
    let clipped = !showsAllOutput && lines.count > outputLines
    let shown = clipped ? lines.prefix(outputLines).joined(separator: "\n") : text
    return VStack(alignment: .leading, spacing: 6) {
      Text(shown)
        .font(.system(size: 12, design: .monospaced))
        .foregroundStyle(step.failed ? Color.red.opacity(0.9) : Color.secondary)
        .textSelection(.enabled)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
      if clipped || (outputTotal.map { $0 > text.utf8.count } ?? false) {
        Button {
          change { showsAllOutput = true }
        } label: {
          Text(
            clipped
              ? "Show all \(lines.count) lines"
              : "Showing the first \(ByteCountFormatter.string(fromByteCount: Int64(text.utf8.count), countStyle: .file))"
          )
          .font(.system(size: 12, weight: .medium)).foregroundStyle(.secondary)
        }
        .buttonStyle(.plain).disabled(!clipped)
      }
    }
    .padding(10)
  }
  private func load() async {
    guard let rpc else { return }
    if step.diff == nil, fetchedDiffs == nil, let reference = step.diffRef {
      if let page = try? await Attachments.toolOutput(reference, rpc: rpc) {
        let decoded = TimelineDiff.decode(page.text)
        change { fetchedDiffs = decoded }
      }
    }
    guard step.result == nil, output == nil, diffs.isEmpty, let reference = step.outputRef
    else { return }
    do {
      let page = try await Attachments.toolOutput(reference, rpc: rpc)
      change {
        output = page.text
        outputTotal = page.total
      }
    } catch {
      change { outputError = "Couldn’t load the output: \(error.localizedDescription)" }
    }
  }
  private func change(_ update: () -> Void) {
    if let resize { resize(update) } else { update() }
  }
}

/// An edit's changed lines with a little context.
private struct DiffPreview: View {
  let diff: TimelineDiff
  private let limit = 40
  var body: some View {
    let lines = diff.lines()
    VStack(alignment: .leading, spacing: 0) {
      ForEach(lines.prefix(limit)) { line in
        HStack(alignment: .top, spacing: 6) {
          Text(marker(line.kind)).foregroundStyle(color(line.kind)).frame(width: 10)
          Text(line.text.isEmpty ? " " : line.text)
            .foregroundStyle(line.kind == .context || line.kind == .header ? .secondary : .primary)
            .fixedSize(horizontal: false, vertical: true)
          Spacer(minLength: 0)
        }
        .font(.system(size: 12, design: .monospaced))
        .padding(.horizontal, 10).padding(.vertical, 1)
        .background(background(line.kind))
      }
      if lines.count > limit {
        Text("\(lines.count - limit) more lines").font(.system(size: 12)).foregroundStyle(
          .secondary
        )
        .padding(10)
      }
    }
    .padding(.vertical, 6)
    .textSelection(.enabled)
  }
  private func marker(_ kind: DiffLine.Kind) -> String {
    switch kind {
    case .addition: "+"
    case .deletion: "−"
    default: " "
    }
  }
  private func color(_ kind: DiffLine.Kind) -> Color {
    switch kind {
    case .addition: .green
    case .deletion: .red
    default: .secondary
    }
  }
  private func background(_ kind: DiffLine.Kind) -> Color {
    switch kind {
    case .addition: Color.green.opacity(0.1)
    case .deletion: Color.red.opacity(0.1)
    default: .clear
    }
  }
}

extension ToolPresentation {
  /// The first line of some prose, for a collapsed preview.
  static func oneLineSummary(_ text: String) -> String {
    let line =
      text.split(whereSeparator: \.isNewline).first.map(String.init)?
      .trimmingCharacters(in: .whitespaces) ?? ""
    return line.replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "`", with: "")
  }
}
