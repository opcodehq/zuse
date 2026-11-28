import SwiftUI
import ZuseCore

/// A Zapper's picture: its own image when it has one, otherwise its pixel mascot.
struct ZapperAvatarView: View {
  let zapper: Zapper
  var size: CGFloat = 28
  var body: some View {
    Group {
      if let image = Self.image(zapper.config.avatarImage) {
        Image(nativeImage: image).resizable().scaledToFill()
          .frame(width: size, height: size)
          .clipShape(RoundedRectangle(cornerRadius: size * 0.2, style: .continuous))
      } else {
        ZapperMascotView(avatar: zapper.avatar, resting: zapper.status != .active, size: size)
      }
    }
    .frame(width: size, height: size)
    .accessibilityHidden(true)
  }
  private static func image(_ url: String?) -> NativeImage? {
    guard let url, let comma = url.firstIndex(of: ","), url.hasPrefix("data:image/"),
      let data = Data(base64Encoded: String(url[url.index(after: comma)...]))
    else { return nil }
    return NativeImage(data: data)
  }
}

/// The shared pixel mascot, drawn cell by cell like every other client.
struct ZapperMascotView: View {
  let avatar: ZapperAvatar
  var resting = false
  var size: CGFloat = 28
  var body: some View {
    let (mascot, color) = ZapperMascots.resolve(avatar)
    let grid = ZapperMascots.grid(for: Double(size))
    let cells = ZapperMascots.raster(mascot, grid: grid, blink: resting ? 1 : 0)
    Canvas { context, canvas in
      let cell = canvas.width / CGFloat(grid)
      var bodyPath = Path()
      var eyePath = Path()
      for (index, kind) in cells.enumerated() {
        guard let kind else { continue }
        // A hair of overlap hides seams between cells.
        let rect = CGRect(
          x: CGFloat(index % grid) * cell, y: CGFloat(index / grid) * cell, width: cell + 0.3,
          height: cell + 0.3)
        if kind == .eye { eyePath.addRect(rect) } else { bodyPath.addRect(rect) }
      }
      context.fill(bodyPath, with: .color(Color(hex: color.body)))
      context.fill(eyePath, with: .color(Color(hex: ZapperMascots.eye)))
    }
    .frame(width: size, height: size)
  }
}

extension Color {
  init(hex: UInt32) {
    self.init(
      red: Double((hex >> 16) & 0xff) / 255, green: Double((hex >> 8) & 0xff) / 255,
      blue: Double(hex & 0xff) / 255)
  }
}

/// A thread's status in the same small slot everywhere it is listed.
struct ZapperThreadMark: View {
  let rank: ZapperThreadRank
  var body: some View {
    Group {
      switch rank {
      case .working: ProgressView().controlSize(.mini)
      case .needsYou: Circle().fill(.orange).frame(width: 7, height: 7)
      case .queued:
        Image(systemName: "clock").font(.system(size: 11, weight: .medium))
          .foregroundStyle(.secondary)
      case .idle:
        Circle().strokeBorder(Color.secondary.opacity(0.5), lineWidth: 1.2)
          .frame(width: 8, height: 8)
      }
    }
    .frame(width: 16, height: 16)
    .accessibilityElement()
    .accessibilityLabel(rank.label)
  }
}

/// The count of things waiting on the user, in the app's warning colour.
struct ZapperNeedsYouBadge: View {
  let count: Int
  var body: some View {
    Text("\(count)").font(.system(size: 12, weight: .semibold)).monospacedDigit()
      .foregroundStyle(.white)
      .padding(.horizontal, 6).frame(minWidth: 20, minHeight: 20)
      .background(Color.orange, in: Capsule())
      .accessibilityLabel(
        count == 1 ? "1 thing waiting for you" : "\(count) things waiting for you")
  }
}

extension ZapperStatus {
  var label: String {
    switch self {
    case .active: return "Active"
    case .paused: return "Paused"
    case .stopped: return "Stopped"
    case .archived: return "Archived"
    }
  }
  var tint: Color {
    switch self {
    case .active: return .green
    case .paused: return .orange
    case .stopped, .archived: return .secondary
    }
  }
}

extension ZapperIssuePriority {
  var symbol: String {
    switch self {
    case .urgent: return "exclamationmark.square.fill"
    case .high: return "chart.bar.fill"
    case .normal: return "chart.bar"
    case .low: return "minus"
    }
  }
  var tint: Color {
    switch self {
    case .urgent: return .orange
    case .high, .normal: return .primary.opacity(0.7)
    case .low: return .secondary
    }
  }
}

extension ZapperIssueStatus {
  var symbol: String {
    switch self {
    case .backlog: return "circle.dashed"
    case .todo: return "circle"
    case .inProgress: return "circle.lefthalf.filled"
    case .needsYou: return "questionmark.circle.fill"
    case .blocked: return "exclamationmark.circle"
    case .inReview: return "eye.circle"
    case .done: return "checkmark.circle.fill"
    case .canceled: return "xmark.circle"
    }
  }
  var tint: Color {
    switch self {
    case .needsYou: return .orange
    case .inProgress: return .yellow
    case .inReview: return .green
    case .blocked: return .red
    case .done: return .purple
    case .backlog, .todo, .canceled: return .secondary
    }
  }
}

/// Unix milliseconds as "5 min ago".
func zapperRelativeTime(_ milliseconds: Int64) -> String {
  let date = Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1000)
  if abs(date.timeIntervalSinceNow) < 60 { return "Just now" }
  return date.formatted(.relative(presentation: .named, unitsStyle: .abbreviated))
}

/// A harness id as people say it: `claude-code` is "Claude Code".
func zapperHarnessName(_ id: String) -> String {
  switch id.lowercased() {
  case "claude-code", "claude": return "Claude Code"
  case "codex": return "Codex"
  case "opencode": return "OpenCode"
  case "oh-my-pi": return "Oh My Pi"
  default:
    return id.split(separator: "-").map { $0.prefix(1).uppercased() + $0.dropFirst() }
      .joined(separator: " ")
  }
}
