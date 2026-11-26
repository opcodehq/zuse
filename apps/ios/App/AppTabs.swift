import SwiftUI
import ZuseCore

/// The app's top-level tabs: a tab bar on iPhone, a sidebar on iPad and Mac.
enum AppTab: String, Hashable, CaseIterable {
  case zappers, chats, prs, more, search
  /// `--tab=<zappers|chats|prs|more>` picks the first tab for screenshots.
  static var launchArgument: AppTab? {
    #if DEBUG
      for argument in ProcessInfo.processInfo.arguments where argument.hasPrefix("--tab=") {
        return AppTab(rawValue: String(argument.dropFirst("--tab=".count)))
      }
    #endif
    return nil
  }
}

/// The Pull requests tab. Its list arrives with the pull request screens; swap the body for
/// that list when it lands.
struct PullRequestsTab: View {
  let workspace: WorkspaceModel
  @Binding var path: [PullRequestRoute]
  var body: some View {
    if workspace.pullRequests.available || workspace.pullRequests.preview {
      PullRequestNavigator(store: workspace.pullRequests, entry: .list, path: $path)
    } else {
      NavigationStack { PullRequestsPlaceholder() }
    }
  }
}

private struct PullRequestsPlaceholder: View {
  var body: some View {
    ZappersEmptyState(
      symbol: AppIcon.pullRequest, title: "Pull requests",
      detail: "Connect a computer to see pull requests for its repositories."
    )
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .background(AppTheme.surface)
    .navigationTitle("Pull requests").compactNavigationTitle()
  }
}

/// The More tab's icon: your initials once signed in, otherwise a circle of dots.
enum AccountTabIcon {
  @MainActor static func image(_ account: Account) -> Image {
    if account.signedIn, let image = render(account) {
      return Image(nativeImage: image).renderingMode(.original)
    }
    return Image(systemName: "ellipsis.circle")
  }
  @MainActor private static func render(_ account: Account) -> NativeImage? {
    let renderer = ImageRenderer(content: AccountAvatar(account: account, size: 26))
    renderer.scale = 3
    #if os(macOS)
      return renderer.nsImage
    #else
      return renderer.uiImage
    #endif
  }
}

/// Search across Zappers, chats and issues. Picking a result opens it in its own tab.
struct SearchView: View {
  let workspace: WorkspaceModel
  @Binding var query: String
  let openZapper: (String) -> Void
  let openChat: (SessionRow) -> Void
  let openCloud: (CloudChat) -> Void
  let openIssue: (IssueEntry) -> Void
  private var needle: String { query.trimmingCharacters(in: .whitespacesAndNewlines) }

  var body: some View {
    let store = workspace.zappers
    let zappers = store.zappers.filter { matches($0.config.name) || matches($0.config.purpose) }
    let names = Dictionary(
      workspace.projects.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
    let sessions =
      workspace.isCloudActive
      ? []
      : workspace.sessions.filter {
        !store.keepsChat($0.id) && (matches($0.title) || matches(names[$0.projectId] ?? ""))
      }
    let clouds = workspace.cloudChats.filter {
      !$0.isArchived && (matches($0.title) || matches($0.repositoryDisplayName))
    }
    let issues = IssueBoard.entries(store.zappers).filter {
      matches($0.issue.details.title) || matches($0.issue.code)
    }
    List {
      if needle.isEmpty {
        ZappersEmptyState(
          symbol: "magnifyingglass", title: "Search Zuse",
          detail: "Find Zappers, chats and issues by name."
        )
        .chatsRow(minHeight: 0).padding(.top, 72)
      } else if zappers.isEmpty && sessions.isEmpty && clouds.isEmpty && issues.isEmpty {
        ContentUnavailableView.search(text: needle).chatsRow(minHeight: 0).padding(.top, 40)
      }
      if !needle.isEmpty && !zappers.isEmpty {
        header("Zappers")
        ForEach(zappers) { zapper in
          Button {
            openZapper(zapper.id)
          } label: {
            ZapperListRow(
              zapper: zapper, working: store.workingCount(zapper), busy: false,
              preview: store.chats[zapper.coordinatorChatId]?.lastMessagePreview,
              updated: store.chats[zapper.coordinatorChatId]?.lastMessageAt)
          }
          .buttonStyle(.plain).chatsRow()
        }
      }
      if !needle.isEmpty && (!sessions.isEmpty || !clouds.isEmpty) {
        header("Chats")
        ForEach(sessions) { session in
          Button {
            openChat(session)
          } label: {
            ChatRowLabel(
              title: session.title.isEmpty ? "New chat" : session.title,
              meta: names[session.projectId] ?? "No project"
            ) {
              ProviderMark(providerID: session.providerId, size: 16).foregroundStyle(.secondary)
            }
          }
          .buttonStyle(.plain).chatsRow()
        }
        ForEach(clouds) { chat in
          Button {
            openCloud(chat)
          } label: {
            ChatRowLabel(title: chat.title, meta: chat.repositoryDisplayName) { DitherCloudIcon() }
          }
          .buttonStyle(.plain).chatsRow()
        }
      }
      if !needle.isEmpty && !issues.isEmpty {
        header("Issues")
        ForEach(issues.prefix(30)) { entry in
          Button {
            openIssue(entry)
          } label: {
            IssueListRow(
              issue: entry.issue, zapper: store.zapper(entry.zapperID), account: workspace.account)
          }
          .buttonStyle(.plain).chatsRow()
        }
      }
    }
    .listStyle(.plain)
    .scrollContentBackground(.hidden).background(AppTheme.surface)
    .navigationTitle("Search").compactNavigationTitle()
    .searchable(text: $query, prompt: "Zappers, chats and issues")
    .autocorrectionDisabled()
  }

  private func matches(_ text: String) -> Bool {
    !needle.isEmpty && text.localizedCaseInsensitiveContains(needle)
  }

  private func header(_ title: String) -> some View {
    Text(title).font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
      .chatsRow(minHeight: 34).padding(.top, 8)
      .accessibilityAddTraits(.isHeader)
  }
}

/// Pages with their own composer hide the iPhone tab bar, like a conversation in Messages.
private struct CompactTabBarHidden: ViewModifier {
  #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
  #endif
  func body(content: Content) -> some View {
    #if os(iOS)
      content.toolbar(sizeClass == .compact ? .hidden : .automatic, for: .tabBar)
    #else
      content
    #endif
  }
}

extension View {
  func hidesTabBarWhenCompact() -> some View { modifier(CompactTabBarHidden()) }
}

extension View {
  /// Hides the tab bar on pages with their own bottom controls (a composer, Merge).
  @ViewBuilder func hidesTabBar() -> some View {
    #if os(iOS)
      toolbar(.hidden, for: .tabBar)
    #else
      self
    #endif
  }
}
