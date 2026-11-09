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
  private var isTablet: Bool {
    #if os(iOS)
      UIDevice.current.userInterfaceIdiom == .pad
    #else
      false
    #endif
  }
  @State private var search = ""
  @State private var collapsed: Set<String> = []
  @State private var showArchivedCloud = false
  @State private var cloudConfirmation: CloudActionSelection?
  @State private var connectMethod: ConnectionMethod?
  @State private var signingIn = false
  private struct CloudActionSelection {
    let chat: CloudChat
    let action: CloudWorkspaceAction
  }

  var body: some View {
    let grouped = Dictionary(
      grouping: workspace.sessions.filter { matches($0.title) }, by: \.projectId)
    let visibleCloud = workspace.cloudChats.filter {
      $0.isArchived == showArchivedCloud && matches($0.title)
    }
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
      if !workspace.cloudChats.isEmpty {
        HStack {
          Text(showArchivedCloud ? "Archived" : "Cloud")
          Spacer()
          Menu {
            Picker("Show", selection: $showArchivedCloud) {
              Text("Active").tag(false)
              Text("Archived").tag(true)
            }
          } label: {
            Image(systemName: "line.3.horizontal.decrease").frame(width: 32, height: 28)
              .contentShape(Rectangle())
          }.accessibilityLabel("Filter cloud chats")
        }
        .font(.system(size: 13, weight: .medium)).foregroundStyle(.secondary)
        .homeRow(leading: 20, minHeight: 32)
        if visibleCloud.isEmpty {
          Text(showArchivedCloud ? "No archived cloud chats" : "No active cloud chats")
            .font(.subheadline).foregroundStyle(.tertiary).homeRow(leading: 24, minHeight: 40)
        }
        ForEach(visibleCloud) { chat in
          Button {
            if let pending = workspace.pendingCloudActions[chat.workspaceId] {
              requestCloudAction(chat, action: pending)
            } else if chat.isArchived {
              requestCloudAction(chat, action: .unarchive)
            } else {
              workspace.openCloud(chat)
            }
          } label: {
            HomeChatLabel(
              title: chat.title.isEmpty ? "Untitled chat" : chat.title,
              detail: workspace.pendingCloudActions[chat.workspaceId].map {
                $0.title + " pending"
              } ?? chat.statusLabel
            ) {
              if workspace.cloudRequestsBusy.contains(chat.workspaceId) {
                ProgressView().controlSize(.mini)
              } else {
                Image(systemName: chat.isArchived ? "archivebox" : "cloud")
                  .font(.system(size: 14)).foregroundStyle(.secondary)
              }
            }
          }
          .buttonStyle(.plain)
          .disabled(workspace.cloudRequestsBusy.contains(chat.workspaceId))
          .homeRow(leading: 24)
          .contextMenu { cloudActions(chat) }
          .swipeActions { cloudActions(chat) }
        }
      }
      ForEach(workspace.projects) { project in
        let chats = grouped[project.id] ?? []
        Button {
          withAnimation(.snappy(duration: 0.2)) {
            if collapsed.contains(project.id) {
              collapsed.remove(project.id)
            } else {
              collapsed.insert(project.id)
            }
          }
        } label: {
          HomeProjectHeader(
            name: project.name, count: chats.count, collapsed: collapsed.contains(project.id))
        }
        .buttonStyle(.plain).homeRow(leading: 16, minHeight: 48)
        .padding(.top, 4)
        .accessibilityLabel(
          "\(project.name), \(chats.count) chats"
            + (collapsed.contains(project.id) ? ", collapsed" : ""))
        if !collapsed.contains(project.id) {
          ForEach(chats) { session in
            SessionListRow(workspace: workspace, session: session)
          }
          if chats.isEmpty && workspace.catalogProblem != nil && search.isEmpty {
            Button("Retry loading chats") {
              workspace.selectedProject = project
              Task { await workspace.loadSessions() }
            }.font(.subheadline).homeRow(leading: 24)
          }
        }
      }
      if workspace.projects.isEmpty && workspace.cloudChats.isEmpty && search.isEmpty
        && !showsConnectionBanner
      {
        emptyState.homeRow(leading: 0).padding(.top, 72)
      }
      if !search.isEmpty && !workspace.sessions.contains(where: { matches($0.title) })
        && visibleCloud.isEmpty
      {
        ContentUnavailableView.search(text: search).homeRow(leading: 0)
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
          Button("Pair with desktop", systemImage: "qrcode.viewfinder", action: connect)
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
          .contextMenu {
            Button("Pair with desktop", systemImage: "qrcode.viewfinder", action: connect)
          }
        }
        HStack(spacing: 8) {
          Image(systemName: "magnifyingglass").font(.system(size: 16)).foregroundStyle(.secondary)
          TextField("Search", text: $search).font(.system(size: 16))
            .autocorrectionDisabled().verbatimInput().accessibilityLabel("Search chats")
          if !search.isEmpty {
            Button("Clear search", systemImage: "xmark.circle.fill") { search = "" }
              .labelStyle(.iconOnly).foregroundStyle(.tertiary)
          }
        }
        .padding(.horizontal, 14).frame(height: 44).floatingGlass(radius: 22)
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
          connectMethod = .pairing
        } label: {
          Label("Scan QR Code", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent)
        .tint(workspace.account.signedIn ? AppTheme.accent : Color.primary.opacity(0.12))
        .foregroundStyle(workspace.account.signedIn ? Color.white : Color.primary)
        HStack(spacing: 24) {
          Button("Find Nearby") { connectMethod = .nearby }
          Button("Add Manually") { connectMethod = .manual }
        }.buttonStyle(.borderless).font(.subheadline).foregroundStyle(.secondary)
      }
      .controlSize(.large).buttonBorderShape(.capsule)
      .frame(maxWidth: 340).padding(.horizontal, 24)
    }.frame(maxWidth: .infinity)
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
      Task {
        if await workspace.controlCloud(chat, action: action), action == .unarchive {
          showArchivedCloud = false
        }
      }
    }
  }
  private func matches(_ title: String) -> Bool {
    search.isEmpty || title.localizedCaseInsensitiveContains(search)
  }
}

struct SettingsView: View {
  @Bindable var workspace: WorkspaceModel
  @Environment(\.dismiss) private var dismiss
  @AppStorage("native.appearance") private var appearance = "system"
  @State private var showCloud = false
  @State private var showCloudPreferences = false
  private var version: String {
    let info = Bundle.main.infoDictionary
    let short = info?["CFBundleShortVersionString"] as? String ?? "1.0"
    let build = info?["CFBundleVersion"] as? String ?? ""
    return build.isEmpty ? short : "\(short) (\(build))"
  }
  var body: some View {
    NavigationStack {
      List {
        Section {
          if workspace.account.signedIn {
            NavigationLink {
              accountSettings
            } label: {
              SettingsRow(
                title: "Account", subtitle: "Cloud chats and remote access",
                icon: "person.crop.circle.fill", brand: true)
            }
          } else {
            Button {
              Task {
                do { try await workspace.signIn() } catch {
                  workspace.error = error.localizedDescription
                }
              }
            } label: {
              SettingsRow(
                title: "Sign in", subtitle: "Cloud chats and remote access",
                icon: "person.crop.circle.fill", brand: true)
            }.accessibilityLabel("Sign in to Zuse")
          }
        }
        Section {
          NavigationLink {
            ComputerConnectionsView(workspace: workspace)
          } label: {
            SettingsRow(
              title: "Computers", icon: "desktopcomputer",
              value: workspace.connectedComputer)
          }
          NavigationLink {
            List {
              Picker("Theme", selection: $appearance) {
                Text("System").tag("system")
                Text("Light").tag("light")
                Text("Dark").tag("dark")
              }.pickerStyle(.inline).labelsHidden()
            }.settingsList().navigationTitle("Appearance").compactNavigationTitle()
          } label: {
            SettingsRow(
              title: "Appearance", icon: "circle.lefthalf.filled", value: appearance.capitalized)
          }
        }
        if workspace.account.signedIn {
          Section {
            NavigationLink {
              CloudInfrastructureView(workspace: workspace)
            } label: {
              SettingsRow(title: "Cloud providers", icon: "cloud.fill")
            }
            NavigationLink {
              RegisteredDevicesView(workspace: workspace)
            } label: {
              SettingsRow(title: "Registered devices", icon: "iphone.gen3")
            }
          }
        }
        Section {
        } footer: {
          Text("Zuse \(version)").frame(maxWidth: .infinity).font(.caption)
            .foregroundStyle(.tertiary)
        }
      }
      .settingsList()
      .navigationTitle("Settings").compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } }
      }
      .disabled(workspace.clearingAccount)
    }.nativeSheetSize(width: 440, height: 520).tint(.primary)
      .sheet(isPresented: $showCloud) { CloudSettingsView(workspace: workspace).nativeSheetSize() }
      .sheet(isPresented: $showCloudPreferences) {
        RuntimeSettingsView(cloud: workspace.cloud).nativeSheetSize()
      }
  }
  private var accountSettings: some View {
    List {
      Section {
        SettingsRow(
          title: "Signed in",
          subtitle: workspace.account.reauthenticationRequired
            ? "Session expired" : "Zuse account", icon: "checkmark.seal.fill", brand: true)
        if workspace.account.reauthenticationRequired {
          Button {
            Task {
              do { try await workspace.signIn() } catch {
                workspace.error = error.localizedDescription
              }
            }
          } label: {
            SettingsRow(title: "Sign in again", icon: "arrow.clockwise")
          }
        }
      }
      Section {
        Button {
          showCloudPreferences = true
        } label: {
          SettingsRow(title: "Cloud preferences", icon: "slider.horizontal.3")
        }
        Button {
          showCloud = true
        } label: {
          SettingsRow(title: "Agent sign-in", icon: "key.fill")
        }
      }
      Section {
        Button {
          Task { await workspace.signOut() }
        } label: {
          SettingsRow(
            title: "Sign out", icon: "rectangle.portrait.and.arrow.right", destructive: true)
        }
      }
    }.settingsList()
      .navigationTitle("Account").compactNavigationTitle()
  }
}

struct ComputerConnectionsView: View {
  @Bindable var workspace: WorkspaceModel
  @State private var method: ConnectionMethod?
  @State private var forgetting: RemoteConnection?
  @State private var showRuntime = false
  var body: some View {
    List {
      Section {
        Button {
          method = .nearby
        } label: {
          SettingsRow(
            title: "Connect to a nearby computer", subtitle: "Find it automatically over Wi-Fi",
            icon: "wifi")
        }
        Button {
          method = .pairing
        } label: {
          SettingsRow(title: "Scan a pairing code", icon: "qrcode.viewfinder")
        }
        Button {
          method = .manual
        } label: {
          SettingsRow(title: "Add manually", icon: "plus")
        }
      } header: {
        Text("Connections")
      } footer: {
        Text("Pairs over your local network. No account needed.")
      }
      if !workspace.savedConnections.isEmpty {
        Section("Saved computers") {
          ForEach(workspace.savedConnections) { connection in
            let current = workspace.connectedComputer == connection.displayName
            Button {
              workspace.openRemote(connection)
            } label: {
              SettingsRow(
                title: connection.displayName,
                subtitle: current ? workspace.connectionState : connection.transportName,
                icon: "laptopcomputer", brand: current)
            }.contextMenu {
              Button("Forget computer", role: .destructive) { forgetting = connection }
            }
            .swipeActions { Button("Forget") { forgetting = connection }.tint(.red) }
          }
        }
      }
      if !workspace.environments.isEmpty {
        Section("Account computers") {
          ForEach(workspace.environments) { environment in
            Button {
              workspace.openEnvironment(environment)
            } label: {
              SettingsRow(
                title: environment.label ?? "Computer", subtitle: "Remote access",
                icon: "desktopcomputer")
            }
          }
        }
      }
      if let name = workspace.connectedComputer {
        Section(name) {
          if let rpc = workspace.rpc {
            Button {
              showRuntime = true
            } label: {
              SettingsRow(title: "Computer settings", icon: "slider.horizontal.3")
            }.disabled(workspace.connectionState != "Connected")
            NavigationLink {
              ComputerAccessView(rpc: rpc)
            } label: {
              SettingsRow(title: "Device access", icon: "ipad.and.iphone")
            }.disabled(workspace.connectionState != "Connected")
          }
          Button {
            workspace.persistDraft(immediate: true)
            workspace.disconnect(clearSaved: true)
          } label: {
            SettingsRow(title: "Disconnect", icon: "xmark", destructive: true)
          }
        }
      }
      Section {
        NavigationLink {
          ServeSettingsView(workspace: workspace)
        } label: {
          SettingsRow(
            title: "Remote network", subtitle: "Zuse Serve and Tailscale", icon: "network")
        }
      }
    }.settingsList()
      .navigationTitle("Computers").compactNavigationTitle()
      .connectionFlow($method, workspace: workspace)
      .sheet(isPresented: $showRuntime) {
        if let rpc = workspace.rpc { RuntimeSettingsView(rpc: rpc).nativeSheetSize() }
      }
      .confirmationDialog(
        "Forget this computer?",
        isPresented: Binding(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } })
      ) {
        Button("Forget computer", role: .destructive) {
          if let connection = forgetting { workspace.forgetComputer(connection) }
          forgetting = nil
        }
      }
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
        Link("Open pull request", destination: url)
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
