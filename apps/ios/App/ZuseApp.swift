import PhotosUI
import SwiftUI
import UniformTypeIdentifiers
import ZuseCore

@main struct ZuseApp: App {
  @State private var workspace = WorkspaceModel()
  @AppStorage("native.appearance") private var appearance = "system"
  var body: some Scene {
    WindowGroup {
      WorkspaceView(workspace: workspace).tint(AppTheme.accent)
        .background(AppTheme.surface.ignoresSafeArea())
        .preferredColorScheme(appearance == "dark" ? .dark : appearance == "light" ? .light : nil)
        .nativeWindowMinimum()
    }
    #if os(macOS)
      .defaultSize(width: 1120, height: 760)
      .commands {
        CommandGroup(replacing: .newItem) {
          Button("New Chat") { NotificationCenter.default.post(name: .nativeNewChat, object: nil) }
          .keyboardShortcut("n")
          Button("New Cloud Chat") {
            NotificationCenter.default.post(name: .nativeNewCloudChat, object: nil)
          }
          .keyboardShortcut("n", modifiers: [.command, .shift])
        }
        CommandGroup(replacing: .appSettings) {
          Button("Settings…") {
            NotificationCenter.default.post(name: .nativeSettings, object: nil)
          }.keyboardShortcut(",")
        }
      }
    #endif
  }
}

struct WorkspaceView: View {
  @Bindable var workspace: WorkspaceModel
  @State private var linkedInvitation: String?
  @State private var photo: PhotosPickerItem?
  @State private var showFilePicker = false
  @State private var attachmentTarget: UUID?
  @State private var connectionMethod: ConnectionMethod?
  @State private var showDiff = false
  @State private var newChat: NewChatRoute?
  @State private var showProject = false
  @State private var showComputers = false
  @State private var composerHeight: CGFloat = 80
  @State private var modelCatalog: [ModelProvider] = []
  @State private var branchName: String?
  @FocusState private var composerFocused: Bool
  @SceneStorage("native.tab") private var tab: AppTab = .zappers
  /// The Pull requests tab's stack, and a pull request opened from a link elsewhere.
  @State private var pullRequestPath: [PullRequestRoute] = []
  @State private var linkedPullRequest: PullRequestLink?
  @State private var linkedPullRequestPath: [PullRequestRoute] = []
  /// The open chat's pull request, shown beside its branch in the header.
  @State private var chatPullRequest: PullRequestCheckoutSummary?
  @Environment(\.openURL) private var openURL
  /// The Chats tab: its list selection, its columns and the new chat page.
  @State private var selection: SessionRow?
  @State private var chatsColumn: NavigationSplitViewColumn = .sidebar
  @State private var chatsVisibility: NavigationSplitViewVisibility = .all
  @State private var visibilityBeforeNewChat: NavigationSplitViewVisibility = .all
  /// The Zappers tab: the Zapper open in its detail column and the pages pushed in it.
  @State private var zapperRoute: String?
  @State private var zapperPath: [ZapperRoute] = []
  @State private var zappersColumn: NavigationSplitViewColumn = .sidebar
  @State private var morePath = NavigationPath()
  @State private var searchPath: [ZapperRoute] = []
  @State private var searchText = ""
  @State private var chatActions: ChatActionsTarget?
  /// Until the user moves on, the chat restored at launch opens in the Chats tab.
  @State private var opensRestoredChat = true
  /// The wide layout: the sidebar's place, the Zapper overview inspector, the open issue
  /// stack and the open settings page.
  @State private var section: ShellSection?
  @State private var zapperInspector = false
  @State private var issuePath: [ZapperRoute] = []
  @State private var settingsPage: MoreRoute?
  @Environment(\.scenePhase) private var scenePhase
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  #if os(iOS)
    @Environment(\.horizontalSizeClass) private var sizeClass
  #endif
  private var isCompact: Bool {
    #if os(iOS)
      sizeClass == .compact
    #else
      false
    #endif
  }
  /// iPad at full or wide split widths, and Mac, use the sidebar shell; compact widths use tabs.
  private var usesRegularShell: Bool {
    #if os(iOS)
      sizeClass == .regular
    #else
      true
    #endif
  }
  private struct NewChatRoute: Equatable {
    let id = UUID()
    let cloud: Bool
    var prompt = ""
  }
  /// New chat is a page in the Chats tab's detail column: pushed on iPhone, full width elsewhere.
  private func openNewChat(cloud: Bool, prompt: String = "") {
    newChat = NewChatRoute(cloud: cloud, prompt: prompt)
    tab = .chats
    section = .chats
    chatsColumn = .detail
    if !isCompact && chatsVisibility != .detailOnly {
      visibilityBeforeNewChat = chatsVisibility
      chatsVisibility = .detailOnly
    }
  }
  private func closeNewChat() {
    newChat = nil
    if !isCompact { chatsVisibility = visibilityBeforeNewChat }
  }
  private func openZapper(_ id: String) {
    tab = .zappers
    if zapperRoute != id { zapperPath = [] }
    zapperRoute = id
    section = .zapper(id)
    zappersColumn = .detail
  }
  private func closeZapper() {
    zapperRoute = nil
    zapperPath = []
    if isCompact { zappersColumn = .sidebar }
  }
  /// Shows an ordinary chat in the Chats tab.
  private func openChat(_ session: SessionRow) {
    if newChat != nil { closeNewChat() }
    tab = .chats
    section = .chats
    selection = session
    chatsColumn = .detail
  }
  /// Leaves the chat open in the Chats tab, back to its list.
  private func closeChat() {
    selection = nil
    if isCompact { chatsColumn = .sidebar }
  }
  /// Pushes a Zapper page onto the stack of the tab on screen.
  private func push(_ route: ZapperRoute) {
    if usesRegularShell {
      // The wide layout shows a Zapper's overview beside its conversation.
      if case .overview(let id) = route {
        if zapperRoute != id { openZapper(id) }
        withAnimation(.snappy) { zapperInspector = true }
      } else {
        zapperPath.append(route)
      }
      return
    }
    switch tab {
    case .more: morePath.append(route)
    case .search: searchPath.append(route)
    default: zapperPath.append(route)
    }
  }
  private var needsYouCount: Int {
    workspace.zappers.zappers.filter { $0.status != .archived }.reduce(0) {
      $0 + $1.needsYouCount
    }
  }
  var body: some View {
    presentedWorkspace
      .alert(
        "Something needs attention",
        isPresented: errorPresented
      ) {
        Button("OK") { workspace.error = nil }
      } message: {
        Text(workspace.error ?? "")
      }
      .onOpenURL { url in
        if let invitation = PairingDeepLink.invitation(from: url) { linkedInvitation = invitation }
      }
      .confirmationDialog(
        "Add this computer?",
        isPresented: Binding(
          get: { linkedInvitation != nil }, set: { if !$0 { linkedInvitation = nil } }),
        titleVisibility: .visible
      ) {
        if let invitation = linkedInvitation {
          Button("Add computer") {
            linkedInvitation = nil
            Task {
              do { try await workspace.pair(invitation) } catch is CancellationError {} catch {
                workspace.error = error.localizedDescription
              }
            }
          }
        }
        Button("Cancel", role: .cancel) { linkedInvitation = nil }
      } message: {
        Text(
          "Only continue if you opened this link from your own computer at "
            + PairingDeepLink.destination(linkedInvitation ?? "") + "."
        )
      }
  }
  private var errorPresented: Binding<Bool> {
    Binding(get: { workspace.error != nil }, set: { if !$0 { workspace.error = nil } })
  }
  private var presentedWorkspace: some View {
    interactiveWorkspace
      .connectionFlow($connectionMethod, workspace: workspace)
      .sheet(isPresented: $showProject) { AddProjectView(workspace: workspace) }
      .sheet(isPresented: $showComputers) { ComputersSheet(workspace: workspace) }
      .sheet(item: $chatActions) { target in
        ChatActionsSheet(
          workspace: workspace, target: target,
          openProfile: {
            if case .zapper(let id) = target { push(.overview(id)) }
          },
          showChanges: { showDiff = true },
          left: {
            switch target {
            case .chat: closeChat()
            case .zapper:
              if tab == .zappers {
                closeZapper()
              } else if tab == .more, !morePath.isEmpty {
                morePath.removeLast()
              } else if tab == .search, !searchPath.isEmpty {
                searchPath.removeLast()
              }
            }
          })
      }
      .sheet(item: $linkedPullRequest) { link in
        PullRequestNavigator(
          store: workspace.pullRequests, entry: .detail(link), path: $linkedPullRequestPath,
          close: { linkedPullRequest = nil }
        )
        .nativeSheetSize(width: 720, height: 820)
      }
      .onChange(of: workspace.operationScope) {
        // Pull request screens act for one computer and account; close them on a switch.
        if zapperRoute != nil { closeZapper() }
        pullRequestPath = []
        linkedPullRequest = nil
        linkedPullRequestPath = []
      }
  }
  /// A pull request link: its own stack in a sheet, or GitHub when this computer can't show it.
  private func openPullRequestLink(_ url: String) {
    guard let link = PullRequestLink(url) else { return }
    guard workspace.pullRequests.available else {
      if let address = URL(string: link.url) { openURL(address) }
      return
    }
    linkedPullRequestPath = []
    linkedPullRequest = link
  }
  #if DEBUG
    /// `--native-preview --pr-preview` opens the sample pull request list; the detail
    /// arguments (see `PullRequestPreview`) push one sample on top of it. Without
    /// `--native-preview` (a paired computer), `--pr-preview` opens the live list and
    /// `--pr-url <link>` a live pull request.
    private func openPullRequestPreview(_ arguments: [String]) {
      guard arguments.contains(where: { $0.hasPrefix("--pr-") }) else { return }
      let sample = arguments.contains("--native-preview")
      if sample { workspace.pullRequests.loadSamplePreview() }
      guard !arguments.contains("--pr-home-preview") else { return }
      tab = .prs
      section = .pullRequests
      let live = arguments.firstIndex(of: "--pr-url").flatMap {
        arguments.indices.contains($0 + 1) ? arguments[$0 + 1] : nil
      }
      if let url = sample ? PullRequestPreview.entryURL(arguments) : live,
        let link = PullRequestLink(url)
      {
        pullRequestPath = [.detail(link)]
      }
    }
    /// Debug launches: `SIMCTL_CHILD_ZUSE_PAIR_LINK=<invitation> xcrun simctl launch …` pairs
    /// without the camera or a confirmation, then `--zapper-detail-preview` opens the first Zapper's overview
    /// (`--zapper-chat-preview` stops at its conversation).
    private func openZapperPreview(_ arguments: [String]) async {
      if let link = ProcessInfo.processInfo.environment["ZUSE_PAIR_LINK"], !link.isEmpty {
        do { try await workspace.pair(link) } catch { workspace.error = error.localizedDescription }
        for _ in 0..<80
        where workspace.zappers.zappers.isEmpty && arguments.contains("--zapper-detail-preview") {
          try? await Task.sleep(for: .milliseconds(250))
        }
      }
      // `--open-chat=<id>` opens one chat on the paired computer.
      if let id = arguments.first(where: { $0.hasPrefix("--open-chat=") })?.dropFirst(12) {
        for _ in 0..<80 where !workspace.sessions.contains(where: { $0.id == id }) {
          try? await Task.sleep(for: .milliseconds(250))
        }
        if let session = workspace.sessions.first(where: { $0.id == id }) { openChat(session) }
        return
      }
      guard arguments.contains("--zapper-detail-preview"),
        let first = workspace.zappers.zappers.first
      else { return }
      openZapper(first.id)
      if arguments.contains("--chat-actions-preview") {
        try? await Task.sleep(for: .milliseconds(500))
        chatActions = .zapper(first.id)
      }
      if arguments.contains("--zapper-chat-preview") { return }
      // Let the Zapper's stack appear on its conversation before pushing the overview.
      try? await Task.sleep(for: .milliseconds(400))
      if usesRegularShell {
        zapperInspector = true
        // Pages open from the overview once it is on screen, as they would by hand.
        try? await Task.sleep(for: .milliseconds(600))
      } else {
        zapperPath = [.overview(first.id)]
      }
      if arguments.contains("--zapper-issue-preview"),
        let issue = first.issues.first(where: { $0.details.status == .needsYou })
      {
        zapperPath.append(.issue(zapper: first.id, issue: issue.id))
      } else if arguments.contains("--zapper-document-preview") {
        zapperPath.append(.document(zapper: first.id, path: "NOTES.md"))
      }
    }
    /// `--more=<issues|archived|computers|appearance|account|help>` opens a More page,
    /// `--issue-detail-preview` the issue waiting on you and `--search=<text>` a search.
    private func openMorePreview(_ arguments: [String]) {
      if let value = arguments.first(where: { $0.hasPrefix("--more=") })?.dropFirst(7) {
        tab = .more
        switch value {
        case "issues": morePath.append(MoreRoute.issues)
        case "archived": morePath.append(MoreRoute.archived)
        case "computers": morePath.append(MoreRoute.computers)
        case "appearance": morePath.append(MoreRoute.appearance)
        case "account": morePath.append(MoreRoute.account)
        case "help": morePath.append(MoreRoute.help)
        default: break
        }
        switch value {
        case "issues": section = .issues
        case "computers": section = .computers
        case "account": settingsPage = .account
        case "appearance": settingsPage = .appearance
        case "archived": settingsPage = .archived
        case "help": settingsPage = .help
        default: section = .settings
        }
        if settingsPage != nil { section = .settings }
      }
      if arguments.contains("--issue-detail-preview"),
        let zapper = workspace.zappers.zappers.first(where: { !$0.issues.isEmpty }),
        let issue = zapper.issues.first(where: { $0.details.status == .needsYou })
          ?? zapper.issues.first
      {
        tab = .more
        if morePath.isEmpty { morePath.append(MoreRoute.issues) }
        morePath.append(ZapperRoute.issue(zapper: zapper.id, issue: issue.id))
        section = .issues
        issuePath = [.issue(zapper: zapper.id, issue: issue.id)]
      }
      if let value = arguments.first(where: { $0.hasPrefix("--search=") })?.dropFirst(9) {
        tab = .search
        searchText = String(value)
      }
    }
  #endif
  private var interactiveWorkspace: some View {
    observedWorkspace
      .fileImporter(
        isPresented: $showFilePicker, allowedContentTypes: [.item], onCompletion: importFile
      )
      .onChange(of: photo) { _, value in importPhoto(value) }
  }
  private var routedWorkspace: some View {
    layout
      .pullRequestLinks()
      #if os(iOS)
        .onReceive(
          NotificationCenter.default.publisher(for: UIResponder.keyboardDidHideNotification)
        ) { _ in
          composerFocused = false
        }
      #endif
      .onReceive(NotificationCenter.default.publisher(for: .nativeNewChat)) { _ in
        openNewChat(cloud: false)
      }
      .onReceive(NotificationCenter.default.publisher(for: .nativeNewCloudChat)) { _ in
        openNewChat(cloud: true)
      }
      .onReceive(NotificationCenter.default.publisher(for: .nativeSettings)) { _ in
        tab = .more
        section = .settings
      }
      .onReceive(NotificationCenter.default.publisher(for: .nativeOpenPullRequest)) { note in
        if let url = note.object as? String { openPullRequestLink(url) }
      }
      .task(id: "pr|" + workspace.connectionState + (workspace.selectedSession?.id ?? "")) {
        chatPullRequest = nil
        guard workspace.connectionState == "Connected", let session = workspace.selectedSession
        else { return }
        chatPullRequest = await workspace.pullRequests.checkoutPullRequest(chatID: session.id)
      }
      .task(id: workspace.connectionState + (workspace.selectedSession?.id ?? "")) {
        modelCatalog = []
        branchName = nil
        #if DEBUG
          if ProcessInfo.processInfo.arguments.contains("--native-preview") {
            do { modelCatalog = try WorkspaceModel.previewModelCatalog() } catch {
              workspace.error = error.localizedDescription
            }
            return
          }
        #endif
        if workspace.connectionState == "Connected", let rpc = workspace.rpc {
          do {
            let models = try await ModelProvider.load(rpc)
            try Task.checkCancellation()
            modelCatalog = models
            if let session = workspace.selectedSession {
              let branch: String?
              if await rpc.canonical {
                branch = try await CanonicalWorkspace(rpc).branch(chatID: session.id)
              } else {
                var payload: [String: JSONValue] = ["folderId": .string(session.projectId)]
                if let worktree = session.worktreeId { payload["worktreeId"] = .string(worktree) }
                branch = try await rpc.call("git.status", payload)["branch"].string
              }
              try Task.checkCancellation()
              branchName = branch
            }
          } catch {
            // The session's current model remains visible when discovery is unavailable.
          }
        }
      }
      .task(id: workspace.account.generation) {
        // The cloud catalog stays fresh whichever tab is open.
        var delay = 5.0
        while !Task.isCancelled && workspace.account.signedIn {
          do { try await Task.sleep(for: .seconds(delay)) } catch { return }
          if workspace.shouldPollCloudCatalog {
            await workspace.refreshCloud(reportErrors: false)
            delay = workspace.cloudCatalogProblem == nil ? 5 : min(30, delay * 2)
          }
        }
      }
  }
  private var observedWorkspace: some View {
    routedWorkspace
      .onChange(of: workspace.selectedSession) { _, session in
        guard let session else { return }
        // Zapper conversations stay in the stack that opened them.
        if workspace.zappers.keepsChat(session.id) {
          opensRestoredChat = false
          return
        }
        guard tab == .chats || newChat != nil || opensRestoredChat else { return }
        opensRestoredChat = false
        if selection?.id != session.id || newChat != nil { openChat(session) }
      }
      .onChange(of: chatsColumn) { _, column in
        // Popping the pushed page on iPhone returns to the list.
        if column == .sidebar && isCompact { newChat = nil }
      }
      .onChange(of: zappersColumn) { _, column in
        if column == .sidebar && isCompact {
          zapperRoute = nil
          zapperPath = []
        }
      }
      .onChange(of: selection) {
        if selection != nil && newChat != nil { closeNewChat() }
        if selection != nil { chatsColumn = .detail }
        if let selection, selection.id != workspace.selectedSession?.id {
          workspace.selectSession(selection)
        }
      }
      .onChange(of: tab) { opensRestoredChat = false }
      .onChange(of: section) { _, section in followSection(section) }
      .onChange(of: zapperRoute) { _, route in
        if route == nil, case .zapper = section { section = .chats }
      }
      .onChange(of: scenePhase) { _, phase in workspace.sceneChanged(active: phase == .active) }
      .onChange(of: workspace.account.generation) {
        if !workspace.account.signedIn && newChat != nil { closeNewChat() }
        connectionMethod = nil
        showProject = false
        showComputers = false
        showDiff = false
        chatActions = nil
      }
      .onChange(of: workspace.draft) { workspace.persistDraft() }
      .onChange(of: workspace.connectionState) { _, state in
        if state != "Connected" { showDiff = false }
      }
      .task { await launch() }
  }
  /// Restores the last place, then applies debug launch arguments.
  private func launch() async {
    #if DEBUG && os(iOS)
      LaunchOrientation.apply()
    #endif
    await workspace.refreshCloud()
    workspace.restoreWorkspace()
    let launchTab = AppTab.launchArgument
    if let session = workspace.selectedSession, !workspace.zappers.keepsChat(session.id) {
      opensRestoredChat = false
      selection = session
      chatsColumn = .detail
      if launchTab == nil { tab = .chats }
    }
    if let launchTab { tab = launchTab }
    #if DEBUG
      let arguments = ProcessInfo.processInfo.arguments
      if launchTab == nil && arguments.contains("--home-preview") { tab = .chats }
      if arguments.contains("--settings-preview") {
        tab = .more
        section = .settings
      }
      if arguments.contains("--new-chat-preview") { openNewChat(cloud: false) }
      if arguments.contains("--pairing-preview") { connectionMethod = .add }
      if arguments.contains("--chat-actions-preview"), let session = selection {
        try? await Task.sleep(for: .milliseconds(500))
        chatActions = .chat(session)
      }
      openMorePreview(arguments)
      await openZapperPreview(arguments)
      openPullRequestPreview(arguments)
    #endif
    if section == nil { section = initialSection }
  }
  /// The wide layout starts where the tabs would: the tab's place, or the first Zapper.
  private var initialSection: ShellSection {
    switch tab {
    case .zappers:
      let id = zapperRoute ?? workspace.zappers.zappers.first { $0.status != .archived }?.id
      return id.map { .zapper($0) } ?? .chats
    case .chats, .search: return .chats
    case .prs: return .pullRequests
    case .more: return .settings
    }
  }
  /// Keeps the tabs in step with the sidebar, so a narrower window keeps your place.
  private func followSection(_ section: ShellSection?) {
    guard usesRegularShell, let section else { return }
    switch section {
    case .zapper(let id):
      tab = .zappers
      if zapperRoute != id {
        zapperPath = []
        zapperRoute = id
      }
    case .chats: tab = .chats
    case .pullRequests: tab = .prs
    case .issues, .computers, .settings: tab = .more
    case .search: break
    }
  }
  private func importFile(_ result: Result<URL, Error>) {
    Task {
      do {
        let url = try result.get()
        let access = url.startAccessingSecurityScopedResource()
        defer { if access { url.stopAccessingSecurityScopedResource() } }
        let resources = try url.resourceValues(forKeys: [.fileSizeKey, .contentTypeKey])
        guard (resources.fileSize ?? 0) <= Attachments.maximumBytes else {
          throw ClientError("Attachments must be 100 MB or smaller.")
        }
        let data = try await Task.detached(priority: .userInitiated) {
          let file = try FileHandle(forReadingFrom: url)
          defer { try? file.close() }
          return try file.read(upToCount: Attachments.maximumBytes + 1) ?? Data()
        }.value
        guard attachmentTarget == workspace.sessionSelection else { return }
        await workspace.attach(
          data, name: url.lastPathComponent,
          mimeType: resources.contentType?.preferredMIMEType ?? "application/octet-stream")
      } catch { workspace.error = error.localizedDescription }
    }
  }
  private func importPhoto(_ value: PhotosPickerItem?) {
    guard let value else { return }
    let target = workspace.sessionSelection
    Task {
      do {
        if let data = try await value.loadTransferable(type: Data.self) {
          guard target == workspace.sessionSelection else { return }
          let type = value.supportedContentTypes.first ?? .image
          await workspace.attach(
            data, name: "Photo." + (type.preferredFilenameExtension ?? "jpg"),
            mimeType: type.preferredMIMEType ?? "image/jpeg")
        }
      } catch { workspace.error = error.localizedDescription }
      photo = nil
    }
  }

  @ViewBuilder private var layout: some View {
    if usesRegularShell { regularShell } else { tabs }
  }

  private var regularShell: some View {
    RegularShell(
      workspace: workspace, section: $section, chatSelection: $selection, zapperID: zapperRoute,
      zapperPath: $zapperPath, inspector: $zapperInspector, pullRequestPath: $pullRequestPath,
      issuePath: $issuePath, settingsPage: $settingsPage, searchText: $searchText,
      actions: ShellActions(
        openZapper: openZapper, closeZapper: closeZapper, openChat: openChat,
        openCloud: { chat in
          tab = .chats
          section = .chats
          workspace.openCloud(chat)
        },
        selectSession: { session in workspace.selectSession(session) },
        newChat: { prompt in openNewChat(cloud: false, prompt: prompt) },
        newCloudChat: { openNewChat(cloud: true) },
        connect: { connectionMethod = .add }, addProject: { showProject = true })
    ) { session in
      chatScreen(session)
    } chatsDetail: {
      regularChatsDetail
    }
  }

  /// The wide layout's Chats detail. With nothing open it is the new chat composer, so the
  /// next chat is one message away.
  private var regularChatsDetail: some View {
    Group {
      if let route = newChat {
        NewChatSetupView(
          workspace: workspace, initialCloud: route.cloud, initialPrompt: route.prompt,
          showsClose: selection != nil, close: closeNewChat
        )
        .id(route.id)
      } else if let session = selection {
        chatScreen(session)
      } else {
        NewChatSetupView(workspace: workspace, showsClose: false, close: {})
          .id(workspace.operationScope ?? "new-chat")
      }
    }
    .inspector(isPresented: $showDiff) {
      if let session = workspace.selectedSession, let rpc = workspace.rpc {
        DiffView(rpc: rpc, session: session).inspectorColumnWidth(min: 320, ideal: 440, max: 680)
      }
    }
  }

  private var tabs: some View {
    TabView(selection: $tab) {
      Tab(value: AppTab.zappers) {
        zappersTab
      } label: {
        Label("Zappers", image: AppIcon.zapper)
      }
      .badge(needsYouCount)
      Tab(value: AppTab.chats) {
        chatsTab
      } label: {
        Label("Chats", systemImage: "bubble.left.and.bubble.right")
          .environment(\.symbolVariants, .none)
      }
      Tab(value: AppTab.prs) {
        PullRequestsTab(workspace: workspace, path: $pullRequestPath)
      } label: {
        Label("Pull requests", image: AppIcon.pullRequest)
          .environment(\.symbolVariants, .none)
      }
      Tab(value: AppTab.more) {
        moreTab
      } label: {
        Label {
          Text("More")
        } icon: {
          Image(systemName: "ellipsis.circle")
        }
        .environment(\.symbolVariants, .none)
      }
      Tab(value: AppTab.search, role: .search) { searchTab }
    }
    .tabViewStyle(.sidebarAdaptable)
  }

  private var zappersTab: some View {
    NavigationSplitView(preferredCompactColumn: $zappersColumn) {
      ZappersListView(
        workspace: workspace, open: openZapper, connect: { connectionMethod = .add }
      )
      .disabled(workspace.clearingAccount)
      .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 360)
    } detail: {
      if let zapperID = zapperRoute {
        ZapperNavigator(
          workspace: workspace, zapperID: zapperID, path: $zapperPath,
          select: { session in workspace.selectSession(session) }, close: closeZapper
        ) { session in
          chatScreen(session)
        }
        .id(zapperID)
      } else {
        emptyDetail("Pick a Zapper", symbol: AppIcon.zapper, detail: "Its conversation opens here.")
      }
    }
    .navigationSplitViewStyle(.balanced)
  }

  private var chatsTab: some View {
    NavigationSplitView(
      columnVisibility: $chatsVisibility, preferredCompactColumn: $chatsColumn
    ) {
      ChatsListView(
        workspace: workspace, selection: $selection, connect: { connectionMethod = .add },
        newChat: { prompt in openNewChat(cloud: false, prompt: prompt) },
        newCloudChat: { openNewChat(cloud: true) }, addProject: { showProject = true },
        openComputers: { showComputers = true }
      )
      .disabled(workspace.clearingAccount)
      .navigationSplitViewColumnWidth(min: 260, ideal: 320, max: 380)
      .toolbar(removing: newChat == nil ? nil : .sidebarToggle)
    } detail: {
      if let route = newChat {
        NewChatSetupView(
          workspace: workspace, initialCloud: route.cloud, initialPrompt: route.prompt,
          showsClose: !isCompact, close: closeNewChat
        )
        .id(route.id)
        .hidesTabBarWhenCompact()
      } else if let session = selection {
        chatScreen(session)
      } else {
        emptyDetail(
          "Your work, wherever you are", symbol: "bubble.left.and.bubble.right",
          detail: "Open a chat, or ask anything to start one.")
      }
    }
    .navigationSplitViewStyle(.balanced)
    .inspector(isPresented: $showDiff) {
      if let session = workspace.selectedSession, let rpc = workspace.rpc {
        DiffView(rpc: rpc, session: session).inspectorColumnWidth(min: 320, ideal: 440, max: 680)
      }
    }
  }

  private var moreTab: some View {
    NavigationStack(path: $morePath) {
      MoreView(workspace: workspace)
        .navigationDestination(for: MoreRoute.self) { route in
          switch route {
          case .account: AccountSettingsView(workspace: workspace)
          case .issues: IssuesListView(workspace: workspace) { morePath.append($0) }
          case .computers: ComputersView(workspace: workspace)
          case .appearance: AppearanceSettingsView()
          case .archived: ArchivedChatsView(workspace: workspace, open: openChat)
          case .help: HelpSettingsView { morePath.append(MoreRoute.computers) }
          }
        }
        .navigationDestination(for: ZapperRoute.self) { route in
          ZapperDestination(
            workspace: workspace, route: route, push: { morePath.append($0) },
            close: { if !morePath.isEmpty { morePath.removeLast() } }
          ) { session in
            chatScreen(session)
          }
        }
    }
  }

  private var searchTab: some View {
    NavigationStack(path: $searchPath) {
      SearchView(
        workspace: workspace, query: $searchText, openZapper: openZapper, openChat: openChat,
        openCloud: { chat in
          tab = .chats
          workspace.openCloud(chat)
        },
        openIssue: { entry in
          searchPath.append(.issue(zapper: entry.zapperID, issue: entry.issue.id))
        }
      )
      .navigationDestination(for: ZapperRoute.self) { route in
        ZapperDestination(
          workspace: workspace, route: route, push: { searchPath.append($0) },
          close: { if !searchPath.isEmpty { searchPath.removeLast() } }
        ) { session in
          chatScreen(session)
        }
      }
    }
  }

  private func emptyDetail(_ title: String, symbol: String, detail: String) -> some View {
    ContentUnavailableView {
      Label {
        Text(title)
      } icon: {
        AppIcon(name: symbol, size: 40, weight: .light)
      }
    } description: {
      Text(detail)
    }
    .overlay {
      if workspace.loading {
        ProgressView().padding().background(.regularMaterial, in: Capsule())
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity).background(AppTheme.surface)
  }
  /// One chat's transcript and composer, shown from the home list or inside a Zapper.
  /// A chat page owns the bottom edge for its composer, so the tab bar steps aside.
  private func chatScreen(_ session: SessionRow) -> some View {
    chatPage(session).hidesTabBar()
  }
  @ViewBuilder private func chatPage(_ session: SessionRow) -> some View {
    VStack(spacing: 0) {
      if workspace.transcript.olderSequence != nil {
        Button(workspace.paging ? "Loading…" : "Load earlier messages") {
          Task { await workspace.loadOlder() }
        }
        .font(.caption).disabled(workspace.paging || !workspace.sessionReady).padding(6)
      }
      TranscriptView(
        messages: workspace.transcript.messages, sessionID: session.id, rpc: workspace.rpc,
        running: workspace.transcript.activeTurnID != nil,
        // Clear of the fade above the composer, so the latest line reads at full strength.
        bottomInset: composerHeight + 20,
        respond: { message, text in try await workspace.respondToUI(message, text: text) })
      if !workspace.outbox.isEmpty {
        ScrollView(.horizontal) {
          HStack {
            ForEach(workspace.outbox) { message in
              VStack(alignment: .leading, spacing: 5) {
                Text(message.text).lineLimit(2)
                HStack {
                  Text(message.delivery == .queued ? "Queued" : "Delivery unconfirmed")
                    .foregroundStyle(.secondary)
                  if message.delivery == .uncertain {
                    Button("Retry") {
                      Task { await workspace.flushOutbox(retryID: message.id) }
                    }
                  }
                  Button("Remove", role: .destructive) {
                    Task { await workspace.removePending(message) }
                  }
                }
              }.font(.caption).padding(10).frame(maxWidth: 280).background(
                .quaternary, in: RoundedRectangle(cornerRadius: 12))
            }
          }.padding(.horizontal)
        }.frame(maxHeight: 100)
      }
      if !workspace.transcript.interactions.isEmpty {
        ForEach(workspace.transcript.interactions, id: \.interactionIdentity) { interaction in
          if interaction["_tag"].string == "Permission", let id = interaction["id"].string {
            PermissionView(
              request: interaction["request"], enabled: workspace.connectionState == "Connected"
            ) { allow in
              await workspace.decidePermission(id, allow: allow)
            }
          } else {
            InteractionView(interaction: interaction, workspace: workspace)
          }
        }
      }
    }
    .background(AppTheme.surface)
    .overlay(alignment: .bottom) {
      composer.background {
        GeometryReader { geometry in
          LinearGradient(
            stops: [
              // Text fades out before it reaches the composer instead of showing through it.
              .init(color: AppTheme.surface.opacity(0), location: 0),
              .init(color: AppTheme.surface.opacity(0.75), location: 0.4),
              .init(color: AppTheme.surface.opacity(0.95), location: 1),
            ], startPoint: .top, endPoint: .bottom
          )
          .frame(height: geometry.size.height + 64)
          .offset(y: -64)
          .allowsHitTesting(false)
          .preference(key: ComposerHeightKey.self, value: geometry.size.height)
        }
      }
    }
    .onPreferenceChange(ComposerHeightKey.self) { composerHeight = $0 }
    .navigationTitle(session.title.isEmpty ? "Chat" : session.title)
    .compactNavigationTitle()
    .hidesTabBarWhenCompact()
    // Each stack shows its own chat; the one transcript follows whichever is on screen.
    .onAppear {
      if workspace.selectedSession?.id != session.id { workspace.selectSession(session) }
    }
    .toolbar {
      if let zapper = coordinatedBy(session) {
        // A Zapper's conversation: its face and name lead to the overview.
        ToolbarItem(placement: .principal) {
          Button {
            push(.overview(zapper.id))
          } label: {
            HStack(spacing: 8) {
              ZapperTile(zapper: zapper, size: 28)
              Text(zapper.config.name).font(.headline).foregroundStyle(.primary).lineLimit(1)
            }
          }
          .buttonStyle(.plain)
          .accessibilityLabel("\(zapper.config.name) overview")
        }
        ToolbarItem(placement: .primaryAction) {
          Button {
            chatActions = .zapper(zapper.id)
          } label: {
            Image(systemName: "ellipsis")
              .overlay(alignment: .topTrailing) {
                if zapper.needsYouCount > 0 {
                  Circle().fill(Color.orange).frame(width: 8, height: 8).offset(x: 6, y: -6)
                }
              }
          }
          .tint(.primary)
          .accessibilityLabel(
            zapper.needsYouCount > 0
              ? "More, \(zapper.needsYouCount) waiting for you" : "More"
          )
          .accessibilityIdentifier("chat-actions")
        }
      } else {
        ToolbarItem(placement: .principal) {
          ChatHeaderTitle(
            title: session.title.isEmpty ? "Chat" : session.title,
            subtitle: [
              workspace.projects.first { $0.id == session.projectId }?.name, branchName,
            ].compactMap { $0 }.joined(separator: " · "),
            pullRequest: chatPullRequest,
            openPullRequest: { openPullRequestLink($0.url) })
        }
        if !isCompact && !workspace.zappers.keepsChat(session.id) {
          ToolbarItem(placement: .primaryAction) {
            Button("Changes", systemImage: "plusminus") { showDiff.toggle() }
              .tint(.primary).disabled(workspace.connectionState != "Connected")
          }
        }
        if !workspace.zappers.keepsChat(session.id) {
          ToolbarItem(placement: .primaryAction) {
            Button("More", systemImage: "ellipsis") { chatActions = .chat(session) }
              .tint(.primary)
              .accessibilityIdentifier("chat-actions")
          }
        }
      }
    }
  }
  /// The Zapper whose coordinator conversation this is, if any.
  private func coordinatedBy(_ session: SessionRow) -> Zapper? {
    workspace.zappers.zappers.first { $0.coordinatorChatId == session.id }
  }
  /// The composer rests on one line and opens while you write.
  private var composerExpanded: Bool {
    composerFocused || !workspace.attachments.isEmpty
  }
  /// Where a message from the composer will run.
  private var composerContext: String? {
    let name = workspace.projects.first { $0.id == workspace.selectedSession?.projectId }?.name
    let parts = [name, branchName].compactMap { $0 }
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
  }
  private var composerPlaceholder: String {
    if workspace.connectionIssue { return "Reconnecting to your computer…" }
    return workspace.transcript.activeTurnID != nil ? "Send a follow-up…" : "Ask Zuse"
  }
  private var hasDraft: Bool {
    !workspace.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      || !workspace.attachments.isEmpty
  }
  /// Mid-turn a written message is a follow-up, and the host queues it.
  private var stopsTurn: Bool { workspace.transcript.activeTurnID != nil && !hasDraft }
  private var composer: some View {
    VStack(spacing: 8) {
      connectionNotice
      VStack(spacing: 0) {
        if !workspace.attachments.isEmpty {
          ScrollView(.horizontal) {
            HStack(spacing: 6) {
              ForEach(Array(workspace.attachments.enumerated()), id: \.offset) { _, ref in
                HStack(spacing: 5) {
                  Label(ref["originalName"].string ?? "Attachment", systemImage: "paperclip")
                    .lineLimit(1)
                  Button("Remove", systemImage: "xmark.circle.fill") {
                    workspace.removeAttachment(ref["id"].string ?? "")
                  }.labelStyle(.iconOnly)
                }.font(.caption).padding(7).background(.quaternary, in: Capsule())
              }
            }.padding(.horizontal, 12).padding(.top, 10)
          }.scrollIndicators(.hidden)
        }
        if composerExpanded, let context = composerContext {
          Text(context).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14).padding(.top, 10)
        }
        TextField(composerPlaceholder, text: $workspace.draft, axis: .vertical)
          .disabled(!workspace.composerEditable && workspace.connectionState != "Preview · offline")
          .focused($composerFocused).accessibilityIdentifier("chat-composer")
          .font(.system(size: 17)).lineLimit(1...(composerExpanded ? 6 : 1))
          .padding(.leading, composerExpanded ? 14 : 46)
          .padding(.trailing, composerExpanded ? 14 : 186)
          .padding(.top, composerExpanded ? 12 : 12).padding(.bottom, composerExpanded ? 2 : 12)
        if composerExpanded {
          HStack(spacing: 4) {
            attachmentMenu
            Spacer(minLength: 4)
            modelMenu
            sendControl
          }.padding(.horizontal, 4)
        }
      }
      .overlay(alignment: .leading) { if !composerExpanded { attachmentMenu.padding(.leading, 3) } }
      .overlay(alignment: .trailing) {
        if !composerExpanded {
          HStack(spacing: 4) {
            modelMenu.frame(maxWidth: 128)
            sendControl
          }.padding(.trailing, 3)
        }
      }
      .frame(minHeight: composerExpanded ? 0 : 44)
      .composerGlass()
      .accessibilityElement(children: .contain)
      .accessibilityIdentifier(composerExpanded ? "composer-expanded" : "composer-collapsed")
    }
    .frame(maxWidth: 720).frame(maxWidth: .infinity)
    .padding(.horizontal, 12).padding(.top, 6).padding(.bottom, 6)
  }
  /// Brief reconnects stay silent; only lasting or user-actionable failures are shown.
  @ViewBuilder private var connectionNotice: some View {
    if let failure = workspace.sessionRestorationError {
      HStack(spacing: 8) {
        Image(systemName: "arrow.clockwise").foregroundStyle(.secondary)
        Text(failure).lineLimit(2)
        Spacer(minLength: 4)
        Button("Retry") { workspace.retrySessionRestoration() }
      }.font(.caption).padding(.horizontal, 12).padding(.vertical, 9).floatingGlass(radius: 14)
    } else if workspace.connectionState == "Preview · offline" {
      HStack(spacing: 5) {
        Image(systemName: "wifi.slash")
        Text("Offline preview")
      }.font(.caption2).foregroundStyle(.secondary)
        .padding(.horizontal, 8).padding(.vertical, 3)
        .background(.ultraThinMaterial, in: Capsule())
        .frame(maxWidth: .infinity, alignment: .leading)
    } else if workspace.connectionIssue {
      HStack(spacing: 8) {
        Circle().fill(.orange).frame(width: 6, height: 6)
        Text(
          workspace.connectionState == "Connection needs attention"
            ? (workspace.connectionProblem ?? "Connection needs attention")
            : "Reconnecting… Messages you send will be delivered when it’s back."
        ).lineLimit(2)
        Spacer(minLength: 4)
        Button("Retry") { workspace.reconnect() }.fontWeight(.medium).tint(AppTheme.accent)
      }
      .font(.caption).foregroundStyle(.secondary)
      .padding(.horizontal, 12).padding(.vertical, 8).floatingGlass(radius: 14)
    }
  }
  private var attachmentMenu: some View {
    Menu {
      PhotosPicker(selection: $photo, matching: .images) {
        Label("Photo library", systemImage: "photo")
      }
      Button("Choose file", systemImage: "doc") {
        attachmentTarget = workspace.sessionSelection
        showFilePicker = true
      }
    } label: {
      Group {
        if workspace.uploading {
          ProgressView()
        } else {
          Image(systemName: "plus").font(.system(size: 20))
        }
      }.frame(width: 44, height: 48).contentShape(Rectangle())
    }.foregroundStyle(.secondary).accessibilityLabel("Add attachment")
      .disabled(
        !workspace.composerEditable || workspace.uploading
          || workspace.connectionState != "Connected")
  }
  @ViewBuilder private var modelMenu: some View {
    if let session = workspace.selectedSession {
      ModelPickerButton(
        catalog: modelCatalog, providerID: session.providerId, modelID: session.model,
        lockedProvider: isSampleCatalog ? nil : session.providerId, sample: isSampleCatalog,
        modelOptions: Binding(
          get: { workspace.selectedModelOptions }, set: { workspace.selectedModelOptions = $0 })
      ) { _, model in
        #if DEBUG
          if isSampleCatalog {
            workspace.choosePreviewModel(model)
            return
          }
        #endif
        Task { await workspace.setModel(model) }
      }.disabled(modelPickerDisabled)
    }
  }
  private var isSampleCatalog: Bool {
    #if DEBUG
      return ProcessInfo.processInfo.arguments.contains("--native-preview")
    #else
      return false
    #endif
  }
  private var modelPickerDisabled: Bool {
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--native-preview") {
        return modelCatalog.isEmpty
      }
    #endif
    return !workspace.sessionReady || workspace.connectionState != "Connected"
      || workspace.transcript.activeTurnID != nil || modelCatalog.isEmpty
  }
  private var sendControl: some View {
    Button {
      Task {
        if stopsTurn {
          await workspace.stop()
        } else {
          await workspace.send()
        }
      }
    } label: {
      ComposerSendGlyph(
        symbol: stopsTurn ? "stop.fill" : "arrow.up",
        enabled: !sendDisabled, busy: workspace.sending)
    }.buttonStyle(.plain)
      .accessibilityLabel(stopsTurn ? "Stop response" : "Send message")
      .disabled(sendDisabled).keyboardShortcut(.return, modifiers: [.command])
  }
  private var sendDisabled: Bool {
    if stopsTurn { return workspace.sending || workspace.connectionState != "Connected" }
    return !workspace.canSend || workspace.sending || !hasDraft
  }
}

struct PermissionView: View {
  let request: JSONValue
  let enabled: Bool
  let decide: (Bool) async -> Void
  @State private var submitting = false
  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Label("Approval required", systemImage: "hand.raised").font(.subheadline.weight(.semibold))
      ScrollView {
        Text(request["kind"].pretty).font(.system(.caption, design: .monospaced)).textSelection(
          .enabled)
      }.frame(maxHeight: 130)
      HStack {
        Button("Deny", role: .destructive) { submit(false) }
        Spacer()
        Button("Allow once") { submit(true) }.buttonStyle(.borderedProminent)
      }.disabled(!enabled || submitting || request["recoveryState"].string == "expired")
    }.padding(16).background(.regularMaterial)
  }
  private func submit(_ allow: Bool) {
    submitting = true
    Task {
      await decide(allow)
      submitting = false
    }
  }
}

extension JSONValue {
  fileprivate var interactionIdentity: String {
    (self["_tag"].string ?? "") + ":" + (self["id"].string ?? "")
  }
}

private struct ChatHeaderTitle: View {
  let title: String
  let subtitle: String
  var pullRequest: PullRequestCheckoutSummary?
  var openPullRequest: (PullRequestCheckoutSummary) -> Void = { _ in }
  var body: some View {
    VStack(spacing: 1) {
      Text(title).font(.system(size: 17, weight: .semibold)).lineLimit(1)
      if !subtitle.isEmpty || pullRequest != nil {
        HStack(spacing: 5) {
          if !subtitle.isEmpty {
            Text(subtitle).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
          }
          if let pullRequest {
            // The checkout's pull request opens in the app.
            Button {
              openPullRequest(pullRequest)
            } label: {
              HStack(spacing: 3) {
                PullRequestGlyph(state: pullRequest.state, draft: pullRequest.isDraft, size: 11)
                Text("#\(pullRequest.number)").font(.system(size: 13, weight: .medium))
                  .monospacedDigit()
              }
              .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Pull request \(pullRequest.number)")
            .accessibilityIdentifier("chat-pull-request")
          }
        }
      }
    }
    .frame(maxWidth: 240)
    .accessibilityElement(children: pullRequest == nil ? .combine : .contain)
    .accessibilityAddTraits(.isHeader)
  }
}
