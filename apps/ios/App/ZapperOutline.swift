import SwiftUI
import ZuseCore

/// A Zapper in the iPad's full-width Zappers list: face, name with a fold chevron when it has
/// threads, its latest line and the time at the far edge. Its threads list under it while it
/// is unfolded.
struct ZapperOutlineRow: View {
  static let tile: CGFloat = 40
  static let spacing: CGFloat = 12
  /// Where a row's text starts, past the face.
  static let textInset = tile + spacing
  let zapper: Zapper
  let working: Int
  let busy: Bool
  let preview: String?
  let updated: Date?
  let threadCount: Int
  let expanded: Bool
  let toggle: () -> Void

  var body: some View {
    let detail = ZapperListRow.detail(zapper: zapper, preview: preview, working: working)
    HStack(alignment: .top, spacing: Self.spacing) {
      ZapperTile(zapper: zapper, size: Self.tile)
      VStack(alignment: .leading, spacing: 4) {
        HStack(alignment: .firstTextBaseline, spacing: 2) {
          Text(zapper.config.name).font(.system(size: 17)).lineLimit(1)
            .foregroundStyle(.primary)
          if threadCount > 0 {
            Button(action: toggle) {
              Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.tertiary)
                .rotationEffect(.degrees(expanded ? 90 : 0))
                .frame(width: 28, height: 22).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHidden(true)
          }
          Spacer(minLength: 8)
          if let updated {
            Text(CompactTime.label(updated)).font(.system(size: 13))
              .foregroundStyle(.secondary).monospacedDigit()
          }
        }
        HStack(alignment: .firstTextBaseline, spacing: 8) {
          Text(detail).font(.system(size: 15)).lineLimit(1)
            // What the Zapper is asking of you reads in the accent colour, like an unread line.
            .foregroundStyle(asks ? AnyShapeStyle(AppTheme.accent) : AnyShapeStyle(.secondary))
            .frame(maxWidth: TabletMeasure.line, alignment: .leading)
          Spacer(minLength: 8)
          if busy {
            ProgressView().controlSize(.mini)
          } else if zapper.needsYouCount > 0 {
            ZapperNeedsYouBadge(count: zapper.needsYouCount)
          }
        }
      }
    }
    .padding(.vertical, 9).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
    .accessibilityLabel(accessibilityLabel(detail))
    .accessibilityHint("Opens this Zapper’s conversation")
    .accessibilityAction(named: expanded ? "Hide threads" : "Show threads") {
      if threadCount > 0 { toggle() }
    }
  }

  private var asks: Bool { zapper.status == .active && zapper.needsYouCount > 0 }

  private func accessibilityLabel(_ detail: String) -> String {
    var parts = [zapper.config.name, detail]
    let needs = zapper.needsYouCount
    if needs > 0 {
      parts.append(needs == 1 ? "1 thing waiting for you" : "\(needs) things waiting for you")
    }
    if threadCount > 0 {
      parts.append(threadCount == 1 ? "1 thread" : "\(threadCount) threads")
    }
    return parts.joined(separator: ", ")
  }
}

/// One of a Zapper's threads, indented under it: where it runs (or how it is doing), its
/// title, then the issue it works on, that issue's pull request and its age.
struct ZapperThreadOutlineRow: View {
  let zapper: Zapper
  let worker: ZapperWorker
  let title: String
  let rank: ZapperThreadRank
  let updated: Date?
  /// Pull requests seen in a listing, for their state.
  let known: [String: PullRequestListItem]

  var body: some View {
    let issue = zapper.issue(forThread: worker.chatId)
    HStack(alignment: .top, spacing: ZapperOutlineRow.spacing) {
      leading.frame(width: ZapperOutlineRow.tile - 4, height: 22, alignment: .trailing)
        .padding(.trailing, 4)
      VStack(alignment: .leading, spacing: 4) {
        Text(title).font(.system(size: 17)).foregroundStyle(.primary.opacity(0.9)).lineLimit(1)
          .frame(maxWidth: TabletMeasure.line, alignment: .leading)
        HStack(spacing: 6) {
          ZapperTile(zapper: zapper, size: 16)
          if let issue {
            Text(issue.code).font(.system(size: 13, design: .monospaced))
              .foregroundStyle(.secondary)
            if let pull = issue.pullRequests.last, let link = PullRequestLink(pull.url) {
              chip(link, issue: issue)
            }
          } else {
            Text(status).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
          }
          if let updated {
            Text("·").font(.system(size: 13)).foregroundStyle(.tertiary)
            Text(CompactTime.label(updated)).font(.system(size: 13)).monospacedDigit()
              .foregroundStyle(.secondary)
          }
        }
      }
      Spacer(minLength: 0)
    }
    .padding(.vertical, 8).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
    .accessibilityLabel(accessibilityLabel(issue))
    .accessibilityHint(
      worker.queued ? "Opens \(zapper.config.name)’s overview" : "Opens this thread")
  }

  /// Working, waiting and queued threads show how they are doing; the rest where they run.
  @ViewBuilder private var leading: some View {
    switch rank {
    case .idle:
      if worker.deviceId.hasPrefix("cloud") {
        DitherCloudIcon(size: 15).foregroundStyle(.secondary)
      } else {
        Image(systemName: "desktopcomputer").font(.system(size: 13))
          .foregroundStyle(.secondary)
      }
    default: ZapperThreadMark(rank: rank)
    }
  }

  private func chip(_ link: PullRequestLink, issue: ZapperIssue) -> some View {
    let tint = pullTint(link, issue: issue)
    return HStack(spacing: 2) {
      AppIcon(name: AppIcon.pullRequest, size: 10, weight: .semibold)
      Text("#\(link.number)").font(.system(size: 13, weight: .medium)).monospacedDigit()
    }
    .foregroundStyle(tint)
  }

  /// The listed state when the pull request has been seen; otherwise the issue's state hints it.
  private func pullTint(_ link: PullRequestLink, issue: ZapperIssue) -> Color {
    if let item = known[link.url] { return PullRequestTint.state(item.state, draft: item.isDraft) }
    switch issue.details.status {
    case .done: return PullRequestTint.merged
    case .canceled: return PullRequestTint.closed
    default: return PullRequestTint.open
    }
  }

  private var status: String {
    if let position = worker.queuePosition {
      return position == 1 ? "Queued · starts next" : "Queued · #\(position) in line"
    }
    return rank.label
  }

  private func accessibilityLabel(_ issue: ZapperIssue?) -> String {
    var parts = [title, rank.label]
    if let issue {
      parts.append(issue.code)
      if let pull = issue.pullRequests.last, let link = PullRequestLink(pull.url) {
        parts.append("pull request \(link.number)")
      }
    }
    return parts.joined(separator: ", ")
  }
}

extension View {
  /// Row metrics for the iPad's full-width Zappers list.
  func outlineRow() -> some View {
    listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 20))
      .listRowSeparator(.hidden).listRowBackground(Color.clear)
  }
}
