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
private let gutter: CGFloat = 20

/// Type sizes the transcript shares, so every row reads as one voice.
private enum TextSize {
  static let body: CGFloat = 16
  static let meta: CGFloat = 14
  static let small: CGFloat = 12.5
  static let mono: CGFloat = 12.5
}

/// One transcript row: a message you sent, the agent's work or reply, or an event between them.
struct TimelineItemView: View {
  let item: TimelineItem
  let rpc: RPCClient?
  var respond: ((ChatMessage, String) async throws -> Void)?
  var resize: RowResize?
  /// Opens the computer's working changes, when a chat can show them.
  var openChanges: (() -> Void)?
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
    case .footer(let footer): TurnFooterView(footer: footer, openChanges: openChanges)
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
    case .waiting(_, let since):
      LiveLine(title: "Thinking…", since: since).padding(.vertical, 6)
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
  /// Inline Markdown only (`code`, emphasis, links), keeping your line breaks as typed.
  private var styledText: AttributedString {
    (try? AttributedString(
      markdown: user.text,
      options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
      ?? AttributedString(user.text)
  }
  var body: some View {
    VStack(alignment: .trailing, spacing: 6) {
      if !user.attachments.isEmpty {
        AttachmentRow(
          attachments: user.attachments, sessionID: user.sessionID, rpc: rpc, alignment: .trailing)
      }
      if !user.text.isEmpty {
        VStack(alignment: .leading, spacing: 6) {
          Text(styledText)
            .font(.system(size: TextSize.body))
            .lineSpacing(3)
            .lineLimit(folds && !showsAll ? 12 : nil)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
          if folds {
            Button(showsAll ? "Show less" : "Show more") {
              if let resize { resize { showsAll.toggle() } } else { showsAll.toggle() }
            }
            .font(.system(size: TextSize.meta, weight: .semibold)).opacity(0.8)
            .buttonStyle(.plain)
          }
        }
        .foregroundStyle(.white)
        .tint(.white)
        .padding(.horizontal, 15).padding(.vertical, 10)
        .background(AppTheme.userBubble, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .contextMenu {
          Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(user.text) }
          ShareLink(item: user.text)
        }
      }
    }
    .frame(maxWidth: .infinity, alignment: .trailing)
    .padding(.leading, 48)
    .padding(.top, 22).padding(.bottom, 8)
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

/// The answer itself: plain, well-set prose with no container.
private struct ReplyView: View {
  let reply: TimelineReply
  var body: some View {
    ChatMarkdown(text: reply.text, size: .reply)
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.top, 6).padding(.bottom, 2)
      .contextMenu {
        Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(reply.text) }
        ShareLink(item: reply.text)
      }
  }
}

/// How a settled turn ends: the files it changed, then quiet actions and how long it took.
private struct TurnFooterView: View {
  let footer: TimelineFooter
  var openChanges: (() -> Void)?
  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      if !footer.changes.isEmpty {
        ChangesCard(footer: footer, open: openChanges)
          .padding(.top, 4)
      }
      HStack(spacing: 0) {
        if !footer.text.isEmpty {
          CopyButton(label: "Copy reply") { NativePlatform.copy(footer.text) }
          ShareLink(item: footer.text) {
            Image(systemName: "square.and.arrow.up")
              .font(.system(size: 13, weight: .medium)).foregroundStyle(.secondary)
              .frame(width: 30, height: 30).contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .accessibilityLabel("Share reply")
        }
        if let meta {
          Text(meta)
            .font(.system(size: 12)).monospacedDigit()
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .padding(.leading, footer.text.isEmpty ? 0 : 6)
        }
        Spacer(minLength: 0)
      }
      .padding(.leading, footer.text.isEmpty ? 0 : -7)
    }
    .padding(.top, 2).padding(.bottom, 14)
    .accessibilityElement(children: .contain)
  }
  /// "Worked for 17s · 9:41 AM".
  private var meta: String? {
    let time = footer.finishedAt.map { $0.formatted(date: .omitted, time: .shortened) }
    let parts = [footer.worked, time].compactMap { $0 }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
  }
}

/// "calc.py  +5 −1" or "3 files  +12 −4", opening the computer's changes.
private struct ChangesCard: View {
  let footer: TimelineFooter
  var open: (() -> Void)?
  private let listLimit = 4
  var body: some View {
    Button {
      open?()
    } label: {
      VStack(alignment: .leading, spacing: 0) {
        HStack(spacing: 8) {
          Image(systemName: "doc.text").font(.system(size: 12, weight: .medium))
            .foregroundStyle(.secondary)
          Text(title).font(.system(size: 13.5, weight: .medium)).foregroundStyle(.primary)
            .lineLimit(1).truncationMode(.middle)
          LineCounts(additions: footer.additions, deletions: footer.deletions)
          if open != nil {
            Spacer(minLength: 6)
            Text("Review").font(.system(size: 12.5, weight: .medium)).foregroundStyle(.secondary)
            Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold))
              .foregroundStyle(.tertiary)
          }
        }
        .padding(.horizontal, 12).frame(minHeight: 38)
        if footer.changes.count > 1 {
          Rectangle().fill(AppTheme.hairline).frame(height: 1)
          VStack(alignment: .leading, spacing: 0) {
            ForEach(footer.changes.prefix(listLimit), id: \.path) { change in
              HStack(spacing: 8) {
                Text(ToolPresentation.fileName(change.path))
                  .font(.system(size: TextSize.mono, design: .monospaced)).foregroundStyle(
                    .secondary
                  )
                  .lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 8)
                LineCounts(additions: change.additions, deletions: change.deletions, size: 11.5)
              }
              .frame(minHeight: 26)
            }
            if footer.changes.count > listLimit {
              Text("\(footer.changes.count - listLimit) more")
                .font(.system(size: 12)).foregroundStyle(.tertiary).frame(minHeight: 24)
            }
          }
          .padding(.horizontal, 12).padding(.vertical, 4)
        }
      }
      .frame(maxWidth: footer.changes.count > 1 ? 420 : nil, alignment: .leading)
      .fixedSize(horizontal: footer.changes.count == 1, vertical: false)
      .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: 12, style: .continuous)
          .strokeBorder(AppTheme.hairline, lineWidth: 1)
      )
      .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
    .buttonStyle(.plain)
    // Without a computer to ask, the card still reads normally; it just doesn't open anything.
    .allowsHitTesting(open != nil)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(
      "\(title), \(footer.additions) lines added, \(footer.deletions) removed"
    )
    .accessibilityHint(open == nil ? "" : "Opens the changes")
    .accessibilityIdentifier("turn-changes")
  }
  private var title: String {
    footer.changes.count == 1
      ? ToolPresentation.fileName(footer.changes[0].path)
      : "\(footer.changes.count) files changed"
  }
}

/// "+5 −1" in the diff colours.
private struct LineCounts: View {
  let additions: Int
  let deletions: Int
  var size: CGFloat = 12.5
  var body: some View {
    HStack(spacing: 4) {
      Text("+\(additions)").foregroundStyle(AppTheme.added)
      Text("−\(deletions)").foregroundStyle(AppTheme.removed)
    }
    .font(.system(size: size, weight: .medium, design: .monospaced))
    .lineLimit(1).fixedSize()
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

/// A running turn's one line: a spinner, what it's doing with a soft shimmer, and a timer.
private struct LiveLine: View {
  let title: String
  let since: Date?
  var body: some View {
    HStack(spacing: 8) {
      ProgressView().controlSize(.mini).frame(width: 14)
      ShimmerText(text: title, size: TextSize.meta)
        .lineLimit(1).truncationMode(.middle)
        .layoutPriority(1)
      if let since { Elapsed(since: since).foregroundStyle(.tertiary).fixedSize() }
      Spacer(minLength: 0)
    }
    .frame(minHeight: 32)
    .frame(maxWidth: .infinity, alignment: .leading)
    .accessibilityElement(children: .combine)
  }
}

/// Muted text with a band of light passing over it; still when motion is reduced.
private struct ShimmerText: View {
  let text: String
  let size: CGFloat
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  var body: some View {
    let base = Text(text).font(.system(size: size))
    if reduceMotion {
      base.foregroundStyle(.secondary)
    } else {
      base.foregroundStyle(.tertiary)
        .overlay {
          TimelineView(.animation(minimumInterval: 1 / 30)) { context in
            let period = 1.8
            let phase =
              context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: period)
              / period
            base.foregroundStyle(.primary)
              .mask {
                GeometryReader { geometry in
                  let band = max(48, geometry.size.width * 0.4)
                  LinearGradient(
                    colors: [.clear, .black, .clear], startPoint: .leading, endPoint: .trailing
                  )
                  .frame(width: band)
                  .offset(x: -band + (geometry.size.width + band) * phase)
                }
              }
          }
        }
    }
  }
}

// MARK: - Events

private struct NoticeView: View {
  let notice: TimelineNotice
  var body: some View {
    switch notice.kind {
    case .error:
      HStack(alignment: .firstTextBaseline, spacing: 9) {
        Image(systemName: "exclamationmark.circle.fill")
          .font(.system(size: 13)).foregroundStyle(AppTheme.removed)
        Text(notice.title).font(.system(size: TextSize.meta)).foregroundStyle(.secondary)
          .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 12).padding(.vertical, 10)
      .background(
        AppTheme.removed.opacity(0.07), in: RoundedRectangle(cornerRadius: 12, style: .continuous)
      )
      .padding(.vertical, 6)
      .contextMenu {
        Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(notice.title) }
      }
      .accessibilityElement(children: .combine)
      .accessibilityLabel("Error, \(notice.title)")
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
          Text(notice.title).font(.system(size: TextSize.meta, weight: .medium))
          if let detail = notice.detail {
            Text(detail).font(.system(size: 13)).foregroundStyle(.secondary)
          }
        }
        Spacer(minLength: 0)
      }
      .padding(12)
      .background(
        AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 12, style: .continuous)
      )
      .padding(.vertical, 6)
    }
  }
  private var rule: some View {
    Rectangle().fill(AppTheme.hairline).frame(height: 1)
  }
}

// MARK: - The agent's work

/// One quiet line per turn ("Read 2 files · edited calc.py · ran 2 commands") that opens onto a
/// timeline of every step. A running turn shows one live line instead.
struct ActivityView: View {
  let activity: TimelineActivity
  let rpc: RPCClient?
  var resize: RowResize?
  /// Set only by a tap; work stays folded until you open it, running or not.
  @State private var expanded = TranscriptPreviewFlags.expandActivity
  @State private var showsEarlier = false
  private var expandable: Bool { !activity.steps.isEmpty }
  /// Long turns open on their latest steps.
  private let recentLimit = 10
  private var hiddenCount: Int {
    showsEarlier ? 0 : max(0, activity.steps.count - recentLimit)
  }
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      header
      if expanded && expandable {
        let shown = Array(activity.steps.suffix(activity.steps.count - hiddenCount))
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
            .padding(.leading, StepRow.textInset)
          }
          ForEach(Array(shown.enumerated()), id: \.element.id) { index, step in
            StepRow(
              step: step, first: index == 0 && hiddenCount == 0, last: index == shown.count - 1,
              rpc: rpc, resize: resize)
          }
        }
        .padding(.top, 2).padding(.bottom, 8)
      }
    }
    .padding(.vertical, 2)
    .frame(maxWidth: .infinity, alignment: .leading)
  }
  @ViewBuilder private var header: some View {
    Button {
      guard expandable else { return }
      change { expanded.toggle() }
    } label: {
      HStack(spacing: 6) {
        if activity.live {
          LiveLine(title: activity.liveTitle, since: activity.startedAt)
            .fixedSize(horizontal: false, vertical: true)
        } else {
          // One flowing line (two at most), so failures and the chevron stay with the words.
          summary
            .font(.system(size: TextSize.meta))
            .lineLimit(2)
            .fixedSize(horizontal: false, vertical: true)
          Spacer(minLength: 0)
        }
      }
      .frame(minHeight: 32).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .disabled(!expandable)
    .accessibilityLabel(activity.title)
    .accessibilityValue(expandable ? (expanded ? "Expanded" : "Collapsed") : "")
    .accessibilityHint(expandable ? "Shows each step" : "")
    .accessibilityIdentifier("tool-group-\(activity.id)")
  }
  private var summary: Text {
    var text = Text(activity.title).foregroundStyle(.secondary)
    if activity.failedCount > 0 {
      text = text + Text(" · \(activity.failedCount) failed").foregroundStyle(AppTheme.removed)
    }
    if expandable {
      text =
        text + Text("\u{00A0}\u{00A0}\u{2060}")
        + Text(Image(systemName: expanded ? "chevron.down" : "chevron.right"))
        .font(.system(size: 10, weight: .semibold)).foregroundStyle(.tertiary)
    }
    return text
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

/// One step on the activity timeline: a glyph on a hairline rail, a verb and its target. Tool
/// calls open onto their output; edits show their diff straight away.
private struct StepRow: View {
  let step: TimelineStep
  let first: Bool
  let last: Bool
  let rpc: RPCClient?
  var resize: RowResize?
  @State private var expanded = TranscriptPreviewFlags.expandSteps
  /// Where step text starts: past the glyph column.
  static let textInset: CGFloat = 26
  private static let glyph: CGFloat = 16
  private static let lineHeight: CGFloat = 26
  var body: some View {
    HStack(alignment: .top, spacing: 0) {
      Color.clear.frame(width: Self.textInset, height: 1)
      VStack(alignment: .leading, spacing: 6) {
        content
      }
      .padding(.bottom, last ? 0 : 4)
    }
    .background(alignment: .topLeading) { rail }
    .overlay(alignment: .topLeading) { glyph }
  }
  /// A hairline joining the glyphs, broken around each one. Rows touch, so it runs unbroken
  /// from the first step to the last.
  private var rail: some View {
    GeometryReader { geometry in
      let x = Self.glyph / 2
      let middle = Self.lineHeight / 2
      Path { path in
        if !first {
          path.move(to: CGPoint(x: x, y: 0))
          path.addLine(to: CGPoint(x: x, y: middle - 9))
        }
        if !last {
          path.move(to: CGPoint(x: x, y: middle + 9))
          path.addLine(to: CGPoint(x: x, y: geometry.size.height))
        }
      }
      .stroke(AppTheme.hairline, lineWidth: 1)
    }
    .accessibilityHidden(true)
  }
  private var glyph: some View {
    Group {
      if step.running {
        ProgressView().controlSize(.mini)
      } else {
        Image(systemName: symbol)
          .font(.system(size: 11, weight: .medium))
          .foregroundStyle(step.failed ? AppTheme.removed : Color.secondary)
      }
    }
    .frame(width: Self.glyph, height: Self.lineHeight)
    .accessibilityHidden(true)
  }
  private var symbol: String {
    switch step.kind {
    case .thinking: "brain"
    case .narration: "text.bubble"
    case .tool:
      if let subagent = step.subagent {
        subagent.status == .failed ? "xmark.circle" : "checkmark.circle"
      } else {
        step.tool.symbol
      }
    }
  }
  @ViewBuilder private var content: some View {
    switch step.kind {
    case .narration:
      ChatMarkdown(text: step.text, size: .compact)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 4)
    case .thinking:
      // Reasoning reads as an aside: italic, muted, no box.
      Button {
        toggle()
      } label: {
        Text(
          step.redacted
            ? "Reasoning is private." : step.text.trimmingCharacters(in: .whitespacesAndNewlines)
        )
        .font(.system(size: 14)).italic()
        .foregroundStyle(.secondary)
        .lineSpacing(2)
        .lineLimit(expanded ? nil : 3)
        .multilineTextAlignment(.leading)
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 4)
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel("Reasoning")
      .accessibilityValue(step.redacted ? "Private" : step.text)
      .accessibilityIdentifier("activity-\(step.id)")
    case .tool:
      let tool = step.tool
      if let subagent = step.subagent {
        line(
          label: tool.detail.isEmpty ? "Agent" : tool.detail,
          detail: subagent.tail ?? status(subagent.status), mono: false)
      } else {
        line(label: tool.label, detail: shortPath(tool), mono: true)
      }
      if showsDiff {
        EditCard(step: step, rpc: rpc, resize: resize, open: expanded)
      } else if expanded {
        ToolDetailView(step: step, rpc: rpc, resize: resize)
      }
    }
  }
  /// Edits show their change inline, like a code review.
  private var showsDiff: Bool {
    guard step.kind == .tool, !step.failed else { return false }
    let category = step.tool.category
    return (category == .edit || category == .write)
      && (step.diff != nil || (step.diffRef != nil && rpc != nil))
  }
  private func toggle() {
    if let resize { resize { expanded.toggle() } } else { expanded.toggle() }
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
  private func line(label: String, detail: String, mono: Bool) -> some View {
    Button {
      toggle()
    } label: {
      HStack(alignment: .firstTextBaseline, spacing: 6) {
        Text(label).font(.system(size: 13.5, weight: .medium)).foregroundStyle(.secondary)
          .lineLimit(1).layoutPriority(1)
        if !detail.isEmpty {
          Text(detail)
            .font(mono ? .system(size: TextSize.mono, design: .monospaced) : .system(size: 13.5))
            .foregroundStyle(.secondary.opacity(0.85))
            .lineLimit(1).truncationMode(mono ? .middle : .tail)
        }
        if step.additions > 0 || step.deletions > 0 {
          LineCounts(additions: step.additions, deletions: step.deletions, size: 11.5)
        }
        if step.failed {
          Text("failed").font(.system(size: 12, weight: .medium))
            .foregroundStyle(AppTheme.removed).fixedSize()
        }
        Spacer(minLength: 0)
      }
      .frame(minHeight: Self.lineHeight).contentShape(Rectangle())
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

/// An edit as a small review card: the file, its line counts and the changed lines.
private struct EditCard: View {
  let step: TimelineStep
  let rpc: RPCClient?
  var resize: RowResize?
  /// The step was opened, so the whole change shows.
  var open: Bool
  @State private var fetched: [TimelineDiff]?
  @State private var showsAll = false
  private let previewLines = 8
  var body: some View {
    let diffs = step.diff.map { [$0] } ?? fetched ?? []
    VStack(alignment: .leading, spacing: 0) {
      if diffs.isEmpty {
        ProgressView().controlSize(.small).padding(12)
      }
      ForEach(Array(diffs.enumerated()), id: \.offset) { index, diff in
        if index > 0 { Rectangle().fill(AppTheme.hairline).frame(height: 1) }
        lines(diff)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: 10, style: .continuous)
        .strokeBorder(AppTheme.hairline, lineWidth: 1)
    )
    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    .padding(.bottom, 4)
    .task(id: step.id) { await load() }
  }
  private func lines(_ diff: TimelineDiff) -> some View {
    let all = diff.lines(context: 1)
    let limit = open || showsAll ? 200 : previewLines
    return VStack(alignment: .leading, spacing: 0) {
      VStack(alignment: .leading, spacing: 0) {
        ForEach(all.prefix(limit)) { line in
          DiffRow(line: line)
        }
      }
      .padding(.vertical, 6)
      if all.count > limit {
        Button {
          if let resize { resize { showsAll = true } } else { showsAll = true }
        } label: {
          Text("Show \(all.count - limit) more \(all.count - limit == 1 ? "line" : "lines")")
            .font(.system(size: 12, weight: .medium)).foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12).frame(minHeight: 30)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .top) { Rectangle().fill(AppTheme.hairline).frame(height: 1) }
      }
    }
    .textSelection(.enabled)
  }
  private func load() async {
    guard step.diff == nil, fetched == nil, let reference = step.diffRef, let rpc else { return }
    if let page = try? await Attachments.toolOutput(reference, rpc: rpc) {
      let decoded = TimelineDiff.decode(page.text)
      if let resize { resize { fetched = decoded } } else { fetched = decoded }
    }
  }
}

/// One line of a diff: a sign column and the text, tinted when it changed.
private struct DiffRow: View {
  let line: DiffLine
  var body: some View {
    HStack(alignment: .top, spacing: 8) {
      Text(marker).foregroundStyle(tint).frame(width: 8)
      Text(line.text.isEmpty ? " " : line.text)
        .foregroundStyle(line.kind == .addition || line.kind == .deletion ? .primary : .secondary)
        .fixedSize(horizontal: false, vertical: true)
      Spacer(minLength: 0)
    }
    .font(.system(size: 11.5, design: .monospaced))
    .lineSpacing(1)
    .padding(.horizontal, 10).padding(.vertical, 1.5)
    .background(background.padding(.vertical, -0.5))
  }
  private var marker: String {
    switch line.kind {
    case .addition: "+"
    case .deletion: "−"
    case .header: "⋯"
    default: " "
    }
  }
  private var tint: Color {
    switch line.kind {
    case .addition: AppTheme.added
    case .deletion: AppTheme.removed
    default: .secondary
    }
  }
  private var background: Color {
    switch line.kind {
    case .addition: AppTheme.added.opacity(0.1)
    case .deletion: AppTheme.removed.opacity(0.09)
    default: .clear
    }
  }
}

/// What a tool call did: the invocation, then its output or checklist, in one restrained card.
private struct ToolDetailView: View {
  let step: TimelineStep
  let rpc: RPCClient?
  var resize: RowResize?
  @State private var output: String?
  @State private var outputTotal: Int?
  @State private var outputError: String?
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
          .padding(.horizontal, 12).padding(.vertical, 9)
      }
      if let plan = step.plan {
        ChatMarkdown(text: plan, size: .compact).padding(12)
      }
      if !step.todos.isEmpty {
        VStack(alignment: .leading, spacing: 6) {
          ForEach(Array(step.todos.enumerated()), id: \.offset) { _, todo in
            HStack(alignment: .firstTextBaseline, spacing: 8) {
              Image(
                systemName: todo.done
                  ? "checkmark.circle.fill" : todo.active ? "circle.dotted" : "circle"
              )
              .foregroundStyle(todo.done ? AppTheme.added : Color.secondary)
              Text(todo.text).strikethrough(todo.done).foregroundStyle(
                todo.done ? .secondary : .primary)
            }
            .font(.system(size: 13))
          }
        }
        .padding(12)
      }
      if let subagent = step.subagent, let tail = subagent.tail {
        Text(tail).font(.system(size: 13)).foregroundStyle(.secondary).padding(12)
      }
      if let result = step.result {
        if hasHeader { divider }
        ToolOutput(value: result.content["output"]).padding(12)
      } else if let text = output ?? step.output?.string, !text.isEmpty {
        if hasHeader { divider }
        outputText(text)
      } else if let outputError {
        if hasHeader { divider }
        Text(outputError).font(.system(size: 12)).foregroundStyle(.secondary).padding(12)
      } else if step.outputRef != nil, rpc != nil {
        if hasHeader { divider }
        ProgressView().controlSize(.small).padding(12)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .background(AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    .padding(.bottom, 4)
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
  private var divider: some View { Rectangle().fill(AppTheme.hairline).frame(height: 1) }
  private func outputText(_ text: String) -> some View {
    let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
    let clipped = !showsAllOutput && lines.count > outputLines
    let shown = clipped ? lines.prefix(outputLines).joined(separator: "\n") : text
    return VStack(alignment: .leading, spacing: 6) {
      Text(shown)
        .font(.system(size: 12, design: .monospaced))
        .foregroundStyle(step.failed ? AppTheme.removed : Color.secondary)
        .lineSpacing(1.5)
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
    .padding(.horizontal, 12).padding(.vertical, 10)
  }
  private func load() async {
    guard let rpc, step.result == nil, output == nil, let reference = step.outputRef else {
      return
    }
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
