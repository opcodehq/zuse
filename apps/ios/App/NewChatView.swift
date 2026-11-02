import SwiftUI
import ZuseCore

struct NewChatView: View {
  let workspace: WorkspaceModel
  var destination: AnyView? = nil
  var prompt: Binding<String> = .constant("")
  var busyChanged: (Bool) -> Void = { _ in }
  var finish: () -> Void = {}
  @State private var project = ""
  @State private var provider = ""
  @State private var model = ""
  @State private var modelOptions: [String: String] = [:]
  @State private var mode = "main"
  @State private var branch = ""
  @State private var worktree = ""
  @State private var runtimeMode = "approval-required"
  @State private var permissionMode = "default"
  @State private var catalog: [ModelProvider] = []
  @State private var branches: [GitBranch] = []
  @State private var worktrees: [JSONValue] = []
  @State private var failure: String?
  @State private var sourceFailure: String?
  @State private var creating = false
  @State private var loadingSources = false
  @State private var scope: String?
  @State private var intent: ChatCreation?
  @State private var uncertainBranch = false
  @State private var showProject = false
  @State private var showOptions = false
  private var selectedProvider: ModelProvider? { catalog.first { $0.id == provider } }
  private var intentKey: String? {
    scope.map { "create-chat:" + ArchiveStore.storageID($0) }
  }
  private var connected: Bool {
    scope == workspace.operationScope && workspace.connectionState == "Connected"
  }
  private var creationProgress: NewChatProgress? {
    guard creating else { return nil }
    let detail: String? =
      switch mode {
      case "fresh": "Creating a new worktree"
      case "branch": "Checking out " + (branches.first { $0.id == branch }?.name ?? "the branch")
      default: nil
      }
    return NewChatProgress(
      title: "Setting up your chat",
      steps: [.init(id: "chat", title: "Starting chat", detail: detail, state: .active)],
      prompt: prompt.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines))
  }
  var body: some View {
    NewChatPage(
      subtitle: workspace.projects.first { $0.id == project }?.name, progress: creationProgress
    ) {
      if let failure { Text(failure).font(.caption).foregroundStyle(.red).padding(.bottom, 8) }
      if let destination { destination }
      Menu {
        Picker("Project", selection: $project) {
          Text("Choose a project").tag("")
          ForEach(workspace.projects) { Text($0.name).tag($0.id) }
        }
        Button("Add a project", systemImage: "folder.badge.plus") { showProject = true }
      } label: {
        SetupSelectorLabel(
          icon: "folder",
          title: workspace.projects.first { $0.id == project }?.name ?? "Choose a project")
      }.disabled(intent != nil || creating)
      Menu {
        Picker("Work in", selection: $mode) {
          Text("Main checkout").tag("main")
          Text("New worktree").tag("fresh")
          Text("Existing branch").tag("branch")
          Text("Existing worktree").tag("existing")
        }
      } label: {
        SetupSelectorLabel(icon: "desktopcomputer", title: workspaceLabel)
      }
      .disabled(intent != nil || creating)
      Menu {
        if mode == "existing" {
          Picker("Worktree", selection: $worktree) {
            Text("Choose a worktree").tag("")
            ForEach(worktrees, id: \.["id"].string) { row in
              Text(row["branch"].string ?? row["name"].string ?? "Worktree").tag(
                row["id"].string ?? "")
            }
          }
        } else {
          Picker("Branch", selection: $branch) {
            Text("Choose a branch").tag("")
            ForEach(branches.filter { !$0.current }) { Text($0.name).tag($0.id) }
          }
        }
      } label: {
        SetupSelectorLabel(icon: "arrow.triangle.branch", title: branchLabel)
      }
      .disabled(
        intent != nil || creating || loadingSources || mode == "main" || mode == "fresh")
      if loadingSources { ProgressView().controlSize(.mini).padding(.leading, 6) }
      if let sourceFailure {
        HStack {
          Text(sourceFailure).lineLimit(2)
          Button("Retry") { Task { await loadSources() } }
        }.font(.caption).foregroundStyle(.secondary)
      }
      if intent != nil {
        HStack {
          Text("A saved chat request is ready to retry.")
          Button("Discard", role: .destructive) {
            do {
              if let intentKey { try Vault.write(nil, key: intentKey) }
              intent = nil
            } catch { failure = error.localizedDescription }
          }
        }.font(.caption).padding(.vertical, 4)
      }
    } composer: {
      NewChatDraftComposer(
        text: prompt, enabled: canCreate, busy: creating, editable: intent == nil,
        options: AnyView(
          Button("Chat options", systemImage: "slider.horizontal.3") { showOptions = true }),
        models: AnyView(modelMenu)
      ) { Task { await create() } }
    }
    .sheet(isPresented: $showProject) { AddProjectView(workspace: workspace) }
    .sheet(isPresented: $showOptions) { options }
    .onChange(of: creating) { _, value in busyChanged(value) }
    .task { await load() }
    .task(id: project) { await loadSources() }
  }
  private var workspaceLabel: String {
    switch mode {
    case "fresh": "New worktree"
    case "branch": "Existing branch"
    case "existing": "Existing worktree"
    default: "Main checkout"
    }
  }
  private var branchLabel: String {
    if mode == "existing" {
      return worktrees.first { $0["id"].string == worktree }?["branch"].string
        ?? "Choose a worktree"
    }
    if mode == "branch" { return branches.first { $0.id == branch }?.name ?? "Choose a branch" }
    return branches.first { $0.current }?.name ?? "Current branch"
  }
  private var modelMenu: some View {
    ModelPickerButton(
      catalog: catalog, providerID: provider, modelID: model, modelOptions: $modelOptions
    ) {
      providerID, modelID in
      provider = providerID
      model = modelID
    }.disabled(intent != nil || creating || catalog.isEmpty)
  }
  private var options: some View {
    NavigationStack {
      Form {
        Picker("Permissions", selection: $runtimeMode) {
          Text("Ask before changes").tag("approval-required")
          Text("Allow edits").tag("auto-accept-edits")
          Text("Allow edits & commands").tag("auto-accept-edits-and-bash")
          Text("Full access").tag("full-access")
        }
        Toggle(
          "Plan mode",
          isOn: Binding(
            get: { permissionMode == "plan" }, set: { permissionMode = $0 ? "plan" : "default" }))
      }.settingsList().disabled(intent != nil).navigationTitle("Chat options")
        .compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .sheetClose) {
            SheetCloseButton { showOptions = false }
          }
        }
    }.nativeSheetSize(width: 400, height: 320).presentationDetents([.medium])
  }
  private var canCreate: Bool {
    connected && !creating && !project.isEmpty && !model.isEmpty && !provider.isEmpty
      && (intent != nil
        || ((mode != "branch" || (!branch.isEmpty && !uncertainBranch))
          && (mode != "existing" || !worktree.isEmpty)))
  }
  private func load() async {
    failure = nil
    catalog = []
    scope = workspace.operationScope
    project = workspace.selectedProject?.id ?? workspace.projects.first?.id ?? ""
    let requestedScope = scope
    do {
      guard let rpc = workspace.rpc else { throw ClientError("Connect to a computer first.") }
      let loaded = try await ModelProvider.load(rpc)
      guard workspace.operationScope == requestedScope, !Task.isCancelled else { return }
      catalog = loaded
      provider = catalog.first?.id ?? ""
      model = selectedProvider?.defaultModel ?? ""
      if let intentKey, let data = try Vault.read(intentKey) {
        intent = try JSONDecoder().decode(ChatCreation.self, from: data)
        if let intent {
          project = intent.projectID
          provider = intent.providerID
          model = intent.model
          runtimeMode = intent.runtimeMode
          permissionMode = intent.permissionMode
          prompt.wrappedValue = intent.initialPrompt ?? ""
          mode = intent.freshWorktree ? "fresh" : intent.worktreeID == nil ? "main" : "existing"
          worktree = intent.worktreeID ?? ""
        }
      }
    } catch {
      if workspace.operationScope == requestedScope && !Task.isCancelled {
        failure = error.localizedDescription
      }
    }
  }
  private func loadSources() async {
    branches = []
    worktrees = []
    if intent == nil { worktree = "" }
    branch = ""
    sourceFailure = nil
    guard !project.isEmpty, let rpc = workspace.rpc else { return }
    let requested = project
    let requestedScope = scope
    loadingSources = true
    defer { if project == requested { loadingSources = false } }
    do {
      if await rpc.canonical {
        let (loadedBranches, loadedWorktrees) = try await CanonicalWorkspace(rpc).sources(
          projectID: requested)
        guard project == requested, scope == requestedScope, !Task.isCancelled else { return }
        branches = loadedBranches
        worktrees = loadedWorktrees
        return
      }
      async let branchRows = rpc.call("git.branches", ["folderId": .string(requested)])
      async let treeRows = rpc.call("worktree.list", ["projectId": .string(requested)])
      let (b, t) = try await (branchRows, treeRows)
      guard project == requested, scope == requestedScope, !Task.isCancelled else { return }
      branches = try b.decode([GitBranch].self)
      worktrees = t.array
    } catch {
      if !Task.isCancelled && project == requested { sourceFailure = error.localizedDescription }
    }
  }
  private func create() async {
    guard canCreate, let rpc = workspace.rpc, let intentKey else { return }
    creating = true
    failure = nil
    defer { creating = false }
    do {
      if intent == nil {
        var treeID = mode == "existing" ? worktree : nil
        if mode == "branch" {
          guard let selected = branches.first(where: { $0.id == branch }) else { return }
          // This legacy mutation has no idempotency key: never replay an uncertain result.
          uncertainBranch = true
          let result: JSONValue
          if await rpc.canonical {
            result = try await CanonicalWorkspace(rpc).branchCheckout(
              projectID: project, branch: selected.name, requestID: UUID().uuidString)
          } else {
            result = try await rpc.call(
              "worktree.create", ["projectId": .string(project), "source": selected.worktreeSource])
          }
          guard connected else { throw CancellationError() }
          guard let id = result["id"].string else {
            throw ClientError(
              "The computer did not return a worktree. Reload worktrees before retrying.")
          }
          treeID = id
          mode = "existing"
          worktree = id
          worktrees.insert(result, at: 0)
          uncertainBranch = false
        }
        let creation = ChatCreation(
          projectID: project, providerID: provider, model: model,
          runtimeMode: runtimeMode, permissionMode: permissionMode,
          freshWorktree: mode == "fresh", worktreeID: treeID,
          initialPrompt: prompt.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines),
          modelOptions: modelOptions)
        try Vault.write(JSONEncoder().encode(creation), key: intentKey)
        intent = creation
      }
      guard let intent else { return }
      try await workspace.createChat(intent)
      try Vault.write(nil, key: intentKey)
      prompt.wrappedValue = ""
      finish()
    } catch {
      failure =
        uncertainBranch
        ? "The branch checkout result is unconfirmed. Reload and choose its existing worktree before creating a chat."
        : error.localizedDescription
    }
  }
}

struct NewChatSetupView: View {
  @Bindable var workspace: WorkspaceModel
  let showsClose: Bool
  let close: () -> Void
  init(
    workspace: WorkspaceModel, initialCloud: Bool = false, showsClose: Bool,
    close: @escaping () -> Void
  ) {
    self.workspace = workspace
    self.showsClose = showsClose
    self.close = close
    _cloud = State(initialValue: initialCloud)
  }
  @State private var cloud = false
  @State private var draft = ""
  @State private var busy = false
  @State private var method: ConnectionMethod?
  @State private var sampleModel = "sample-balanced"
  @State private var sampleModelOptions: [String: String] = [:]
  private var machineLabel: String {
    cloud ? "Cloud" : workspace.connectedComputer ?? "Choose computer"
  }
  private var destinationMenu: some View {
    Menu {
      Section("Run on") {
        Button("Computer", systemImage: "laptopcomputer") { cloud = false }
        Button("Cloud", systemImage: "cloud") { cloud = true }
      }
      if !workspace.savedConnections.isEmpty {
        Section("Saved computers") {
          ForEach(workspace.savedConnections) { connection in
            Button(connection.displayName) {
              cloud = false
              workspace.openRemote(connection)
            }
          }
        }
      }
      if !workspace.environments.isEmpty {
        Section("Remote computers") {
          ForEach(workspace.environments) { environment in
            Button(environment.label ?? "Computer") {
              cloud = false
              workspace.openEnvironment(environment)
            }
          }
        }
      }
      Section {
        Button("Find a nearby computer", systemImage: "wifi") {
          cloud = false
          method = .nearby
        }
        Button("Scan a pairing code", systemImage: "qrcode.viewfinder") {
          cloud = false
          method = .pairing
        }
        Button("Add manually", systemImage: "plus") {
          cloud = false
          method = .manual
        }
      }
    } label: {
      SetupSelectorLabel(icon: cloud ? "cloud" : "laptopcomputer", title: machineLabel)
    }
    .accessibilityIdentifier("new-chat-destination").accessibilityLabel("Run on: " + machineLabel)
    .disabled(busy).menuOrder(.fixed)
  }
  @ViewBuilder private var fallbackModelPicker: some View {
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--native-preview"),
        let catalog = try? WorkspaceModel.previewModelCatalog()
      {
        ModelPickerButton(
          catalog: catalog,
          providerID: catalog.first { $0.models.contains { $0.id == sampleModel } }?.id
            ?? "Preview",
          modelID: sampleModel, sample: true, modelOptions: $sampleModelOptions
        ) { _, model in sampleModel = model }
      } else {
        Text("Choose agent").font(.caption).foregroundStyle(.tertiary)
      }
    #else
      Text("Choose agent").font(.caption).foregroundStyle(.tertiary)
    #endif
  }
  private var sampleProgress: NewChatProgress? {
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--setup-progress-preview") {
        return NewChatProgress(
          title: "Setting up your workspace",
          steps: [
            .init(id: "create", title: "Creating workspace", state: .done),
            .init(
              id: "start", title: "Starting cloud computer", detail: "Provisioning",
              state: .active),
            .init(id: "open", title: "Opening chat", state: .pending),
          ], prompt: "Testing, if this works")
      }
    #endif
    return nil
  }
  var body: some View {
    Group {
      if cloud && workspace.account.signedIn {
        CloudLaunchView(
          workspace: workspace, prompt: $draft, destination: AnyView(destinationMenu),
          busyChanged: { busy = $0 })
      } else if !cloud && workspace.rpc != nil {
        NewChatView(
          workspace: workspace, destination: AnyView(destinationMenu), prompt: $draft,
          busyChanged: { busy = $0 }, finish: close
        )
        .id(workspace.operationScope)
      } else {
        NewChatPage(progress: sampleProgress) {
          destinationMenu
          if cloud {
            Button {
              Task {
                do { try await workspace.signIn() } catch {
                  workspace.error = error.localizedDescription
                }
              }
            } label: {
              SetupSelectorLabel(
                icon: "person.crop.circle", title: "Sign in to choose a repository")
            }
            .buttonStyle(.plain).accessibilityLabel("Sign in to Zuse")
          } else {
            Button {
              method = .nearby
            } label: {
              SetupSelectorLabel(icon: "folder", title: "Connect to choose a project")
            }.buttonStyle(.plain)
          }
          if workspace.loading {
            ProgressView("Connecting…").font(.caption).padding(.vertical, 6).padding(.leading, 6)
          }
          if let problem = workspace.connectionProblem {
            Text(problem).font(.caption).foregroundStyle(.secondary).padding(.leading, 6)
          }
        } composer: {
          NewChatDraftComposer(
            text: $draft, enabled: false, busy: false,
            options: AnyView(
              Menu {
                Button("Find a nearby computer", systemImage: "wifi") { method = .nearby }
                Button("Add manually", systemImage: "plus") { method = .manual }
              } label: {
                Image(systemName: "plus").frame(width: 36, height: 40)
              }),
            models: AnyView(fallbackModelPicker)
          ) {}
        }
      }
    }
    .navigationTitle("New Chat").compactNavigationTitle()
    .navigationBarBackButtonHidden(busy)
    .toolbar {
      if showsClose {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close", systemImage: "xmark", action: close).tint(.primary).disabled(busy)
        }
      }
    }
    .connectionFlow($method, workspace: workspace)
  }
}

/// Shared new-chat page: an empty canvas with setup selectors and the composer at the bottom.
struct NewChatProgress {
  enum StepState { case done, active, pending }
  struct Step: Identifiable {
    let id: String
    let title: String
    var detail: String? = nil
    let state: StepState
  }
  let title: String
  let steps: [Step]
  var prompt: String = ""
  var failure: String? = nil
  var retry: (() -> Void)? = nil
}

/// Shared new-chat page: a prompt canvas with setup selectors and the composer at the bottom.
/// Once a chat is submitted the page shows live setup progress until the chat opens.
struct NewChatPage<Selectors: View, Composer: View>: View {
  var subtitle: String? = nil
  var progress: NewChatProgress? = nil
  @ViewBuilder let selectors: Selectors
  @ViewBuilder let composer: Composer
  var body: some View {
    ZStack {
      AppTheme.surface.ignoresSafeArea()
      if let progress {
        NewChatProgressView(progress: progress).transition(.opacity)
      } else {
        VStack(spacing: 6) {
          Text("What should we work on?").font(.title2.weight(.semibold))
          if let subtitle {
            Text(subtitle).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
          }
        }
        .multilineTextAlignment(.center).padding(.horizontal, 32).padding(.bottom, 80)
        .transition(.opacity)
      }
    }
    .safeAreaInset(edge: .bottom, spacing: 0) {
      Group {
        if let progress {
          if !progress.prompt.isEmpty {
            Text(progress.prompt).font(.body).lineLimit(4)
              .padding(.horizontal, 14).padding(.vertical, 10)
              .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
              .frame(maxWidth: .infinity, alignment: .trailing)
          }
        } else {
          VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) { selectors }.tint(.primary)
            composer.padding(.top, 14)
          }
        }
      }
      .frame(maxWidth: 720).frame(maxWidth: .infinity)
      .padding(.horizontal, 12).padding(.top, 12).padding(.bottom, 8)
    }
    .animation(.snappy(duration: 0.25), value: progress == nil)
  }
}

private struct NewChatProgressView: View {
  let progress: NewChatProgress
  var body: some View {
    VStack(alignment: .leading, spacing: 18) {
      Text(progress.title).font(.title3.weight(.semibold))
      VStack(alignment: .leading, spacing: 14) {
        ForEach(progress.steps) { step in
          HStack(alignment: .firstTextBaseline, spacing: 12) {
            Group {
              switch step.state {
              case .done:
                Image(systemName: "checkmark.circle.fill").foregroundStyle(AppTheme.accent)
              case .active:
                if progress.failure == nil {
                  ProgressView().controlSize(.small)
                } else {
                  Image(systemName: "exclamationmark.circle.fill").foregroundStyle(.orange)
                }
              case .pending:
                Image(systemName: "circle").foregroundStyle(.tertiary)
              }
            }.frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
              Text(step.title).foregroundStyle(step.state == .pending ? .secondary : .primary)
              if let detail = step.detail, step.state == .active {
                Text(detail).font(.footnote).foregroundStyle(.secondary)
              }
            }
          }
          .accessibilityElement(children: .combine)
        }
      }
      if let failure = progress.failure {
        VStack(alignment: .leading, spacing: 10) {
          Text(failure).font(.footnote).foregroundStyle(.secondary).textSelection(.enabled)
          if let retry = progress.retry {
            Button("Try Again", action: retry).buttonStyle(.borderedProminent)
              .tint(AppTheme.accent).buttonBorderShape(.capsule)
          }
        }
      }
    }
    .frame(maxWidth: 340, alignment: .leading).padding(.horizontal, 32).padding(.bottom, 60)
    .animation(.snappy(duration: 0.25), value: progress.steps.map(\.state))
  }
}

struct SetupSelectorLabel: View {
  let icon: String
  let title: String
  var body: some View {
    HStack(spacing: 10) {
      Image(systemName: icon).font(.system(size: 17)).foregroundStyle(.secondary).frame(width: 24)
      Text(title).font(.system(size: 17)).lineLimit(1).foregroundStyle(.primary)
      Image(systemName: "chevron.up.chevron.down").font(.system(size: 11, weight: .semibold))
        .foregroundStyle(.tertiary)
      Spacer(minLength: 0)
    }.frame(minHeight: 44).padding(.horizontal, 6).contentShape(Rectangle())
  }
}

struct NewChatDraftComposer: View {
  @FocusState private var focused: Bool
  @Binding var text: String
  let enabled: Bool
  let busy: Bool
  var editable = true
  let options: AnyView
  let models: AnyView
  let submit: () -> Void
  var body: some View {
    VStack(spacing: 0) {
      TextField("Ask Zuse", text: $text, axis: .vertical)
        .focused($focused).font(.system(size: 17)).lineLimit(2...6).padding(.horizontal, 8)
        .padding(.top, 10)
        .padding(.bottom, 4).disabled(busy || !editable).accessibilityIdentifier("new-chat-prompt")
      HStack(spacing: 6) {
        options.buttonStyle(.plain).font(.system(size: 19)).frame(minWidth: 44, minHeight: 44)
          .foregroundStyle(.secondary).accessibilityLabel("Chat options")
        Spacer(minLength: 4)
        models
        Button {
          focused = false
          submit()
        } label: {
          ComposerSendGlyph(enabled: enabled, busy: busy)
        }.buttonStyle(.plain).disabled(!enabled || busy)
          .accessibilityIdentifier("new-chat-create").accessibilityLabel("Create chat")
      }
    }.padding(8).composerGlass()
  }
}
