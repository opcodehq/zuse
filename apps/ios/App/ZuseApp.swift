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
  }
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
              var payload: [String: JSONValue] = ["folderId": .string(session.projectId)]
              if let worktree = session.worktreeId { payload["worktreeId"] = .string(worktree) }
              let status = try await rpc.call("git.status", payload)
              try Task.checkCancellation()
              branchName = status["branch"].string
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
        if column == .sidebar && isCompact { newChat = nil }
      }
      .onChange(of: selection) {
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
          if arguments.contains("--pairing-preview") { connectionMethod = .pairing }
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
        settings: { showSettings = true }, connect: { connectionMethod = .pairing },
        newChat: { openNewChat(cloud: false) }, newCloudChat: { openNewChat(cloud: true) },
        addProject: { showProject = true }
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
      } else if let session = workspace.selectedSession {
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
  private var composerExpanded: Bool {
    composerFocused || !workspace.attachments.isEmpty
      || workspace.transcript.activeTurnID != nil
  }
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
        TextField("Ask Zuse", text: $workspace.draft, axis: .vertical)
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
        if workspace.transcript.activeTurnID != nil {
          await workspace.stop()
        } else {
          await workspace.send()
        }
      }
    } label: {
      ComposerSendGlyph(
        symbol: workspace.transcript.activeTurnID != nil ? "stop.fill" : "arrow.up",
        enabled: !sendDisabled, busy: workspace.sending)
    }.buttonStyle(.plain)
      .accessibilityLabel(
        workspace.transcript.activeTurnID != nil ? "Stop response" : "Send message"
      )
      .disabled(sendDisabled).keyboardShortcut(.return, modifiers: [.command])
  }
  private var sendDisabled: Bool {
    if workspace.transcript.activeTurnID != nil {
      return workspace.sending || workspace.connectionState != "Connected"
    }
    return !workspace.canSend || workspace.sending
      || (workspace.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        && workspace.attachments.isEmpty)

  }
}

enum ConnectionMethod: String, Identifiable {
  case nearby, pairing, manual
  var id: String { rawValue }
  var title: String {
    switch self {
    case .nearby: "Nearby computers"
    case .pairing: "Pair a computer"
    case .manual: "Add computer"
    }
  }
}

struct ConnectionView: View {
  let workspace: WorkspaceModel
  var method: ConnectionMethod = .nearby
  @Environment(\.dismiss) private var dismiss
  @State private var address = ""
  @State private var token = ""
  @State private var link = ""
  @State private var pairing = false
  @State private var pairingTask: Task<Void, Never>?
  @State private var pairRequest = UUID()
  @State private var nearby = NearbyBrowser()
  @State private var nearbySelection: NearbyBrowser.Computer?
  @State private var nearbyCode = ""
  @State private var error: String?
  var body: some View {
    NavigationStack {
      Form {
        if method == .nearby {
          Section("Nearby computers") {
            ForEach(nearby.computers) { computer in
              Button(computer.name, systemImage: "laptopcomputer") { nearbySelection = computer }
                .disabled(pairing)
            }
            if nearby.computers.isEmpty {
              Text(nearby.error ?? "Looking for computers on your network…").font(.caption)
                .foregroundStyle(.secondary)
            }
            if let computer = nearbySelection {
              Text("Pair with " + computer.name).font(.subheadline)
              TextField("Pairing code from your computer", text: $nearbyCode)
                .verbatimInput().autocorrectionDisabled().frame(height: 28)
              Button("Pair nearby computer") {
                var url = URLComponents(url: computer.url, resolvingAgainstBaseURL: false)
                var fragment = URLComponents()
                fragment.queryItems = [URLQueryItem(name: "pair", value: nearbyCode)]
                url?.percentEncodedFragment = fragment.percentEncodedQuery
                if let value = url?.url?.absoluteString {
                  link = value
                  pair(pin: computer.pin)
                }
              }.disabled(pairing || nearbyCode.isEmpty)
            }
          }
          .disabled(pairing)
        }
        if method == .pairing {
          Section("Pair with your computer") {
            TextField("Paste a Zuse pairing link", text: $link).verbatimInput()
              .autocorrectionDisabled().frame(height: 28)
            Button(pairing ? "Pairing…" : "Pair") { pair() }.disabled(link.isEmpty || pairing)
          }
        }
        if method == .manual {
          Section {
            TextField("wss://your-computer.example/rpc", text: $address).verbatimInput()
              .autocorrectionDisabled()
            SecureField("Connection token", text: $token).frame(height: 28)
            Button("Connect") {
              workspace.openRemote(
                RemoteConnection(
                  address: address.trimmingCharacters(in: .whitespacesAndNewlines), token: token))
              dismiss()
            }.disabled(address.isEmpty || pairing)
          } header: {
            Text("Manual connection")
          } footer: {
            Text(
              "Use a secure remote address or a private local address. Credentials stay in this device’s Keychain."
            )
          }
        }
        if let error { Text(error).foregroundStyle(.red) }
      }.navigationTitle(method.title).compactNavigationTitle()
        .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
    }.task { if method == .nearby { nearby.start() } }.onDisappear {
      nearby.stop()
      pairRequest = UUID()
      pairingTask?.cancel()
      pairingTask = nil
      pairing = false
      token = ""
      link = ""
      nearbyCode = ""
    }
  }
  private func pair(pin: String? = nil) {
    guard !pairing else { return }
    let submittedLink = link
    let request = UUID()
    pairRequest = request
    pairing = true
    error = nil
    pairingTask = Task {
      defer {
        if pairRequest == request {
          pairing = false
          pairingTask = nil
        }
      }
      do {
        try await workspace.pair(submittedLink, certificatePin: pin)
        guard pairRequest == request, !Task.isCancelled else { return }
        dismiss()
      } catch is CancellationError {} catch {
        if pairRequest == request { self.error = error.localizedDescription }
      }
    }
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
