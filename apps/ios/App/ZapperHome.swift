import SwiftUI
import ZuseCore

/// The home list's Zappers: one row each, like a chat. Opening a Zapper opens its
/// conversation; its threads, issues and docs live on its overview page.
struct ZapperHomeSection: View {
  let workspace: WorkspaceModel
  let search: String
  let filter: ArchiveFilter
  let open: (String) -> Void

  private var store: ZapperStore { workspace.zappers }

  private func matches(_ zapper: Zapper) -> Bool {
    switch filter {
    case .active: if zapper.status == .archived { return false }
    case .archived: if zapper.status != .archived { return false }
    case .all: break
    }
    let query = search.trimmingCharacters(in: .whitespacesAndNewlines)
    return query.isEmpty || zapper.config.name.localizedCaseInsensitiveContains(query)
  }

  var body: some View {
    let visible = store.zappers.filter(matches)
    if !visible.isEmpty {
      Text("Zappers").font(.system(size: 13, weight: .semibold)).foregroundStyle(.secondary)
        .zapperHomeRow(leading: 16, minHeight: 28).padding(.top, 4)
        .accessibilityAddTraits(.isHeader)
      ForEach(visible) { zapper in
        let coordinator = store.chats[zapper.coordinatorChatId]
        ZapperHomeRow(
          zapper: zapper, working: store.workingCount(zapper), busy: store.busy[zapper.id] != nil,
          preview: coordinator?.lastMessagePreview, updated: coordinator?.lastMessageAt,
          open: { open(zapper.id) }
        )
        .zapperHomeRow(leading: 16, minHeight: 60)
      }
    }
  }
}

/// A Zapper as a conversation: mascot, name and time, then the latest message.
private struct ZapperHomeRow: View {
  let zapper: Zapper
  let working: Int
  let busy: Bool
  let preview: String?
  let updated: Date?
  let open: () -> Void
  var body: some View {
    Button(action: open) {
      HStack(alignment: .center, spacing: 12) {
        ZapperAvatarView(zapper: zapper, size: 38).frame(width: 38, height: 38)
        VStack(alignment: .leading, spacing: 2) {
          HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(zapper.config.name).font(.system(size: 16, weight: .semibold)).lineLimit(1)
              .foregroundStyle(.primary)
            Spacer(minLength: 6)
            if let updated {
              Text(ZapperHomeRow.relative(updated)).font(.system(size: 13))
                .foregroundStyle(.secondary).monospacedDigit()
            }
          }
          HStack(spacing: 6) {
            Text(detail).font(.system(size: 14)).foregroundStyle(.secondary).lineLimit(1)
            Spacer(minLength: 6)
            if busy {
              ProgressView().controlSize(.mini)
            } else if zapper.needsYouCount > 0 {
              ZapperNeedsYouBadge(count: zapper.needsYouCount)
            }
          }
        }
      }
      .padding(.vertical, 6).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityElement(children: .combine)
    .accessibilityLabel(accessibilityLabel)
    .accessibilityHint("Opens this Zapper's conversation")
  }
  /// Paused or archived first, then the latest message, then what is running.
  private var detail: String {
    if zapper.status != .active { return zapper.status.label }
    let text = preview?.split(whereSeparator: \.isNewline).joined(separator: " ")
      .trimmingCharacters(in: .whitespaces)
    if let text, !text.isEmpty { return text }
    if working > 0 { return working == 1 ? "1 thread working" : "\(working) threads working" }
    return zapper.config.purpose.isEmpty ? "No messages yet" : zapper.config.purpose
  }
  /// Compact times as in a message list: 9m, 3h, 2d, 1w.
  static func relative(_ date: Date, now: Date = Date()) -> String {
    let seconds = max(0, now.timeIntervalSince(date))
    switch seconds {
    case ..<60: return "now"
    case ..<3600: return "\(Int(seconds / 60))m"
    case ..<86_400: return "\(Int(seconds / 3600))h"
    case ..<604_800: return "\(Int(seconds / 86_400))d"
    default: return "\(Int(seconds / 604_800))w"
    }
  }
  private var accessibilityLabel: String {
    var parts = [zapper.config.name, detail]
    let needs = zapper.needsYouCount
    if needs > 0 {
      parts.append(needs == 1 ? "1 thing waiting for you" : "\(needs) things waiting for you")
    }
    return parts.joined(separator: ", ")
  }
}

extension View {
  /// The home list's row metrics.
  func zapperHomeRow(leading: CGFloat, minHeight: CGFloat = 44) -> some View {
    frame(minHeight: minHeight)
      .listRowInsets(EdgeInsets(top: 0, leading: leading, bottom: 0, trailing: 12))
      .listRowSeparator(.hidden).listRowBackground(Color.clear)
  }
}
