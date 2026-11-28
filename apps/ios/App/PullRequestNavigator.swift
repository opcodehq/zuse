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
          case .detail(let link): PullRequestDetailView(store: store, link: link).hidesTabBar()
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
      PullRequestListView(store: store) { link in path.append(.detail(link)) }.clearsTabletBar()
    case .detail(let link):
      PullRequestDetailView(store: store, link: link)
    }
  }
}
