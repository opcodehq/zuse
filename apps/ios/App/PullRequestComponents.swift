import SwiftUI
import Textual
import ZuseCore

/// Opens a pull request link in the app (posted by link handlers, handled by `WorkspaceView`).
extension Notification.Name {
  static let nativeOpenPullRequest = Notification.Name("sh.zuse.native.open-pull-request")
}

extension View {
  /// GitHub pull request links open in the app; every other link opens as usual.
  func pullRequestLinks() -> some View {
    environment(
      \.openURL,
      OpenURLAction { url in
        guard let link = PullRequestLink(url: url) else { return .systemAction }
        NotificationCenter.default.post(name: .nativeOpenPullRequest, object: link.url)
        return .handled
      })
  }
}

/// State colours shared by every pull request surface.
enum PullRequestTint {
  static let open = Color(red: 0.16, green: 0.62, blue: 0.29)
  static let merged = Color(red: 0.51, green: 0.31, blue: 0.86)
  static let closed = Color(red: 0.85, green: 0.24, blue: 0.22)
  static let draft = Color.secondary
  static let addition = Color(red: 0.13, green: 0.6, blue: 0.27)
  static let deletion = Color(red: 0.84, green: 0.23, blue: 0.2)
  static let attention = Color(red: 0.86, green: 0.45, blue: 0.05)

  static func state(_ state: PullRequestState, draft: Bool) -> Color {
    switch state {
    case .merged: return merged
    case .closed: return closed
    case .open: return draft ? Self.draft : open
    }
  }
}

/// The pull request mark, tinted and shaped by state.
struct PullRequestGlyph: View {
  let state: PullRequestState
  var draft = false
  var size: CGFloat = 15
  var body: some View {
    PullRequestGlyphShape(kind: kind)
      .stroke(
        PullRequestTint.state(state, draft: draft),
        style: StrokeStyle(lineWidth: max(1.3, size / 10), lineCap: .round, lineJoin: .round)
      )
      .frame(width: size, height: size)
      .accessibilityElement()
      .accessibilityLabel(label)
  }
  private var kind: PullRequestGlyphShape.Kind {
    switch state {
    case .merged: return .merged
    case .closed: return .closed
    case .open: return .open
    }
  }
  private var label: String {
    switch state {
    case .merged: return "Merged"
    case .closed: return "Closed"
    case .open: return draft ? "Draft" : "Open"
    }
  }
}

/// Two commits joined by a branch: open (an arrow back to the base), merged (the branch folds
/// in) or closed (a cross where the arrow was). Drawn on a 16-point grid.
struct PullRequestGlyphShape: Shape {
  enum Kind { case open, merged, closed }
  let kind: Kind
  func path(in rect: CGRect) -> Path {
    let scale = min(rect.width, rect.height) / 16
    func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
      CGPoint(x: rect.minX + x * scale, y: rect.minY + y * scale)
    }
    func ring(_ x: CGFloat, _ y: CGFloat) -> CGRect {
      CGRect(
        x: rect.minX + (x - 2) * scale, y: rect.minY + (y - 2) * scale, width: 4 * scale,
        height: 4 * scale)
    }
    var path = Path()
    path.addEllipse(in: ring(4, 3.5))
    path.addEllipse(in: ring(4, 12.5))
    path.move(to: point(4, 5.5))
    path.addLine(to: point(4, 10.5))
    switch kind {
    case .open:
      path.addEllipse(in: ring(12, 12.5))
      path.move(to: point(12, 10.5))
      path.addLine(to: point(12, 6.5))
      path.addQuadCurve(to: point(9.5, 3.5), control: point(12, 3.5))
      path.addLine(to: point(7.5, 3.5))
      path.move(to: point(9.5, 1.5))
      path.addLine(to: point(7.5, 3.5))
      path.addLine(to: point(9.5, 5.5))
    case .merged:
      path.addEllipse(in: ring(12, 8))
      path.move(to: point(4, 5.5))
      path.addQuadCurve(to: point(10, 8), control: point(4.5, 8))
    case .closed:
      path.addEllipse(in: ring(12, 12.5))
      path.move(to: point(12, 10.5))
      path.addLine(to: point(12, 8))
      path.move(to: point(10.5, 2))
      path.addLine(to: point(13.5, 5))
      path.move(to: point(13.5, 2))
      path.addLine(to: point(10.5, 5))
    }
    return path
  }
}

/// A GitHub avatar, or a tinted initial while it loads or when there is none.
struct PullRequestAvatar: View {
  let actor: PullRequestActor
  var size: CGFloat = 18
  var body: some View {
    let shape = RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
    Group {
      if let url = actor.avatarURL {
        AsyncImage(url: url) { phase in
          if let image = phase.image {
            image.resizable().scaledToFill()
          } else {
            initial
          }
        }
      } else {
        initial
      }
    }
    .frame(width: size, height: size)
    .clipShape(shape)
    .overlay { shape.strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5) }
    .accessibilityHidden(true)
  }
  private var initial: some View {
    let name = actor.displayName
    let palette: [Color] = [
      .orange, .pink, .purple, .indigo, .teal, .mint, .blue, .brown, .cyan,
    ]
    let index = name.unicodeScalars.reduce(0) { ($0 &* 31 &+ Int($1.value)) & 0xFFFF }
    let color = actor.isBot ? Color.gray : palette[index % palette.count]
    return ZStack {
      color.opacity(0.85)
      if actor.isBot {
        Image(systemName: "gearshape.fill").font(.system(size: size * 0.5, weight: .semibold))
      } else {
        Text(String(name.prefix(1)).uppercased())
          .font(.system(size: size * 0.52, weight: .bold, design: .rounded))
      }
    }.foregroundStyle(.white)
  }
}

/// A small grey token on the detail card: status, branch, size, reviewers.
struct PullRequestChip<Content: View>: View {
  @ViewBuilder let content: Content
  var body: some View {
    HStack(spacing: 6) { content }
      .font(.system(size: 14))
      .padding(.horizontal, 9).frame(minHeight: 30)
      .background(
        Color.primary.opacity(0.055), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
  }
}

/// `+815 −120`, green and red.
struct PullRequestLineCounts: View {
  let additions: Int
  let deletions: Int
  var size: CGFloat = 14
  var body: some View {
    HStack(spacing: 4) {
      Text("+\(additions.formatted())").foregroundStyle(PullRequestTint.addition)
      Text("−\(deletions.formatted())").foregroundStyle(PullRequestTint.deletion)
    }
    .font(.system(size: size, weight: .medium).monospacedDigit())
    .accessibilityElement(children: .ignore)
    .accessibilityLabel("\(additions) additions, \(deletions) deletions")
  }
}

/// Wraps children onto as many lines as they need.
struct PullRequestFlow: Layout {
  var spacing: CGFloat = 8
  var lineSpacing: CGFloat = 8
  func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
    let rows = arrange(width: proposal.width ?? .infinity, subviews: subviews)
    let height = rows.map(\.height).reduce(0, +) + lineSpacing * CGFloat(max(0, rows.count - 1))
    let width = rows.map(\.width).max() ?? 0
    return CGSize(width: proposal.width ?? width, height: height)
  }
  func placeSubviews(
    in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()
  ) {
    var y = bounds.minY
    for row in arrange(width: bounds.width, subviews: subviews) {
      var x = bounds.minX
      for index in row.indices {
        let size = subviews[index].sizeThatFits(.init(width: bounds.width, height: nil))
        subviews[index].place(
          at: CGPoint(x: x, y: y + (row.height - size.height) / 2), anchor: .topLeading,
          proposal: .init(width: min(size.width, bounds.width), height: size.height))
        x += min(size.width, bounds.width) + spacing
      }
      y += row.height + lineSpacing
    }
  }
  private struct Row {
    var indices: [Int] = []
    var width: CGFloat = 0
    var height: CGFloat = 0
  }
  private func arrange(width: CGFloat, subviews: Subviews) -> [Row] {
    var rows: [Row] = [Row()]
    for (index, subview) in subviews.enumerated() {
      let size = subview.sizeThatFits(.init(width: width, height: nil))
      let itemWidth = min(size.width, width)
      if !rows[rows.count - 1].indices.isEmpty
        && rows[rows.count - 1].width + spacing + itemWidth > width
      {
        rows.append(Row())
      }
      var row = rows[rows.count - 1]
      row.width += (row.indices.isEmpty ? 0 : spacing) + itemWidth
      row.height = max(row.height, size.height)
      row.indices.append(index)
      rows[rows.count - 1] = row
    }
    return rows.filter { !$0.indices.isEmpty }
  }
}

/// A comment or description body: GitHub Markdown, with its HTML turned into Markdown.
struct PullRequestMarkdownText: View {
  let markdown: String
  var size: CGFloat = 16
  var body: some View {
    StructuredText(markdown: PullRequestMarkdown.clean(markdown))
      .font(.system(size: size))
      .textual.headingStyle(CompactHeading())
      .textual.structuredTextStyle(.gitHub)
      .textual.listItemSpacing(.fontScaled(top: 0.15, bottom: 0.15))
      .textual.textSelection(.enabled)
      .frame(maxWidth: .infinity, alignment: .leading)
  }
}

/// GitHub's headings, scaled for a phone: a little larger than body text, no rules.
private struct CompactHeading: StructuredText.HeadingStyle {
  private static let scales: [CGFloat] = [1.25, 1.15, 1.06, 1, 0.94, 0.9]
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .textual.fontScale(Self.scales[min(max(configuration.headingLevel, 1), 6) - 1])
      .textual.blockSpacing(.fontScaled(top: 0.9, bottom: 0.35))
      .fontWeight(.semibold)
  }
}

/// A rounded card on the page background.
struct PullRequestCard<Content: View>: View {
  var padding: CGFloat = 14
  @ViewBuilder let content: Content
  var body: some View {
    VStack(alignment: .leading, spacing: 10) { content }
      .padding(padding)
      .frame(maxWidth: .infinity, alignment: .leading)
      .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
      .overlay {
        RoundedRectangle(cornerRadius: 18, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.05), lineWidth: 0.5)
      }
  }
}

/// Explains why pull requests can't be shown, with a way forward.
struct PullRequestUnavailable: View {
  let title: String
  let message: String
  var symbol = "arrow.triangle.pull"
  var retry: (() -> Void)?
  var body: some View {
    ContentUnavailableView {
      Label(title, systemImage: symbol)
    } description: {
      Text(message)
    } actions: {
      if let retry { Button("Try again", action: retry).buttonStyle(.bordered) }
    }
  }
}
