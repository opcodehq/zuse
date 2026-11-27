import SwiftUI
import ZuseCore

/// What the Chats list shows: everything, one project, or the archive.
enum ChatsView: Hashable {
  case all
  case project(String)
  case archived
  var title: String {
    switch self {
    case .all: return "All chats"
    case .project(let name): return name
    case .archived: return "Archived"
    }
  }
}

/// The Chats tab: every ordinary chat on the open computer and in Zuse Cloud, most recent first.
/// Zapper conversations and their threads live under the Zappers tab instead.
struct ChatsListView: View {
  @Bindable var workspace: WorkspaceModel
  @Binding var selection: SessionRow?
  let connect: () -> Void
  /// Opens a new chat, with any text already typed into the ask field.
  let newChat: (String) -> Void
  let newCloudChat: () -> Void
  let addProject: () -> Void
  let openComputers: () -> Void
  /// The ask field under the list; the wide layout starts chats from the detail column.
  var showsAsk = true
  /// Highlights the open chat, for layouts that show it beside the list.
  var highlightsSelection = false
  @Environment(\.tabletLists) private var tablet
  @State private var view: ChatsView = .all
  @State private var ask = ""
  @State private var cloudConfirmation: CloudActionSelection?
  @State private var signingIn = false
  @FocusState private var askFocused: Bool
  private struct CloudActionSelection {
    let chat: CloudChat
    let action: CloudWorkspaceAction
  }
  private static let computerPlace = "computer"
  private static func key(_ session: SessionRow) -> String { computerPlace + "|" + session.id }
  private static func key(_ chat: CloudChat) -> String { ChatList.cloudPlace + "|" + chat.id }

  /// Every chat the list can show: the open computer's, active and archived, then Zuse Cloud's.
  private var chatRows: [ChatListRow] {
    var rows: [ChatListRow] = []
    if !workspace.isCloudActive {
      let label = workspace.connectedComputer ?? "Computer"
      let names = Dictionary(
        workspace.projects.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
      for (sessions, archived) in [(workspace.sessions, false), (workspace.archivedSessions, true)]
      {
        for session in sessions where !workspace.zappers.keepsChat(session.id) {
          let name =
            session.projectId == CanonicalWorkspace.projectless ? nil : names[session.projectId]
          rows.append(
            ChatListRow(
              id: Self.key(session), chatID: session.id, placeKey: Self.computerPlace,
              placeLabel: label, projectKey: session.projectId, projectName: name,
              title: session.title.isEmpty ? "New chat" : session.title, archived: archived,
              lastActivity: workspace.zappers.chats[session.id]?.lastMessageAt))
        }
      }
    }
    for chat in workspace.cloudChats {
      rows.append(
        ChatListRow(
          id: Self.key(chat), chatID: chat.chatId, placeKey: ChatList.cloudPlace,
          placeLabel: "Zuse Cloud", projectKey: chat.projectId,
          projectName: chat.repositoryDisplayName,
          title: chat.title.isEmpty ? "Untitled chat" : chat.title, archived: chat.isArchived))
    }
    return rows
  }

  var body: some View {
    let all = chatRows
    let rows: [ChatListRow] = {
      switch view {
      case .all: return ChatList.recent(all)
      case .project(let name): return ChatList.recent(all, project: name)
      case .archived: return ChatList.recent(all, archive: .archived)
      }
    }()
    let sessionsByKey = Dictionary(
      (workspace.sessions + workspace.archivedSessions).map { (Self.key($0), $0) },
      uniquingKeysWith: { first, _ in first })
    let cloudByKey = Dictionary(
      workspace.cloudChats.map { (Self.key($0), $0) }, uniquingKeysWith: { first, _ in first })
    let projects = ChatList.projectNames(all.filter { !$0.archived })
    List(selection: $selection) {
      if showsConnectionBanner {
        ConnectionBanner(workspace: workspace).chatsRow(minHeight: 0).padding(.vertical, 4)
        if rows.isEmpty && !workspace.connectionIssue {
          LoadingRows(count: 5, tile: 18, circle: true).chatsRow(minHeight: 0).padding(.top, 10)
        }
      }
      if let problem = workspace.catalogProblem {
        ChatsNotice(text: problem, icon: "arrow.clockwise")
      }
      if let problem = workspace.cloudCatalogProblem {
        ChatsNotice(text: problem, icon: "cloud.bolt")
      }
      ForEach(rows) { row in
        if row.archived {
          ArchivedChatRow(
            title: row.title, cloud: row.isCloud, project: row.projectName,
            busy: restoring(row, chat: cloudByKey[row.id])
          ) { open in
            restore(row, open: open)
          }
          .chatsRow()
        } else if row.isCloud, let chat = cloudByKey[row.id] {
          cloudRow(chat)
        } else if let session = sessionsByKey[row.id] {
          SessionListRow(
            workspace: workspace, session: session, project: row.projectName,
            selected: highlightsSelection && selection?.id == session.id
          )
          .swipeActions {
            Button("Archive", systemImage: "archivebox") {
              Task { await workspace.archiveChat(session) }
            }
            .tint(.gray)
          }
        }
      }
      if rows.isEmpty { emptyState.chatsRow(minHeight: 0).padding(.top, 64) }
    }
    .listStyle(.plain)
    .environment(\.defaultMinListRowHeight, 32)
    .contentMargins(.top, 2, for: .scrollContent)
    .scrollContentBackground(.hidden)
    .background(AppTheme.surface)
    .navigationTitle("Chats")
    .compactNavigationTitle()
    .toolbar {
      ToolbarItem(placement: .primaryAction) {
        Menu {
          Button("New chat", systemImage: "square.and.pencil") { newChat(ask) }
          if workspace.account.signedIn {
            Button("New cloud chat", systemImage: "cloud") { newCloudChat() }
          }
          if workspace.connectionState == "Connected" && !workspace.isCloudActive {
            Button("Add project", systemImage: "folder.badge.plus", action: addProject)
          }
          Divider()
          Button("Computers", systemImage: "desktopcomputer", action: openComputers)
        } label: {
          Label("More", systemImage: "ellipsis")
        }
        .accessibilityIdentifier("chats-more")
      }
    }
    .tint(.primary)
    // The view chip stays above the list: a menu inside a selectable list row won't open.
    .topBar {
      chips(projects: projects)
        .padding(.horizontal, 16).padding(.top, 2).padding(.bottom, 6)
    }
    .safeAreaInset(edge: .bottom) { if showsAsk { askField } }
    .confirmationDialog(
      cloudConfirmation?.action == .archive ? "Archive cloud workspace?" : "Pause cloud workspace?",
      isPresented: Binding(
        get: { cloudConfirmation != nil }, set: { if !$0 { cloudConfirmation = nil } })
    ) {
      if let selection = cloudConfirmation {
        Button(selection.action.title, role: selection.action == .archive ? .destructive : nil) {
          cloudConfirmation = nil
          Task { await workspace.controlCloud(selection.chat, action: selection.action) }
        }
      }
    } message: {
      Text(
        cloudConfirmation?.action == .archive
          ? "The workspace will stop and move to Archived. Available archives can be restored later."
          : "Active agent and terminal work may be interrupted. You can wake this workspace later.")
    }
    .refreshable {
      await workspace.refreshCloud()
      await workspace.refreshSessionCatalog()
    }
  }

  // MARK: Header

  /// One choice in the view menu, checked when it is the current one.
  private func choice(_ option: ChatsView, symbol: String) -> some View {
    Button {
      view = option
    } label: {
      Label(option.title, systemImage: view == option ? "checkmark" : symbol)
    }
  }

  private func chips(projects: [String]) -> some View {
    HStack(spacing: 8) {
      if tablet {
        tabletChips(projects: projects)
      } else {
        Menu {
          Section { choice(.all, symbol: "bubble.left.and.bubble.right") }
          if !projects.isEmpty {
            Section("Projects") {
              ForEach(projects, id: \.self) { name in choice(.project(name), symbol: "folder") }
            }
          }
          Section { choice(.archived, symbol: "archivebox") }
        } label: {
          FilterChip(title: view.title, prominent: true, showsChevron: true)
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
        .accessibilityLabel("Showing \(view.title)")
        .accessibilityIdentifier("archive-filter")
      }
      if view != .all {
        Button {
          view = .all
        } label: {
          Image(systemName: "xmark").font(.system(size: 12, weight: .semibold))
            .foregroundStyle(.secondary).frame(width: 34, height: 34)
            .background(Color.primary.opacity(0.06), in: Circle())
        }
        .buttonStyle(.plain).accessibilityLabel("Show all chats")
      }
      Spacer(minLength: 0)
    }
  }

  /// iPad keeps the view and the project filter apart: which chats, then a Filter chip that
  /// narrows them to one project.
  @ViewBuilder private func tabletChips(projects: [String]) -> some View {
    let archived = view == .archived
    Menu {
      choice(.all, symbol: "bubble.left.and.bubble.right")
      choice(.archived, symbol: "archivebox")
    } label: {
      FilterChip(title: archived ? "Archived" : "All chats", prominent: true, showsChevron: true)
    }
    .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
    .accessibilityLabel("Showing \(archived ? "Archived" : "All chats")")
    .accessibilityIdentifier("archive-filter")
    if case .project(let name) = view {
      FilterChip(title: name, systemImage: "folder")
    } else if !projects.isEmpty {
      Menu {
        Section("Project") {
          ForEach(projects, id: \.self) { name in choice(.project(name), symbol: "folder") }
        }
      } label: {
        FilterChip(title: "Filter", systemImage: "plus")
      }
      .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
      .accessibilityLabel("Filter by project")
      .accessibilityIdentifier("chats-project-filter")
    }
  }

  /// First connections show progress; later reconnects only surface once they become a problem.
  private var showsConnectionBanner: Bool {
    workspace.connectionIssue
      || (workspace.projects.isEmpty && workspace.cloudChats.isEmpty
        && ["Connecting…", "Synchronizing…", "Offline · reconnecting"].contains(
          workspace.connectionState))
  }

  // MARK: Ask

  private var askField: some View {
    let canSend = !ask.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    return HStack(spacing: 4) {
      Menu {
        Button("New chat", systemImage: "square.and.pencil") { newChat(ask) }
        if workspace.account.signedIn {
          Button("New cloud chat", systemImage: "cloud") { newCloudChat() }
        }
      } label: {
        Image(systemName: "plus").font(.system(size: 17, weight: .medium))
          .foregroundStyle(.primary).frame(width: 34, height: 34)
          .background(Color.primary.opacity(0.07), in: Circle())
          .frame(width: 44, height: 44).contentShape(Rectangle())
      }
      .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
      .accessibilityLabel("New chat options")
      TextField("Ask anything", text: $ask)
        .font(.system(size: 17)).focused($askFocused).submitLabel(.send)
        .onSubmit { submitAsk() }
        .accessibilityIdentifier("ask-anything")
      if canSend {
        Button(action: submitAsk) {
          ComposerSendGlyph(symbol: "arrow.up", enabled: true, busy: false)
        }
        .buttonStyle(.plain).accessibilityLabel("Start chat")
      }
    }
    .padding(.leading, 2).padding(.trailing, canSend ? 2 : 14)
    .frame(minHeight: 48)
    .composerGlass(radius: 24)
    // iPad's ask field runs the width of the list, like the rows above it.
    .frame(maxWidth: tablet ? .infinity : 640).frame(maxWidth: .infinity)
    .padding(.horizontal, tablet ? 24 : 12).padding(.bottom, 6).padding(.top, 4)
  }

  private func submitAsk() {
    let text = ask
    ask = ""
    askFocused = false
    newChat(text)
  }

  // MARK: Empty

  /// A computer or Zuse Cloud is open, so a new chat is one message away.
  private var hasPlaceToWork: Bool {
    workspace.isCloudActive || workspace.connectionState == "Connected"
      || (workspace.connectionState == "Preview · offline" && !workspace.projects.isEmpty)
  }

  @ViewBuilder private var emptyState: some View {
    if view == .archived {
      ZappersEmptyState(
        symbol: "archivebox", title: "No archived chats",
        detail: "Chats you archive show up here, ready to restore.")
    } else if case .project(let name) = view {
      ZappersEmptyState(
        symbol: "folder", title: "No chats in \(name)",
        detail: "Start one from the field below.")
    } else if !showsConnectionBanner && hasPlaceToWork {
      ZappersEmptyState(
        symbol: "bubble.left.and.bubble.right", title: "No chats yet",
        detail:
          "Ask anything below to start one. Chats you start on your computer show up here too."
      )
    } else if !showsConnectionBanner {
      ZappersEmptyState(
        symbol: "bubble.left.and.bubble.right", title: "No chats yet",
        detail: workspace.account.signedIn
          ? "Ask anything below, or connect a computer to see its chats."
          : "Connect a computer or sign in for cloud chats."
      ) {
        VStack(spacing: 10) {
          if !workspace.account.signedIn {
            Button {
              signingIn = true
              Task {
                defer { signingIn = false }
                do { try await workspace.signIn() } catch {
                  workspace.error = error.localizedDescription
                }
              }
            } label: {
              Text(signingIn ? "Signing In…" : "Sign In").frame(maxWidth: 260)
            }
            .brandProminentButton().disabled(signingIn)
          }
          if let computer = workspace.recentComputer,
            workspace.isCloudActive || workspace.rpc == nil
          {
            Button {
              workspace.openRemote(computer)
            } label: {
              Label("Reconnect to \(computer.displayName)", systemImage: "arrow.clockwise")
                .lineLimit(1).frame(maxWidth: 300)
            }
            .buttonStyle(.bordered)
          } else if workspace.savedConnections.isEmpty && workspace.environments.isEmpty {
            Button(action: connect) {
              Label("Add computer", systemImage: "desktopcomputer").frame(maxWidth: 260)
            }
            .buttonStyle(.bordered)
          }
        }
        .controlSize(.large).buttonBorderShape(.capsule)
      }
    }
  }

  // MARK: Cloud

  /// A cloud chat is an ordinary chat; only the dithered cloud says where it runs.
  private func cloudRow(_ chat: CloudChat) -> some View {
    let busy = workspace.cloudRequestsBusy.contains(chat.workspaceId)
    let pending = workspace.pendingCloudActions[chat.workspaceId]
    return Button {
      if let pending {
        requestCloudAction(chat, action: pending)
      } else {
        workspace.openCloud(chat)
      }
    } label: {
      ChatRowLabel(
        title: chat.title.isEmpty ? "Untitled chat" : chat.title,
        meta: [
          chat.repositoryDisplayName.isEmpty ? nil : chat.repositoryDisplayName,
          pending.map { $0.title + " pending" }
            ?? (chat.state == "ready" && !chat.isTransitioning ? "Cloud" : chat.statusLabel),
        ].compactMap { $0 }.joined(separator: " · ")
      ) {
        if busy {
          ProgressView().controlSize(.mini)
        } else {
          DitherCloudIcon()
        }
      }
    }
    .buttonStyle(.plain)
    .disabled(busy)
    .chatsRow()
    .contextMenu { cloudActions(chat) }
    .swipeActions { cloudActions(chat) }
  }
  @ViewBuilder private func cloudActions(_ chat: CloudChat) -> some View {
    if let pending = workspace.pendingCloudActions[chat.workspaceId] {
      Button("Retry " + pending.title.lowercased() + " request", systemImage: "arrow.clockwise") {
        requestCloudAction(chat, action: pending)
      }.disabled(workspace.cloudRequestsBusy.contains(chat.workspaceId))
    } else {
      ForEach(chat.availableActions, id: \.rawValue) { action in
        Button(role: action == .archive ? .destructive : nil) {
          requestCloudAction(chat, action: action)
        } label: {
          Label(action.title, systemImage: action.symbol)
        }
        .disabled(workspace.cloudRequestsBusy.contains(chat.workspaceId))
      }
    }
  }
  private func requestCloudAction(_ chat: CloudChat, action: CloudWorkspaceAction) {
    if action.stopsWork {
      cloudConfirmation = CloudActionSelection(chat: chat, action: action)
    } else {
      Task { await workspace.controlCloud(chat, action: action) }
    }
  }
  private func restoring(_ row: ChatListRow, chat: CloudChat?) -> Bool {
    if let chat { return workspace.cloudRequestsBusy.contains(chat.workspaceId) }
    return workspace.restoringChats.contains(row.chatID)
  }
  /// Restores an archived chat where it lives, then opens it when asked.
  private func restore(_ row: ChatListRow, open: Bool) {
    Task {
      if row.isCloud {
        guard let chat = workspace.cloudChats.first(where: { Self.key($0) == row.id }),
          await workspace.controlCloud(chat, action: .unarchive), open
        else { return }
        view = .all
        // A workspace still coming back from the archive opens once it is ready.
        if let fresh = workspace.cloudChats.first(where: { $0.id == chat.id }), !fresh.isArchived {
          workspace.openCloud(fresh)
        }
      } else {
        guard let session = workspace.archivedSessions.first(where: { $0.id == row.chatID }),
          await workspace.restoreChat(session), open
        else { return }
        view = .all
        selection = workspace.sessions.first { $0.id == session.id } ?? session
      }
    }
  }
}

// MARK: - Rows

/// A chat row: where it runs, its title, then its project and age.
struct ChatRowLabel<Leading: View, Trailing: View>: View {
  let title: String
  var meta: String = ""
  var dimmed = false
  @ViewBuilder let leading: Leading
  @ViewBuilder var trailing: Trailing
  /// iPad's full-width lists: a roomier title kept to a readable measure.
  @Environment(\.tabletLists) private var tablet
  var body: some View {
    HStack(alignment: .firstTextBaseline, spacing: tablet ? 14 : 11) {
      leading.frame(width: 18, height: 18).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 3 }
      VStack(alignment: .leading, spacing: tablet ? 3 : 2) {
        Text(title).font(.system(size: tablet ? 17 : 16))
          .foregroundStyle(
            dimmed ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary.opacity(0.92))
          )
          .lineLimit(1)
          .frame(maxWidth: tablet ? TabletMeasure.line : nil, alignment: .leading)
        if !meta.isEmpty {
          Text(meta).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
        }
      }
      Spacer(minLength: 6)
      trailing
    }
    .padding(.vertical, tablet ? 9 : 7).contentShape(Rectangle())
  }
}

extension ChatRowLabel where Trailing == EmptyView {
  init(
    title: String, meta: String = "", dimmed: Bool = false, @ViewBuilder leading: () -> Leading
  ) {
    self.init(title: title, meta: meta, dimmed: dimmed, leading: leading, trailing: { EmptyView() })
  }
}

private struct SessionListRow: View {
  let workspace: WorkspaceModel
  let session: SessionRow
  let project: String?
  var selected = false
  private var context: String {
    (workspace.operationScope ?? "") + workspace.connectionState
      + workspace.account.generation.uuidString
  }
  private var meta: String {
    let age = workspace.zappers.chats[session.id]?.lastMessageAt.map { CompactTime.label($0) }
    return [project ?? "No project", age].compactMap { $0 }.joined(separator: " · ")
  }
  var body: some View {
    let info = workspace.gitState(for: session)
    // A hidden link keeps split-view selection while omitting the disclosure chevron.
    ZStack {
      NavigationLink(value: session) { EmptyView() }.opacity(0)
      ChatRowLabel(title: session.title.isEmpty ? "New chat" : session.title, meta: meta) {
        ProviderMark(providerID: session.providerId, size: 16).foregroundStyle(.secondary)
      } trailing: {
        if let number = info["number"].int {
          HStack(spacing: 4) {
            GitStatusGlyph(info: info).scaleEffect(0.8)
            Text("#\(number)").font(.system(size: 12)).monospacedDigit()
              .foregroundStyle(.secondary)
          }.accessibilityIdentifier("git-status-\(session.id)")
        }
      }
    }
    .chatsRow(selected: selected)
    .task(id: context) {
      workspace.requestGitState(session)
      while !Task.isCancelled && workspace.connectionState == "Connected" {
        do { try await Task.sleep(for: .seconds(60)) } catch { return }
        workspace.requestGitState(session, refresh: true)
      }
    }
    .contextMenu {
      if let value = info["url"].string, let url = URL(string: value), url.scheme == "https" {
        Link(destination: url) {
          Label("Open pull request", systemImage: "arrow.up.right.square")
        }
      }
      Button("Copy title", systemImage: "doc.on.doc") { NativePlatform.copy(session.title) }
      Button("Archive", systemImage: "archivebox") {
        Task { await workspace.archiveChat(session) }
      }
    }
  }
}

/// An archived chat: quieter, and tapping offers to bring it back rather than opening it.
struct ArchivedChatRow: View {
  let title: String
  let cloud: Bool
  var project: String? = nil
  let busy: Bool
  let restore: (Bool) -> Void
  var body: some View {
    Menu {
      Button("Restore and open", systemImage: "arrow.up.bin") { restore(true) }
      Button("Restore", systemImage: "arrow.uturn.backward") { restore(false) }
    } label: {
      ChatRowLabel(
        title: title,
        meta: [project, cloud ? "Cloud" : nil, "Archived"].compactMap { $0 }.joined(
          separator: " · "), dimmed: true
      ) {
        if busy {
          ProgressView().controlSize(.mini)
        } else if cloud {
          DitherCloudIcon()
        } else {
          Image(systemName: "archivebox").font(.system(size: 14)).foregroundStyle(.secondary)
        }
      }
    }
    .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
    .disabled(busy)
    .accessibilityLabel(title + ", archived" + (cloud ? ", Zuse Cloud" : ""))
    .accessibilityHint("Shows restore options")
  }
}

/// The dithered cloud that marks Zuse Cloud chats, drawn from the shared icon's cells.
struct DitherCloudIcon: View {
  var size: CGFloat = 16
  var body: some View {
    Canvas { context, canvas in
      let cell = canvas.width / CGFloat(DitherCloud.gridSize)
      func path(_ cells: [DitherCloud.Cell]) -> Path {
        var path = Path()
        for item in cells {
          path.addRect(
            CGRect(
              x: CGFloat(item.x) * cell, y: CGFloat(item.y) * cell,
              width: CGFloat(item.width) * cell, height: cell))
        }
        return path
      }
      context.fill(path(DitherCloud.solid), with: .foreground)
      context.opacity = DitherCloud.faintOpacity
      context.fill(path(DitherCloud.faint), with: .foreground)
    }
    .frame(width: size, height: size)
    .foregroundStyle(.secondary)
    .accessibilityElement()
    .accessibilityLabel("Zuse Cloud")
  }
}

struct GitStatusGlyph: View {
  let info: JSONValue
  private var color: Color {
    if info["checks"].string == "failure" || info["mergeable"].string == "conflicting" {
      return .red
    }
    switch info["state"].string {
    case "merged": return .purple
    case "open": return info["isDraft"].bool ? .secondary : .green
    case "closed": return .red
    default: return .secondary
    }
  }
  var body: some View {
    GitBranchShape(pullRequest: info["number"].int != nil, merged: info["state"].string == "merged")
      .stroke(color, style: StrokeStyle(lineWidth: 1.35, lineCap: .round, lineJoin: .round))
      .frame(width: 16, height: 16)
      .accessibilityLabel(
        info["number"].int.map { "Pull request #\($0), " + (info["state"].string ?? "") }
          ?? "Git branch")
  }
}
private struct GitBranchShape: Shape {
  let pullRequest: Bool
  let merged: Bool
  func path(in rect: CGRect) -> Path {
    var p = Path()
    p.addEllipse(in: CGRect(x: 1, y: 1, width: 4, height: 4))
    p.addEllipse(in: CGRect(x: 1, y: 11, width: 4, height: 4))
    p.move(to: CGPoint(x: 3, y: 5))
    p.addLine(to: CGPoint(x: 3, y: 11))
    p.addEllipse(in: CGRect(x: 11, y: pullRequest ? 11 : 1, width: 4, height: 4))
    p.move(to: CGPoint(x: 13, y: pullRequest ? 11 : 5))
    if pullRequest && !merged {
      p.addLine(to: CGPoint(x: 13, y: 5))
      p.addQuadCurve(to: CGPoint(x: 8, y: 2), control: CGPoint(x: 13, y: 2))
    } else {
      p.addQuadCurve(to: CGPoint(x: 3, y: 9), control: CGPoint(x: 13, y: 9))
    }
    return p
  }
}

/// Connecting progress, or why the computer can't be reached, with a retry.
struct ConnectionBanner: View {
  let workspace: WorkspaceModel
  private var connecting: Bool { !workspace.connectionIssue }
  var body: some View {
    HStack(alignment: .top, spacing: 10) {
      Group {
        if connecting {
          ProgressView().controlSize(.small)
        } else {
          Image(systemName: "wifi.exclamationmark").foregroundStyle(.orange)
        }
      }.frame(width: 20, height: 20)
      VStack(alignment: .leading, spacing: 2) {
        Text(
          connecting
            ? workspace.connectedComputer.map { "Connecting to \($0)…" } ?? "Connecting…"
            : "Can’t reach \(workspace.connectedComputer ?? "your computer")"
        ).font(.subheadline.weight(.semibold)).lineLimit(1)
        if let problem = workspace.connectionProblem, !connecting {
          Text(problem).font(.footnote).foregroundStyle(.secondary).lineLimit(3)
            .textSelection(.enabled)
        }
      }
      Spacer(minLength: 4)
      if !connecting {
        Button("Retry") { workspace.reconnect() }.font(.subheadline.weight(.medium))
          .buttonStyle(.borderless).tint(AppTheme.accent)
      }
    }
    .padding(12).frame(maxWidth: .infinity, alignment: .leading)
    .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
  }
}

private struct ChatsNotice: View {
  let text: String
  let icon: String
  var body: some View {
    Label(text, systemImage: icon).font(.footnote).foregroundStyle(.secondary)
      .chatsRow(minHeight: 32)
  }
}

extension View {
  /// Row metrics for the Chats list.
  func chatsRow(minHeight: CGFloat = 44, selected: Bool = false) -> some View {
    frame(minHeight: minHeight)
      .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
      .listRowSeparator(.hidden).listRowBackground(SelectedRowBackground(selected: selected))
  }
}

extension View {
  /// Pins controls under the navigation bar, inside its scroll edge effect where available.
  @ViewBuilder func topBar<Bar: View>(@ViewBuilder _ bar: () -> Bar) -> some View {
    if #available(iOS 26, macOS 26, *) {
      safeAreaBar(edge: .top, spacing: 0, content: bar)
    } else {
      safeAreaInset(edge: .top, spacing: 0) { bar().background(AppTheme.surface) }
    }
  }
}
