import SwiftUI
import ZuseCore

struct HomeView: View {
  @Bindable var workspace: WorkspaceModel
  @Binding var selection: SessionRow?
  let settings: () -> Void
  let connect: () -> Void
  let newChat: () -> Void
  let newCloudChat: () -> Void
  var addProject: () -> Void = {}
  var openZapper: (String) -> Void = { _ in }
  private var isTablet: Bool {
    #if os(iOS)
      UIDevice.current.userInterfaceIdiom == .pad
    #else
      false
    #endif
  }
  @State private var search = ""
  @State private var collapsed: Set<String> = []
  @State private var archiveFilter: ArchiveFilter = .active
  @State private var cloudConfirmation: CloudActionSelection?
  @State private var connectMethod: ConnectionMethod?
  @State private var showComputers = false
  @State private var signingIn = false
  private struct CloudActionSelection {
    let chat: CloudChat
    let action: CloudWorkspaceAction
  }
  /// The open computer's place in the chat list; the home list shows one computer at a time.
  private static let computerPlace = "computer"
  private static func key(_ session: SessionRow) -> String { computerPlace + "|" + session.id }
  private static func key(_ chat: CloudChat) -> String { ChatList.cloudPlace + "|" + chat.id }

  /// Every chat the list can show: the open computer's, active and archived, then Zuse Cloud's.
  private var chatRows: [ChatListRow] {
    var rows: [ChatListRow] = []
    // A cloud workspace's own catalog is reached through its cloud chat row.
    if !workspace.isCloudActive {
      let label = workspace.connectedComputer ?? "Computer"
      for project in workspace.projects {
        let name = project.id == CanonicalWorkspace.projectless ? nil : project.name
        for (sessions, archived) in [
          (workspace.sessions, false), (workspace.archivedSessions, true),
        ] {
          // Zapper conversations are listed under their Zapper instead.
          for session in sessions
          where session.projectId == project.id && !workspace.zappers.keepsChat(session.id) {
            rows.append(
              ChatListRow(
                id: Self.key(session), chatID: session.id, placeKey: Self.computerPlace,
                placeLabel: label, projectKey: project.id, projectName: name,
                title: session.title.isEmpty ? "New chat" : session.title, archived: archived))
          }
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
  /// Chat groups, plus the open computer's projects that have no active chats yet.
  private func listGroups(_ rows: [ChatListRow]) -> [ChatListGroup] {
    var groups = ChatList.groups(rows, query: search, archive: archiveFilter)
    guard archiveFilter == .active, search.isEmpty, !workspace.isCloudActive else { return groups }
    for project in workspace.projects where project.id != CanonicalWorkspace.projectless {
      let id = Self.computerPlace + "|" + project.id
      guard
        !groups.contains(where: {
          $0.id == id || $0.title.lowercased() == project.name.lowercased()
        })
      else { continue }
      groups.append(ChatListGroup(id: id, title: project.name, place: nil, rows: []))
    }
    return groups
  }

  var body: some View {
    let groups = listGroups(chatRows)
    let sessionsByKey = Dictionary(
      (workspace.sessions + workspace.archivedSessions).map { (Self.key($0), $0) },
      uniquingKeysWith: { first, _ in first })
    let cloudByKey = Dictionary(
      workspace.cloudChats.map { (Self.key($0), $0) }, uniquingKeysWith: { first, _ in first })
    List(selection: $selection) {
      if showsConnectionBanner {
        ConnectionBanner(workspace: workspace).homeRow(leading: 4, minHeight: 0)
          .padding(.bottom, 8)
      }
      if let problem = workspace.catalogProblem {
        HomeNotice(text: problem, icon: "arrow.clockwise")
      }
      if let problem = workspace.cloudCatalogProblem {
        HomeNotice(text: problem, icon: "cloud.bolt")
      }
      ZapperHomeSection(
        workspace: workspace, search: search, filter: archiveFilter, open: openZapper)
      ForEach(groups) { group in
        let isCollapsed = collapsed.contains(group.id)
        Button {
          withAnimation(.snappy(duration: 0.2)) {
            if isCollapsed {
              collapsed.remove(group.id)
            } else {
              collapsed.insert(group.id)
            }
          }
        } label: {
          HomeProjectHeader(
            name: group.title, place: group.place, count: group.rows.count,
            collapsed: isCollapsed)
        }
        .buttonStyle(.plain).homeRow(leading: 16, minHeight: 48)
        .padding(.top, 4)
        .accessibilityLabel(
          "\(group.title), \(group.rows.count) chats" + (isCollapsed ? ", collapsed" : ""))
        if !isCollapsed {
          ForEach(group.rows) { row in
            if row.archived {
              ArchivedChatRow(
                title: row.title, cloud: row.isCloud, tagged: archiveFilter == .all,
                busy: restoring(row, chat: cloudByKey[row.id])
              ) { open in
                restore(row, open: open)
              }
              .homeRow(leading: 24)
            } else if row.isCloud, let chat = cloudByKey[row.id] {
              cloudRow(chat)
            } else if let session = sessionsByKey[row.id] {
              SessionListRow(workspace: workspace, session: session)
            }
          }
          if group.rows.isEmpty && workspace.catalogProblem != nil && search.isEmpty,
            let project = workspace.projects.first(where: {
              Self.computerPlace + "|" + $0.id == group.id
            })
          {
            Button("Retry loading chats") {
              workspace.selectedProject = project
              Task { await workspace.loadSessions() }
            }.font(.subheadline).homeRow(leading: 24)
          }
        }
      }
      if groups.isEmpty {
        if !search.isEmpty {
          ContentUnavailableView.search(text: search).homeRow(leading: 0)
        } else if archiveFilter == .archived {
          ContentUnavailableView {
            Label("No archived chats", systemImage: "archivebox")
          } description: {
            Text("Chats you archive show up here, ready to restore.")
          }.homeRow(leading: 0).padding(.top, 48)
        } else if workspace.projects.isEmpty && workspace.cloudChats.isEmpty
          && !showsConnectionBanner
        {
          emptyState.homeRow(leading: 0).padding(.top, 72)
        }
      }
    }
    .listStyle(.plain)
    .environment(\.defaultMinListRowHeight, 32)
    .contentMargins(.top, 4, for: .scrollContent)
    .contentMargins(.horizontal, isTablet ? 8 : 12, for: .scrollContent)
    .scrollContentBackground(.hidden)
    .background(AppTheme.surface)
    .navigationTitle("Zuse")
    .compactNavigationTitle()
    .toolbar {
      ToolbarItem(placement: .principal) { BrandTitle() }
      if !isTablet {
        ToolbarItemGroup(placement: .primaryAction) {
          ComputersButton(workspace: workspace) { showComputers = true }
          Button("Open settings", systemImage: "gearshape.fill", action: settings)
        }
      }
    }
    .tint(.primary)
    .safeAreaInset(edge: .bottom) {
      HStack(spacing: 10) {
        if isTablet {
          Button(action: settings) {
            Image(systemName: "gearshape.fill").font(.system(size: 16))
              .frame(width: 44, height: 44).contentShape(Circle()).floatingGlass(radius: 22)
          }
          .buttonStyle(.plain).accessibilityLabel("Open settings")
          ComputersButton(workspace: workspace) { showComputers = true }
            .font(.system(size: 16)).frame(width: 44, height: 44).contentShape(Circle())
            .floatingGlass(radius: 22).buttonStyle(.plain)
        }
        HStack(spacing: 8) {
          Image(systemName: "magnifyingglass").font(.system(size: 16)).foregroundStyle(.secondary)
          TextField("Search", text: $search).font(.system(size: 16))
            .autocorrectionDisabled().verbatimInput().accessibilityLabel("Search chats")
          if !search.isEmpty {
            Button("Clear search", systemImage: "xmark.circle.fill") { search = "" }
              .labelStyle(.iconOnly).foregroundStyle(.tertiary)
          }
          ArchiveFilterMenu(filter: $archiveFilter)
        }
        .padding(.leading, 14).padding(.trailing, 6).frame(height: 44).floatingGlass(radius: 22)
        Button(action: newChat) {
          Image(systemName: "square.and.pencil").font(.system(size: 18, weight: .medium))
            .frame(width: 44, height: 44).contentShape(Circle()).floatingGlass(radius: 22)
        }.buttonStyle(.plain).accessibilityLabel("New chat")
      }
      .frame(maxWidth: 560).frame(maxWidth: .infinity).padding(.horizontal, 16)
      .padding(.vertical, 8).foregroundStyle(.primary)
    }
    .task(id: workspace.account.generation) {
      var delay = 5.0
      while !Task.isCancelled && workspace.account.signedIn {
        do { try await Task.sleep(for: .seconds(delay)) } catch { return }
        if workspace.shouldPollCloudCatalog {
          await workspace.refreshCloud(reportErrors: false)
          delay = workspace.cloudCatalogProblem == nil ? 5 : min(30, delay * 2)
        }
      }
    }
    .connectionFlow($connectMethod, workspace: workspace)
    .sheet(isPresented: $showComputers) { ComputersSheet(workspace: workspace) }
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
  /// First connections show progress; later reconnects only surface once they become a problem.
  private var showsConnectionBanner: Bool {
    workspace.connectionIssue
      || (workspace.projects.isEmpty && workspace.cloudChats.isEmpty
        && ["Connecting…", "Synchronizing…", "Offline · reconnecting"].contains(
          workspace.connectionState))
  }
  private var emptyState: some View {
    VStack(spacing: 28) {
      VStack(spacing: 8) {
        Image(systemName: "bubble.left.and.bubble.right.fill").font(.system(size: 40))
          .foregroundStyle(.tertiary).padding(.bottom, 6)
        Text("No chats yet").font(.title3.weight(.semibold))
        Text(
          workspace.account.signedIn
            ? "Start a cloud chat or connect a computer." : "Sign in or connect a computer."
        ).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
      }
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
            Text(signingIn ? "Signing In…" : "Sign In").frame(maxWidth: .infinity)
          }
          .buttonStyle(.borderedProminent).tint(AppTheme.accent).disabled(signingIn)
        }
        Button {
          connectMethod = .add
        } label: {
          Label("Add computer", systemImage: "desktopcomputer").frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .tint(workspace.account.signedIn ? AppTheme.accent : Color.primary.opacity(0.12))
        .foregroundStyle(workspace.account.signedIn ? Color.white : Color.primary)
      }
      .controlSize(.large).buttonBorderShape(.capsule)
      .frame(maxWidth: 340).padding(.horizontal, 24)
    }.frame(maxWidth: .infinity)
  }
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
      HomeChatLabel(
        title: chat.title.isEmpty ? "Untitled chat" : chat.title,
        detail: pending.map { $0.title + " pending" }
          ?? (chat.state == "ready" && !chat.isTransitioning ? nil : chat.statusLabel)
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
    .homeRow(leading: 24)
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
        archiveFilter = .active
        // A workspace still coming back from the archive opens once it is ready.
        if let fresh = workspace.cloudChats.first(where: { $0.id == chat.id }), !fresh.isArchived {
          workspace.openCloud(fresh)
        }
      } else {
        guard let session = workspace.archivedSessions.first(where: { $0.id == row.chatID }),
          await workspace.restoreChat(session), open
        else { return }
        archiveFilter = .active
        selection = workspace.sessions.first { $0.id == session.id } ?? session
      }
    }
  }
}

/// Active, archived or every chat, from the trailing edge of the search field.
private struct ArchiveFilterMenu: View {
  @Binding var filter: ArchiveFilter
  var body: some View {
    Menu {
      Picker("Show", selection: $filter) {
        ForEach(ArchiveFilter.allCases, id: \.self) { option in
          Label(option.label, systemImage: option.symbol).tag(option)
        }
      }
    } label: {
      HStack(spacing: 5) {
        if filter != .active {
          Text(filter.label).font(.system(size: 13, weight: .medium)).lineLimit(1)
            .foregroundStyle(.primary)
        }
        Image(systemName: "line.3.horizontal.decrease").font(.system(size: 15, weight: .medium))
          .foregroundStyle(.secondary)
      }
      .padding(.horizontal, 8).frame(minWidth: 32, minHeight: 32)
      .background(
        filter == .active ? Color.clear : Color.primary.opacity(0.08), in: Capsule()
      )
      .contentShape(Capsule())
    }
    .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
    .accessibilityLabel(filter.accessibilityLabel)
    .accessibilityHint("Choose which chats to show")
    .accessibilityIdentifier("archive-filter")
  }
}

extension ArchiveFilter {
  fileprivate var symbol: String {
    switch self {
    case .active: return "bubble.left.and.bubble.right"
    case .archived: return "archivebox"
    case .all: return "tray.full"
    }
  }
}

/// An archived chat: quieter, and tapping offers to bring it back rather than opening it.
private struct ArchivedChatRow: View {
  let title: String
  let cloud: Bool
  let tagged: Bool
  let busy: Bool
  let restore: (Bool) -> Void
  var body: some View {
    Menu {
      Button("Restore and open", systemImage: "arrow.up.bin") { restore(true) }
      Button("Restore", systemImage: "arrow.uturn.backward") { restore(false) }
    } label: {
      HStack(spacing: 10) {
        Group {
          if busy {
            ProgressView().controlSize(.mini)
          } else {
            Image(systemName: "archivebox").font(.system(size: 14)).foregroundStyle(.secondary)
          }
        }.frame(width: 20, height: 20)
        if cloud { DitherCloudIcon() }
        Text(title).font(.system(size: 16)).foregroundStyle(.secondary).lineLimit(1)
        Spacer(minLength: 6)
        if tagged {
          Text("Archived").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
            .padding(.horizontal, 6).padding(.vertical, 1)
            .background(
              Color.primary.opacity(0.08), in: RoundedRectangle(cornerRadius: 6, style: .continuous)
            )
        }
      }
      .frame(minHeight: 44).contentShape(Rectangle())
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

private struct SessionListRow: View {
  let workspace: WorkspaceModel
  let session: SessionRow
  private var context: String {
    (workspace.operationScope ?? "") + workspace.connectionState
      + workspace.account.generation.uuidString
  }
  var body: some View {
    let info = workspace.gitState(for: session)
    // A hidden link keeps split-view selection while omitting the disclosure chevron.
    ZStack {
      NavigationLink(value: session) { EmptyView() }.opacity(0)
      HomeChatLabel(title: session.title.isEmpty ? "New chat" : session.title) {
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
    .homeRow(leading: 24)
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
    }
  }
}

private struct HomeChatLabel<Leading: View, Trailing: View>: View {
  let title: String
  var detail: String? = nil
  @ViewBuilder let leading: Leading
  @ViewBuilder var trailing: Trailing
  var body: some View {
    HStack(spacing: 10) {
      leading.frame(width: 20, height: 20)
      Text(title).font(.system(size: 16)).foregroundStyle(.primary.opacity(0.92)).lineLimit(1)
      Spacer(minLength: 6)
      if let detail {
        Text(detail).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1)
      }
      trailing
    }.frame(minHeight: 44).contentShape(Rectangle())
  }
}

extension HomeChatLabel where Trailing == EmptyView {
  init(title: String, detail: String? = nil, @ViewBuilder leading: () -> Leading) {
    self.init(title: title, detail: detail, leading: leading, trailing: { EmptyView() })
  }
}

private struct HomeProjectHeader: View {
  let name: String
  var place: String? = nil
  let count: Int
  let collapsed: Bool
  var body: some View {
    HStack(spacing: 10) {
      Text(String(name.trimmingCharacters(in: .whitespaces).prefix(1)).uppercased())
        .font(.system(size: 14, weight: .bold)).foregroundStyle(AppTheme.accent)
        .frame(width: 28, height: 28)
        .background(
          Color.primary.opacity(0.07), in: RoundedRectangle(cornerRadius: 6, style: .continuous)
        )
        .overlay {
          RoundedRectangle(cornerRadius: 6, style: .continuous)
            .strokeBorder(Color.primary.opacity(0.1), lineWidth: 0.5)
        }
      Text(name).font(.system(size: 16, weight: .bold)).lineLimit(1)
      if let place {
        Text(place).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
      }
      Spacer(minLength: 6)
      Text("\(count)").font(.system(size: 13)).monospacedDigit().foregroundStyle(.secondary)
      Image(systemName: collapsed ? "chevron.right" : "chevron.down")
        .font(.system(size: 12, weight: .semibold)).foregroundStyle(.tertiary).frame(width: 16)
    }.contentShape(Rectangle())
  }
}

private struct ConnectionBanner: View {
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
          Text(problem).font(.footnote).foregroundStyle(.secondary).lineLimit(4)
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

private struct HomeNotice: View {
  let text: String
  let icon: String
  var body: some View {
    Label(text, systemImage: icon).font(.footnote).foregroundStyle(.secondary)
      .homeRow(leading: 20, minHeight: 32)
  }
}

extension View {
  fileprivate func homeRow(leading: CGFloat, minHeight: CGFloat = 44) -> some View {
    frame(minHeight: minHeight)
      .listRowInsets(EdgeInsets(top: 0, leading: leading, bottom: 0, trailing: 12))
      .listRowSeparator(.hidden).listRowBackground(Color.clear)
  }
}
