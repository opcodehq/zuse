import SwiftUI
import ZuseCore

/// A place in the regular-width sidebar: one Zapper, or one of the app's lists.
enum ShellSection: Hashable {
  case zapper(String)
  case chats, pullRequests, issues, computers, settings, search
}

/// What the regular-width shell asks the rest of the app to do.
struct ShellActions {
  let openZapper: (String) -> Void
  let closeZapper: () -> Void
  let openChat: (SessionRow) -> Void
  let openCloud: (CloudChat) -> Void
  let selectSession: (SessionRow) -> Void
  /// Opens a new chat, with any text already typed.
  let newChat: (String) -> Void
  let newCloudChat: () -> Void
  let connect: () -> Void
  let addProject: () -> Void
}

/// The Mac layout (and the widest iPhones in landscape): a sidebar of Zappers and lists, each
/// list beside its selection.
/// A Zapper opens on its conversation with its overview in a trailing inspector. Compact
/// widths use the tab layout in `WorkspaceView`, and iPad uses `TabletShell`.
struct RegularShell<Chat: View, ChatsDetail: View>: View {
  @Bindable var workspace: WorkspaceModel
  @Binding var section: ShellSection?
  @Binding var chatSelection: SessionRow?
  let zapperID: String?
  @Binding var zapperPath: [ZapperRoute]
  @Binding var inspector: Bool
  @Binding var pullRequestPath: [PullRequestRoute]
  /// The open issue first, then pages pushed from it.
  @Binding var issuePath: [ZapperRoute]
  @Binding var settingsPage: MoreRoute?
  @Binding var searchText: String
  let actions: ShellActions
  @ViewBuilder let chat: (SessionRow) -> Chat
  /// The Chats detail: a new chat, the selected chat, or the new chat composer.
  @ViewBuilder let chatsDetail: () -> ChatsDetail
  @State private var visibility: NavigationSplitViewVisibility = .automatic
  @State private var width: CGFloat = 0
  @State private var creatingZapper = false
  @State private var beforeSearch: ShellSection?
  private var store: ZapperStore { workspace.zappers }
  /// Wide enough for every column at once: landscape iPads and most Mac windows.
  private var wide: Bool { width >= 1100 }
  /// The sidebar's search field stays in view rather than hiding until the list is pulled.
  private static var searchPlacement: SearchFieldPlacement {
    #if os(iOS)
      .navigationBarDrawer(displayMode: .always)
    #else
      .sidebar
    #endif
  }
  private var showsZapper: Bool {
    if case .zapper = section { return zapperID != nil }
    return false
  }

  var body: some View {
    Group {
      if showsZapper { zapperSplit } else { listSplit }
    }
    .onGeometryChange(for: CGFloat.self) {
      $0.size.width
    } action: { value in
      let wasWide = wide
      width = value
      if wasWide != wide || visibility == .automatic { fitColumns() }
      // The overview opens beside the conversation only when there's room for both.
      if showsZapper && wasWide != wide { inspector = wide }
    }
    .onChange(of: showsZapper) { fitColumns() }
    .onChange(of: section) { old, new in
      // A narrow sidebar is an overlay: picking a place puts it away.
      if !wide { fitColumns() }
      if case .zapper(let id) = new, old != new { inspector = wide && store.zapper(id) != nil }
    }
    .onChange(of: searchText) { _, text in
      let searching = !text.trimmingCharacters(in: .whitespaces).isEmpty
      if searching && section != .search {
        beforeSearch = section
        section = .search
      } else if !searching && section == .search {
        section = beforeSearch ?? .chats
      }
    }
    .sheet(isPresented: $creatingZapper) {
      NewZapperSheet(workspace: workspace) { id in
        creatingZapper = false
        actions.openZapper(id)
      }
    }
    #if DEBUG
      .onAppear {
        if ProcessInfo.processInfo.arguments.contains("--new-zapper-preview") {
          creatingZapper = true
        }
      }
      // `--show-sidebar` opens the sidebar over a narrow layout, for screenshots of it.
      .task {
        guard ProcessInfo.processInfo.arguments.contains("--show-sidebar") else { return }
        try? await Task.sleep(for: .milliseconds(1500))
        withAnimation { visibility = .all }
      }
    #endif
  }

  /// The split view reopens its inspector on its own when it is rebuilt; only the toolbar
  /// button and the app open it, while a swipe can still close it.
  private var inspectorShown: Binding<Bool> {
    Binding(get: { inspector }, set: { shown in if !shown { inspector = false } })
  }

  private func fitColumns() {
    if showsZapper {
      visibility = wide ? .all : .detailOnly
    } else {
      visibility = wide ? .all : .doubleColumn
    }
  }

  // MARK: Splits

  /// A Zapper: sidebar, then its conversation with the overview as an inspector.
  private var zapperSplit: some View {
    NavigationSplitView(columnVisibility: $visibility) {
      sidebar
    } detail: {
      if let zapperID {
        ZapperNavigator(
          workspace: workspace, zapperID: zapperID, path: $zapperPath,
          select: actions.selectSession, close: actions.closeZapper,
          // Pushed pages carry the overview too, so it stays beside pages opened from it.
          page: { page in AnyView(overviewInspector(page, zapperID: zapperID)) }
        ) { session in
          chat(session).toolbar {
            ToolbarItem(placement: .primaryAction) { inspectorToggle }
          }
        }
        .inspector(isPresented: inspectorShown) { overview(zapperID) }
        .id(zapperID)
      }
    }
    .modifier(ZapperSplitStyle(wide: wide))
    .task {
      // A freshly built split view shows its sidebar; a narrow one gives the room back to the
      // conversation once it is on screen.
      guard !wide else { return }
      visibility = .automatic
      try? await Task.sleep(for: .milliseconds(30))
      fitColumns()
    }
  }

  private func overviewInspector(_ page: AnyView, zapperID: String) -> some View {
    page
      .toolbar {
        ToolbarItem(placement: .primaryAction) { inspectorToggle }
      }
      .inspector(isPresented: inspectorShown) { overview(zapperID) }
  }

  private func overview(_ zapperID: String) -> some View {
    ZapperDetailView(
      workspace: workspace, zapperID: zapperID, push: pushFromOverview,
      close: actions.closeZapper, sidePanel: true
    )
    .inspectorColumnWidth(min: 320, ideal: 380, max: 480)
  }

  /// Every other place: sidebar, its list, and the list's selection.
  private var listSplit: some View {
    NavigationSplitView(columnVisibility: $visibility) {
      sidebar
    } content: {
      content
        .navigationSplitViewColumnWidth(min: 300, ideal: 360, max: 440)
    } detail: {
      detail
    }
    .navigationSplitViewStyle(.balanced)
  }

  private var inspectorToggle: some View {
    Button {
      withAnimation(.snappy) { inspector.toggle() }
    } label: {
      Label(inspector ? "Hide overview" : "Show overview", systemImage: "sidebar.trailing")
    }
    .tint(.primary)
    .accessibilityIdentifier("zapper-inspector")
  }

  /// Pages opened from the overview push in the conversation's column.
  private func pushFromOverview(_ route: ZapperRoute) {
    switch route {
    case .overview: return
    case .chat(let session) where session.id == store.zapper(zapperID ?? "")?.coordinatorChatId:
      zapperPath = []
    default: zapperPath.append(route)
    }
  }

  // MARK: Sidebar

  private var activeZappers: [Zapper] { store.zappers.filter { $0.status != .archived } }
  private var issuesNeedingYou: Int {
    activeZappers.reduce(0) { $0 + $1.issues.filter { $0.details.status == .needsYou }.count }
  }

  private var sidebar: some View {
    List(selection: $section) {
      Section {
        computerStatus
      }
      Section {
        ForEach(activeZappers) { zapper in
          ShellZapperRow(zapper: zapper, working: store.workingCount(zapper))
            .tag(ShellSection.zapper(zapper.id))
            .accessibilityIdentifier("sidebar-zapper-\(zapper.id)")
        }
        if store.zappers.isEmpty && canCreateZapper && (store.loaded || store.preview) {
          ZapperSidebarInvite { creatingZapper = true }
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets(top: 4, leading: 12, bottom: 4, trailing: 12))
        } else {
          Button {
            creatingZapper = true
          } label: {
            ShellRow(title: "New Zapper", symbol: "plus").foregroundStyle(.secondary)
          }
          .buttonStyle(.plain)
          .disabled(!canCreateZapper)
          .accessibilityIdentifier("sidebar-new-zapper")
        }
      } header: {
        ShellSectionHeader(title: "Zappers")
      }
      Section {
        ShellRow(title: "Chats", symbol: "bubble.left.and.bubble.right").tag(ShellSection.chats)
        ShellRow(title: "Pull requests", symbol: AppIcon.pullRequest) {
          if let count = workspace.pullRequests.openCount, count > 0 {
            Text("\(count)").font(.subheadline).monospacedDigit().foregroundStyle(.secondary)
          }
        }
        .tag(ShellSection.pullRequests)
        ShellRow(title: "Issues", symbol: "circle.dashed") {
          if issuesNeedingYou > 0 { ZapperNeedsYouBadge(count: issuesNeedingYou) }
        }
        .tag(ShellSection.issues)
      } header: {
        ShellSectionHeader(title: "Work")
      }
      Section {
        ShellRow(title: "Computers", symbol: "desktopcomputer") {
          if let state = workspace.computersIndicator { ComputerStatusDot(state: state, size: 7) }
        }
        .tag(ShellSection.computers)
        .accessibilityIdentifier("sidebar-computers")
        ShellRow(title: "Settings", symbol: "gearshape").tag(ShellSection.settings)
          .accessibilityIdentifier("sidebar-settings")
        accountRow
      }
    }
    .listStyle(.sidebar)
    .environment(\.defaultMinListRowHeight, 44)
    .navigationTitle("Zuse")
    .navigationSplitViewColumnWidth(min: 240, ideal: 280, max: 340)
    .searchable(text: $searchText, placement: Self.searchPlacement, prompt: "Search")
    .autocorrectionDisabled()
    .disabled(workspace.clearingAccount)
    .refreshable { await workspace.refreshSessionCatalog() }
  }

  private var canCreateZapper: Bool {
    store.preview || (store.available && workspace.connectionState == "Connected")
  }

  /// You: your picture and name, opening your account in Settings.
  private var accountRow: some View {
    let account = workspace.account
    return Button {
      section = .settings
      settingsPage = .account
    } label: {
      HStack(spacing: 10) {
        AccountAvatar(account: account, size: 26)
        Text(account.signedIn ? account.displayName ?? account.email ?? "Zuse account" : "Sign in")
          .font(.body).foregroundStyle(.primary).lineLimit(1)
        Spacer(minLength: 0)
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(account.signedIn ? "Account" : "Sign in")
    .accessibilityIdentifier("sidebar-account")
  }

  /// Which computer the app is driving, and how the connection is.
  private var computerStatus: some View {
    Button {
      section = .computers
    } label: {
      HStack(spacing: 8) {
        if let state = workspace.computersIndicator {
          ComputerStatusDot(state: state, size: 8)
        } else {
          Circle().fill(Color.secondary.opacity(0.5)).frame(width: 8, height: 8)
        }
        Text(workspace.connectedComputer ?? workspace.connectionState)
          .font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
        Spacer(minLength: 0)
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .listRowBackground(Color.clear)
    .accessibilityLabel("Computer: \(workspace.connectedComputer ?? workspace.connectionState)")
  }

  // MARK: Content

  @ViewBuilder private var content: some View {
    switch section ?? .chats {
    case .zapper, .chats:
      ChatsListView(
        workspace: workspace, selection: $chatSelection, connect: actions.connect,
        newChat: actions.newChat, newCloudChat: actions.newCloudChat,
        addProject: actions.addProject, openComputers: { section = .computers },
        showsAsk: false, highlightsSelection: true
      )
      .disabled(workspace.clearingAccount)
      .toolbar {
        ToolbarItem(placement: .primaryAction) {
          Button("New chat", systemImage: "square.and.pencil") { actions.newChat("") }
            .tint(.primary)
            .accessibilityIdentifier("shell-new-chat")
        }
      }
    case .pullRequests:
      if workspace.pullRequests.available || workspace.pullRequests.preview {
        PullRequestListView(
          store: workspace.pullRequests, selectedURL: openPullRequest?.url
        ) { link in
          pullRequestPath = [.detail(link)]
        }
      } else {
        emptyColumn(
          symbol: AppIcon.pullRequest, title: "Pull requests",
          detail: "Connect a computer to see pull requests for its repositories."
        )
        .navigationTitle("Pull requests")
      }
    case .issues:
      IssuesListView(workspace: workspace, selectedIssue: openIssueID) { route in
        issuePath = [route]
      }
    case .computers:
      ComputersView(workspace: workspace)
    case .settings:
      MoreView(workspace: workspace, selection: $settingsPage)
    case .search:
      SearchView(
        workspace: workspace, query: $searchText, openZapper: actions.openZapper,
        openChat: actions.openChat, openCloud: actions.openCloud,
        openIssue: { entry in
          section = .issues
          issuePath = [.issue(zapper: entry.zapperID, issue: entry.issue.id)]
        },
        searchable: false)
    }
  }

  // MARK: Detail

  private var openPullRequest: PullRequestLink? {
    if case .detail(let link) = pullRequestPath.last { return link }
    return nil
  }
  private var openIssueID: String? {
    if case .issue(_, let issue) = issuePath.first { return issue }
    return nil
  }

  @ViewBuilder private var detail: some View {
    switch section ?? .chats {
    case .zapper, .chats:
      chatsDetail()
    case .pullRequests:
      if let link = openPullRequest {
        NavigationStack {
          PullRequestDetailView(store: workspace.pullRequests, link: link).id(link.url)
        }
        .environment(
          \.openURL,
          OpenURLAction { url in
            guard let link = PullRequestLink(url: url) else { return .systemAction }
            pullRequestPath = [.detail(link)]
            return .handled
          })
      } else {
        emptyColumn(
          symbol: AppIcon.pullRequest, title: "No pull request selected",
          detail: "Pick one to read its conversation, review files and merge.")
      }
    case .issues:
      if let first = issuePath.first {
        NavigationStack(path: issueStack) {
          ZapperDestination(
            workspace: workspace, route: first, push: pushFromIssue, close: { issuePath = [] },
            chat: chat
          )
          .navigationDestination(for: ZapperRoute.self) { route in
            ZapperDestination(
              workspace: workspace, route: route, push: pushFromIssue,
              close: { if issuePath.count > 1 { issuePath.removeLast() } }, chat: chat)
          }
        }
        .id(openIssueID)
      } else {
        emptyColumn(
          symbol: "circle.dashed", title: "No issue selected",
          detail: "Issues your Zappers plan show up in the list. Pick one to read and answer it.")
      }
    case .computers:
      emptyColumn(
        symbol: "desktopcomputer", title: "Your computers",
        detail: "Pick a computer to see its connection, or add another one.")
    case .settings:
      NavigationStack { settingsDetail(settingsPage ?? .account) }
    case .search:
      emptyColumn(
        symbol: "magnifyingglass", title: "Search Zuse",
        detail: "Results for Zappers, chats and issues are listed beside this.")
    }
  }

  /// Pages pushed above the open issue.
  private var issueStack: Binding<[ZapperRoute]> {
    Binding(
      get: { Array(issuePath.dropFirst()) },
      set: { pushed in issuePath = Array(issuePath.prefix(1)) + pushed })
  }

  private func pushFromIssue(_ route: ZapperRoute) {
    if case .overview(let id) = route {
      actions.openZapper(id)
      inspector = true
    } else {
      issuePath.append(route)
    }
  }

  @ViewBuilder private func settingsDetail(_ page: MoreRoute) -> some View {
    Group {
      switch page {
      case .account: AccountSettingsView(workspace: workspace)
      case .appearance: AppearanceSettingsView()
      case .archived: ArchivedChatsView(workspace: workspace, open: actions.openChat)
      case .help: HelpSettingsView { section = .computers }
      case .issues: IssuesListView(workspace: workspace) { issuePath = [$0] }
      case .computers: ComputersView(workspace: workspace)
      }
    }
    .readableColumn()
  }

  private func emptyColumn(symbol: String, title: String, detail: String) -> some View {
    ZappersEmptyState(symbol: symbol, title: title, detail: detail)
      .padding(.bottom, 40)
      .frame(maxWidth: .infinity, maxHeight: .infinity)
      .background(AppTheme.surface)
  }
}

/// A Zapper's conversation keeps its full width when narrow; the sidebar slides over it.
private struct ZapperSplitStyle: ViewModifier {
  let wide: Bool
  func body(content: Content) -> some View {
    if wide {
      content.navigationSplitViewStyle(.balanced)
    } else {
      content.navigationSplitViewStyle(.prominentDetail)
    }
  }
}

// MARK: - Sidebar rows

/// A sidebar row: a glyph in a fixed slot, a title and an optional trailing count.
private struct ShellRow<Trailing: View>: View {
  let title: String
  let symbol: String
  @ViewBuilder var trailing: Trailing
  var body: some View {
    HStack(spacing: 12) {
      AppIcon(name: symbol, size: 16).frame(width: 26).foregroundStyle(.primary.opacity(0.75))
        .accessibilityHidden(true)
      Text(title).font(.body).lineLimit(1)
      Spacer(minLength: 6)
      trailing
    }
    .frame(minHeight: 30)
    .contentShape(Rectangle())
  }
}

extension ShellRow where Trailing == EmptyView {
  init(title: String, symbol: String) {
    self.init(title: title, symbol: symbol) { EmptyView() }
  }
}

/// A Zapper in the sidebar: its face, its name and what is waiting on you.
private struct ShellZapperRow: View {
  let zapper: Zapper
  let working: Int
  var body: some View {
    HStack(spacing: 10) {
      ZapperTile(zapper: zapper, size: 28)
      VStack(alignment: .leading, spacing: 0) {
        Text(zapper.config.name).font(.body).lineLimit(1)
        if let status {
          Text(status).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }
      }
      Spacer(minLength: 6)
      if zapper.needsYouCount > 0 { ZapperNeedsYouBadge(count: zapper.needsYouCount) }
    }
    .frame(minHeight: 36)
    .contentShape(Rectangle())
    .accessibilityElement(children: .combine)
  }
  private var status: String? {
    if zapper.status != .active { return zapper.status.label }
    switch working {
    case 0: return nil
    case 1: return "1 thread working"
    default: return "\(working) threads working"
    }
  }
}

private struct ShellSectionHeader: View {
  let title: String
  var body: some View {
    Text(title).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary).textCase(nil)
  }
}

extension View {
  /// Keeps a page's text to a comfortable measure on wide screens.
  func readableColumn(_ width: CGFloat = 720) -> some View {
    frame(maxWidth: width).frame(maxWidth: .infinity).background(AppTheme.surface)
  }
}

#if DEBUG && os(iOS)
  /// `--orientation=landscape` asks for landscape for screenshots, since simctl can't rotate.
  /// iPadOS refuses it while apps can be windowed; the reason is printed to the console.
  enum LaunchOrientation {
    @MainActor static func apply() {
      guard ProcessInfo.processInfo.arguments.contains("--orientation=landscape"),
        let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene })
          .first
      else { return }
      scene.requestGeometryUpdate(.iOS(interfaceOrientations: .landscapeRight)) { error in
        print("orientation request failed:", error.localizedDescription)
      }
    }
  }
#endif

/// A list row's background: clear, or a soft rounded highlight when it is open beside the list.
struct SelectedRowBackground: View {
  let selected: Bool
  var body: some View {
    if selected {
      RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color.primary.opacity(0.07))
        .padding(.horizontal, 8)
    } else {
      Color.clear
    }
  }
}
