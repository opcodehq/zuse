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
        ZapperHomeRow(
          zapper: zapper, working: store.workingCount(zapper), busy: store.busy[zapper.id] != nil,
          open: { open(zapper.id) }
        )
        .zapperHomeRow(leading: 16, minHeight: 44)
      }
    }
  }
}

private struct ZapperHomeRow: View {
  let zapper: Zapper
  let working: Int
  let busy: Bool
  let open: () -> Void
  var body: some View {
    Button(action: open) {
      HStack(spacing: 10) {
        ZapperAvatarView(zapper: zapper, size: 24).frame(width: 24, height: 24)
        Text(zapper.config.name).font(.system(size: 16)).lineLimit(1)
          .foregroundStyle(.primary.opacity(0.92))
        Spacer(minLength: 6)
        if busy {
          ProgressView().controlSize(.mini)
        } else if zapper.needsYouCount > 0 {
          ZapperNeedsYouBadge(count: zapper.needsYouCount)
        } else if zapper.status != .active {
          Text(zapper.status.label).font(.system(size: 12)).foregroundStyle(.secondary)
        } else if working > 0 {
          Text(working == 1 ? "1 working" : "\(working) working").font(.system(size: 12))
            .foregroundStyle(.secondary).monospacedDigit()
        }
      }
      .frame(minHeight: 44).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(accessibilityLabel)
    .accessibilityHint("Opens this Zapper's conversation")
  }
  private var accessibilityLabel: String {
    var parts = [zapper.config.name]
    if zapper.status != .active { parts.append(zapper.status.label) }
    if working > 0 {
      parts.append(working == 1 ? "1 thread working" : "\(working) threads working")
    }
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
