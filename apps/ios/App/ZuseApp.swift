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
  @State private var columnVisibility: NavigationSplitViewVisibility = .all
  @State private var visibilityBeforeNewChat: NavigationSplitViewVisibility = .all
  @State private var showSettings = false
  @State private var showProject = false
  @State private var composerHeight: CGFloat = 80
  @State private var modelCatalog: [ModelProvider] = []
  @State private var branchName: String?
  @FocusState private var composerFocused: Bool
  @State private var selection: SessionRow?
  @State private var preferredColumn: NavigationSplitViewColumn = .sidebar
  /// The Zapper open in the detail column, its pushed pages, and the chat it opened.
  @State private var zapperRoute: String?
  @State private var zapperPath: [ZapperRoute] = []
  @State private var zapperChat: String?
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
  private struct NewChatRoute: Equatable {
    let id = UUID()
    let cloud: Bool
  }
  /// New chat is a page in the detail column: pushed on iPhone, full width elsewhere.
  private func openNewChat(cloud: Bool) {
    newChat = NewChatRoute(cloud: cloud)
    preferredColumn = .detail
    if !isCompact && columnVisibility != .detailOnly {
      visibilityBeforeNewChat = columnVisibility
      columnVisibility = .detailOnly
    }
  }
  private func closeNewChat() {
    newChat = nil
    if !isCompact { columnVisibility = visibilityBeforeNewChat }
  }
  private func openZapper(_ id: String) {
    if newChat != nil { closeNewChat() }
    if zapperRoute != id { zapperPath = [] }
    zapperRoute = id
    // A chat tapped next always reads as a new selection and leaves the Zapper.
    selection = nil
    preferredColumn = .detail
  }
  private func closeZapper() {
    zapperRoute = nil
    zapperPath = []
    zapperChat = nil
    if isCompact { preferredColumn = .sidebar }
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
      .inspector(isPresented: $showDiff) {
        if let session = workspace.selectedSession, let rpc = workspace.rpc {
          DiffView(rpc: rpc, session: session).inspectorColumnWidth(min: 320, ideal: 440, max: 680)
        }
      }
      .sheet(isPresented: $showSettings) { SettingsView(workspace: workspace) }
      .onChange(of: workspace.operationScope) { if zapperRoute != nil { closeZapper() } }
  }
  #if DEBUG
    /// Debug launches: `SIMCTL_CHILD_ZUSE_PAIR_LINK=<invitation> xcrun simctl launch …` pairs
    /// without the camera or a confirmation, then `--zapper-detail-preview` opens the first Zapper's overview
    /// (`--zapper-chat-preview` stops at its conversation).
    private func openZapperPreview(_ arguments: [String]) async {
      if let link = ProcessInfo.processInfo.environment["ZUSE_PAIR_LINK"], !link.isEmpty {
        do { try await workspace.pair(link) } catch { workspace.error = error.localizedDescription }
        for _ in 0..<80 where workspace.zappers.zappers.isEmpty {
          try? await Task.sleep(for: .milliseconds(250))
        }
      }
      guard arguments.contains("--zapper-detail-preview"),
        let first = workspace.zappers.zappers.first
      else { return }
      openZapper(first.id)
      if arguments.contains("--zapper-chat-preview") { return }
      // Let the Zapper's stack appear on its conversation before pushing the overview.
      try? await Task.sleep(for: .milliseconds(400))
      zapperPath = [.overview]
      if arguments.contains("--zapper-issue-preview"),
        let issue = first.issues.first(where: { $0.details.status == .needsYou })
      {
        zapperPath.append(.issue(issue.id))
      } else if arguments.contains("--zapper-document-preview") {
        zapperPath.append(.document("NOTES.md"))
      }
    }
  #endif
  private var interactiveWorkspace: some View {
    navigation
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
        showSettings = true
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
      .onChange(of: workspace.selectedSession) { _, session in
        selection = session
        if session != nil {
          if newChat != nil { closeNewChat() }
          preferredColumn = .detail
        }
      }
      .onChange(of: preferredColumn) { _, column in
        // Popping the pushed page on iPhone returns to the sidebar.
        if column == .sidebar && isCompact {
          newChat = nil
          zapperRoute = nil
          zapperPath = []
        }
      }
      .onChange(of: selection) {
        if let selection, zapperRoute != nil, selection.id != zapperChat { closeZapper() }
        if let selection, selection.id != workspace.selectedSession?.id {
          workspace.selectSession(selection)
        }
      }
      .onChange(of: scenePhase) { _, phase in workspace.sceneChanged(active: phase == .active) }
      .onChange(of: workspace.account.generation) {
        showSettings = false
        if !workspace.account.signedIn && newChat != nil { closeNewChat() }
        connectionMethod = nil
        showProject = false
        showDiff = false
      }
      .onChange(of: workspace.draft) { workspace.persistDraft() }
      .onChange(of: workspace.connectionState) { _, state in
        if state != "Connected" { showDiff = false }
      }
      .task {
        await workspace.refreshCloud()
        workspace.restoreWorkspace()
        if workspace.selectedSession != nil { preferredColumn = .detail }
        #if DEBUG
          let arguments = ProcessInfo.processInfo.arguments
          if arguments.contains("--settings-preview") { showSettings = true }
          if arguments.contains("--new-chat-preview") { openNewChat(cloud: false) }
          if arguments.contains("--pairing-preview") { connectionMethod = .add }
          await openZapperPreview(arguments)
        #endif
      }
      .fileImporter(
        isPresented: $showFilePicker, allowedContentTypes: [.item], onCompletion: importFile
      )
      .onChange(of: photo) { _, value in importPhoto(value) }
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

  private var navigation: some View {
    NavigationSplitView(
      columnVisibility: $columnVisibility, preferredCompactColumn: $preferredColumn
    ) {
      HomeView(
        workspace: workspace, selection: $selection,
        settings: { showSettings = true }, connect: { connectionMethod = .add },
        newChat: { openNewChat(cloud: false) }, newCloudChat: { openNewChat(cloud: true) },
        addProject: { showProject = true }, openZapper: openZapper
      )
      .disabled(workspace.clearingAccount)
      .navigationSplitViewColumnWidth(min: 230, ideal: 280, max: 340)
      .toolbar(removing: newChat == nil ? nil : .sidebarToggle)
    } detail: {
      if let route = newChat {
        NewChatSetupView(
          workspace: workspace, initialCloud: route.cloud, showsClose: !isCompact,
          close: closeNewChat
        ).id(route.id)
      } else if let zapperID = zapperRoute {
        ZapperNavigator(
          workspace: workspace, zapperID: zapperID, path: $zapperPath,
          select: { session in
            zapperChat = session.id
            workspace.selectSession(session)
          },
          close: closeZapper
        ) { session in
          chatScreen(session)
        }
        .id(zapperID)
      } else if let session = workspace.selectedSession {
        chatScreen(session)
      } else {
        ContentUnavailableView {
          Label("Your work, wherever you are", systemImage: "bubble.left.and.bubble.right")
        } description: {
          Text("Open a cloud chat or connect your computer to continue.")
        }
        .overlay {
          if workspace.loading {
            ProgressView().padding().background(.regularMaterial, in: Capsule())
          }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity).background(AppTheme.surface)
      }
    }
    .navigationSplitViewStyle(.balanced)
  }
  /// One chat's transcript and composer, shown from the home list or inside a Zapper.
  @ViewBuilder private func chatScreen(_ session: SessionRow) -> some View {
    VStack(spacing: 0) {
      if workspace.transcript.olderSequence != nil {
        Button(workspace.paging ? "Loading…" : "Load earlier messages") {
          Task { await workspace.loadOlder() }
        }
        .font(.caption).disabled(workspace.paging || !workspace.sessionReady).padding(6)
      }
      TranscriptView(
        messages: workspace.transcript.messages, sessionID: session.id, rpc: workspace.rpc,
        bottomInset: composerHeight,
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
              .init(color: AppTheme.surface.opacity(0), location: 0),
              .init(color: AppTheme.surface.opacity(0.18), location: 0.35),
              .init(color: AppTheme.surface.opacity(0.55), location: 1),
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
    .toolbar {
      ToolbarItem(placement: .principal) {
        ChatHeaderTitle(
          title: session.title.isEmpty ? "Chat" : session.title,
          subtitle: [
            workspace.projects.first { $0.id == session.projectId }?.name, branchName,
          ].compactMap { $0 }.joined(separator: " · "))
      }
      ToolbarItem(placement: .primaryAction) {
        Button("Changes", systemImage: "plusminus") { showDiff = true }
          .tint(.primary).disabled(workspace.connectionState != "Connected")
      }
    }
  }
  private var composerExpanded: Bool {
    composerFocused || !workspace.attachments.isEmpty
      || workspace.transcript.activeTurnID != nil
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
          .padding(.trailing, composerExpanded ? 14 : 160)
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
            modelMenu.frame(maxWidth: 94)
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
  var body: some View {
    VStack(spacing: 1) {
      Text(title).font(.system(size: 17, weight: .semibold)).lineLimit(1)
      if !subtitle.isEmpty {
        Text(subtitle).font(.system(size: 13)).foregroundStyle(.secondary).lineLimit(1)
      }
    }
    .frame(maxWidth: 240)
    .accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
  }
}
