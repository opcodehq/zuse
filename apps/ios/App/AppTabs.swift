import SwiftUI
import ZuseCore

/// The app's top-level tabs: the system tab bar on iPhone, a compact floating bar on iPad
/// (`TabletShell`) and a sidebar on Mac.
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

/// Search across Zappers, chats and issues. Picking a result opens it in its own tab.
struct SearchView: View {
  let workspace: WorkspaceModel
  @Binding var query: String
  let openZapper: (String) -> Void
  let openChat: (SessionRow) -> Void
  let openCloud: (CloudChat) -> Void
  let openIssue: (IssueEntry) -> Void
  /// Off when the field lives elsewhere, like the wide layout's sidebar.
  var searchable = true
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
    .modifier(SearchField(enabled: searchable, query: $query))
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

private struct SearchField: ViewModifier {
  let enabled: Bool
  @Binding var query: String
  func body(content: Content) -> some View {
    if enabled {
      content.searchable(text: $query, prompt: "Zappers, chats and issues")
        .autocorrectionDisabled()
    } else {
      content
    }
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

/// The iPhone tab bar as iOS draws it: it shrinks while you scroll down, and on Chats it carries
/// an "Ask anything" accessory that opens a new chat, the way Music carries its player.
enum NativeTabBar {
  /// iOS 26.1 and later host the Chats ask row in the tab bar; earlier systems keep it under
  /// the list.
  static var hostsAsk: Bool {
    #if os(iOS)
      if #available(iOS 26.1, *) { return true }
    #endif
    return false
  }
}

private struct NativeTabBarModifier: ViewModifier {
  let askEnabled: Bool
  let ask: () -> Void
  let askCloud: (() -> Void)?
  func body(content: Content) -> some View {
    #if os(iOS)
      if #available(iOS 26.1, *) {
        content
          .tabBarMinimizeBehavior(.onScrollDown)
          .tabViewBottomAccessory(isEnabled: askEnabled) {
            AskAccessory(ask: ask, askCloud: askCloud)
          }
      } else if #available(iOS 26, *) {
        content.tabBarMinimizeBehavior(.onScrollDown)
      } else {
        content
      }
    #else
      content
    #endif
  }
}

#if os(iOS)
  /// "Ask anything" in the tab bar: opens a new chat ready to write, or a cloud chat.
  @available(iOS 26.1, *)
  private struct AskAccessory: View {
    let ask: () -> Void
    let askCloud: (() -> Void)?
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement
    var body: some View {
      HStack(spacing: 0) {
        Button(action: ask) {
          HStack(spacing: 10) {
            Image(systemName: "plus").font(.system(size: 16, weight: .medium))
              .foregroundStyle(.primary)
            Text("Ask anything").font(.system(size: 16)).foregroundStyle(.secondary)
              .lineLimit(1)
            Spacer(minLength: 0)
          }
          .padding(.leading, 16)
          .frame(maxHeight: .infinity).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("New chat")
        .accessibilityIdentifier("ask-anything")
        if let askCloud, placement != .inline {
          Button(action: askCloud) {
            Image(systemName: "cloud").font(.system(size: 15, weight: .medium))
              .foregroundStyle(.secondary)
              .frame(width: 48).frame(maxHeight: .infinity).contentShape(Rectangle())
          }
          .buttonStyle(.plain)
          .accessibilityLabel("New cloud chat")
        }
      }
    }
  }
#endif

extension View {
  func nativeTabBar(
    askEnabled: Bool, ask: @escaping () -> Void, askCloud: (() -> Void)?
  ) -> some View {
    modifier(NativeTabBarModifier(askEnabled: askEnabled, ask: ask, askCloud: askCloud))
  }
}

/// Whether a new chat page opens with its composer focused.
extension EnvironmentValues {
  @Entry var focusesNewChatComposer = false
}
