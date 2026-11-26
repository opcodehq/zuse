import SwiftUI
import ZuseCore

/// Pages pushed on the pull request stack.
enum PullRequestRoute: Hashable {
  case detail(PullRequestLink)
}

/// Where a pull request stack starts: the list (from home) or one pull request (from a link).
enum PullRequestEntry: Hashable, Identifiable {
  case list
  case detail(PullRequestLink)
  var id: String {
    switch self {
    case .list: return "list"
    case .detail(let link): return link.url
    }
  }
}

/// The pull request screens' own navigation stack. Links to other pull requests inside it push
/// onto the same stack.
struct PullRequestNavigator: View {
  let store: PullRequestStore
  let entry: PullRequestEntry
  @Binding var path: [PullRequestRoute]
  /// Shown as a close button when the stack is presented as a sheet.
  var close: (() -> Void)?
  var body: some View {
    NavigationStack(path: $path) {
      root
        .toolbar {
          if let close {
            ToolbarItem(placement: .sheetClose) { SheetCloseButton(action: close) }
          }
        }
        .navigationDestination(for: PullRequestRoute.self) { route in
          switch route {
          case .detail(let link): PullRequestDetailView(store: store, link: link)
          }
        }
    }
    .environment(
      \.openURL,
      OpenURLAction { url in
        guard let link = PullRequestLink(url: url) else { return .systemAction }
        path.append(.detail(link))
        return .handled
      })
  }
  @ViewBuilder private var root: some View {
    switch entry {
    case .list:
      PullRequestListView(store: store) { link in path.append(.detail(link)) }
    case .detail(let link):
      PullRequestDetailView(store: store, link: link)
    }
  }
}

/// The home list's entry to pull requests, above Zappers.
struct PullRequestHomeRow: View {
  let store: PullRequestStore
  let open: () -> Void
  var body: some View {
    Button(action: open) {
      HStack(spacing: 10) {
        PullRequestGlyph(state: .open, size: 16).frame(width: 20, height: 20)
          .accessibilityHidden(true)
        Text("Pull requests").font(.system(size: 16)).foregroundStyle(.primary.opacity(0.92))
        Spacer(minLength: 6)
        if let count = store.openCount {
          Text(count.formatted()).font(.system(size: 13)).monospacedDigit()
            .foregroundStyle(.secondary)
        } else if store.resolvingRepositories {
          ProgressView().controlSize(.mini)
        }
        Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
          .foregroundStyle(.tertiary).frame(width: 16)
      }
      .frame(minHeight: 44).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(
      "Pull requests" + (store.openCount.map { ", \($0) open" } ?? "")
    )
    .accessibilityIdentifier("home-pull-requests")
  }
}
