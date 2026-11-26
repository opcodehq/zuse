import SwiftUI
import ZuseCore

/// The home list's Zappers: each Zapper opens its page, with the threads still in flight
/// listed under it instead of under their projects.
struct ZapperHomeSection: View {
  let workspace: WorkspaceModel
  let search: String
  let filter: ArchiveFilter
  @Binding var collapsed: Set<String>
  let open: (String) -> Void

  private var store: ZapperStore { workspace.zappers }

  /// Threads the home list shows: running, waiting, queued or not started yet. Threads that
  /// finished a turn move to the Zapper's Threads page, as on desktop.
  private func threads(_ zapper: Zapper) -> [ZapperWorker] {
    store.orderedThreads(zapper).filter { worker in
      guard store.chats[worker.chatId]?.archived != true else { return false }
      return store.rank(of: worker) != .idle || store.chats[worker.chatId]?.lastMessageAt == nil
    }
  }
  private func matches(_ zapper: Zapper, threads: [ZapperWorker]) -> Bool {
    switch filter {
    case .active: if zapper.status == .archived { return false }
    case .archived: if zapper.status != .archived { return false }
    case .all: break
    }
    let query = search.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !query.isEmpty else { return true }
    return zapper.config.name.localizedCaseInsensitiveContains(query)
      || threads.contains {
        store.title(of: $0.chatId, fallback: $0.objective).localizedCaseInsensitiveContains(query)
      }
  }

  var body: some View {
    let visible = store.zappers.map { ($0, threads($0)) }.filter { matches($0.0, threads: $0.1) }
    if !visible.isEmpty {
      Text("Zappers").font(.system(size: 13, weight: .semibold)).foregroundStyle(.secondary)
        .zapperHomeRow(leading: 16, minHeight: 28).padding(.top, 4)
        .accessibilityAddTraits(.isHeader)
      ForEach(visible, id: \.0.id) { zapper, threads in
        let key = "zapper|" + zapper.id
        let isCollapsed = collapsed.contains(key)
        ZapperHomeHeader(
          zapper: zapper, working: store.workingCount(zapper), threads: threads.count,
          collapsed: isCollapsed, busy: store.busy[zapper.id] != nil,
          open: { open(zapper.id) },
          toggle: {
            withAnimation(.snappy(duration: 0.2)) {
              if isCollapsed { collapsed.remove(key) } else { collapsed.insert(key) }
            }
          }
        )
        .zapperHomeRow(leading: 16, minHeight: 48)
        if !isCollapsed {
          ForEach(threads) { worker in
            ZapperHomeThreadRow(workspace: workspace, worker: worker)
          }
        }
      }
    }
  }
}

private struct ZapperHomeHeader: View {
  let zapper: Zapper
  let working: Int
  let threads: Int
  let collapsed: Bool
  let busy: Bool
  let open: () -> Void
  let toggle: () -> Void
  var body: some View {
    HStack(spacing: 10) {
      Button(action: open) {
        HStack(spacing: 10) {
          ZapperAvatarView(zapper: zapper, size: 28)
          Text(zapper.config.name).font(.system(size: 16, weight: .bold)).lineLimit(1)
            .foregroundStyle(.primary)
          if zapper.status != .active {
            Text(zapper.status.label).font(.system(size: 12, weight: .medium))
              .foregroundStyle(.secondary)
              .padding(.horizontal, 6).padding(.vertical, 1)
              .background(Color.primary.opacity(0.07), in: Capsule())
          }
          Spacer(minLength: 6)
          if busy {
            ProgressView().controlSize(.mini)
          } else if zapper.needsYouCount > 0 {
            ZapperNeedsYouBadge(count: zapper.needsYouCount)
          }
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel(accessibilityLabel)
      .accessibilityHint("Opens this Zapper")
      if threads > 0 {
        Button(action: toggle) {
          HStack(spacing: 6) {
            Text("\(threads)").font(.system(size: 13)).monospacedDigit()
              .foregroundStyle(.secondary)
            Image(systemName: collapsed ? "chevron.right" : "chevron.down")
              .font(.system(size: 12, weight: .semibold)).foregroundStyle(.tertiary)
              .frame(width: 16)
          }
          .frame(minWidth: 36, minHeight: 44).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(collapsed ? "Show threads" : "Hide threads")
      }
    }
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

private struct ZapperHomeThreadRow: View {
  let workspace: WorkspaceModel
  let worker: ZapperWorker
  var body: some View {
    let store = workspace.zappers
    let title = store.title(of: worker.chatId, fallback: worker.objective)
    let rank = store.rank(of: worker)
    Group {
      if worker.queued {
        label(title: title, rank: rank, detail: "Queued")
          .accessibilityHint("Starts when a running thread finishes")
      } else {
        // A hidden link keeps split-view selection while omitting the disclosure chevron.
        ZStack {
          NavigationLink(
            value: store.session(
              for: worker.chatId, fallbackTitle: worker.objective,
              known: workspace.sessions + workspace.archivedSessions)
          ) { EmptyView() }.opacity(0)
          label(title: title, rank: rank, detail: rank == .needsYou ? "Needs you" : nil)
        }
      }
    }
    .zapperHomeRow(leading: 24)
  }
  private func label(title: String, rank: ZapperThreadRank, detail: String?) -> some View {
    HStack(spacing: 10) {
      ZapperThreadMark(rank: rank).frame(width: 20, height: 20)
      Text(title).font(.system(size: 16)).foregroundStyle(.primary.opacity(0.92)).lineLimit(1)
      Spacer(minLength: 6)
      if let detail {
        Text(detail).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1)
      }
    }
    .frame(minHeight: 44).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
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
