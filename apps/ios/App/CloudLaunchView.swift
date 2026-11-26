import SwiftUI
import ZuseCore

/// Starts a cloud chat from server data, as the desktop app does: `/cloud/context` supplies the
/// repositories, agent catalog and connections; capabilities decide the computer and its size.
struct CloudLaunchView: View {
  let workspace: WorkspaceModel
  let prompt: Binding<String>
  let destination: AnyView?
  var busyChanged: (Bool) -> Void
  @Environment(\.scenePhase) private var scenePhase
  private let client: CloudClient
  private let accountGeneration: UUID
  private let accountIdentity: String?
  init(
    workspace: WorkspaceModel, prompt: Binding<String> = .constant(""),
    destination: AnyView? = nil, busyChanged: @escaping (Bool) -> Void = { _ in }
  ) {
    self.prompt = prompt
    self.destination = destination
    self.busyChanged = busyChanged
    self.workspace = workspace
    client = workspace.cloud
    accountGeneration = workspace.account.generation
    accountIdentity = workspace.account.archiveIdentity
  }
  @State private var options: CloudLaunchOptions?
  @State private var projects: [CloudProject] = []
  @State private var catalog: [ModelProvider] = []
  @State private var provider = ""
  @State private var providerProblem: String?
  @State private var project = ""
  @State private var size: String?
  @State private var agent = ""
  @State private var model = ""
  @State private var modelOptions: [String: String] = [:]
  @State private var branch = ""
  @State private var repository = ""
  @State private var repositoryBranch = "main"
  @State private var isPrivate = true
  @State private var busy = false
  @State private var loaded = false
  @State private var failure: String?
  @State private var pendingIntent: CloudIntent?
  @State private var showOptions = false
  @State private var showAgents = false
  @State private var launching = false
  /// The created workspace; the page shows setup progress until its chat opens.
  @State private var launched: String?
  @State private var launchedPrompt = ""
  private var selectedProject: CloudProject? { projects.first { $0.projectId == project } }
  private var selectedAgent: CloudAgent? { CloudAgent(harness: agent) }
  private var agentTitle: String {
    catalog.first { $0.id == agent }?.name ?? selectedAgent?.title ?? "an agent"
  }
  private var connection: CloudAgentConnection? { options?.connection(forHarness: agent) }
  private var needsConnection: Bool {
    options?.requiresConnection == true && !agent.isEmpty && connection == nil
  }
  private var sizeChoices: [CloudSizeOption] {
    options?.sizeChoices(provider: provider) ?? []
  }
  private var resolvedSize: CloudSizeOption? {
    guard let options, case .success(let id) = options.size(provider: provider, preferred: size)
    else { return nil }
    return sizeChoices.first { $0.id == id }
  }
  private var blocker: String? { providerProblem ?? options?.allocationProblem }
  private var locked: Bool { pendingIntent != nil || busy }
  private var launchProgress: NewChatProgress? {
    if launching && launched == nil {
      return NewChatProgress(
        title: "Setting up your workspace",
        steps: [
          .init(id: "create", title: "Creating workspace", state: .active),
          .init(id: "start", title: "Starting cloud computer", state: .pending),
          .init(id: "open", title: "Opening chat", state: .pending),
        ], prompt: prompt.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    guard let launched else { return nil }
    let chat = workspace.cloudChats.first { $0.workspaceId == launched }
    let opening = ["Synchronizing…", "Connected"].contains(workspace.connectionState)
    let started = opening || chat?.state == "ready"
    let blocked = workspace.connectionState == "Connection needs attention"
    return NewChatProgress(
      title: "Setting up your workspace",
      steps: [
        .init(id: "create", title: "Creating workspace", state: .done),
        .init(
          id: "start", title: "Starting cloud computer",
          detail: chat.map { $0.statusLabel } ?? "This can take a minute",
          state: started ? .done : .active),
        .init(id: "open", title: "Opening chat", state: started ? .active : .pending),
      ], prompt: launchedPrompt,
      failure: blocked ? workspace.connectionProblem ?? "The workspace needs attention." : nil,
      retry: blocked ? { workspace.reconnect() } : nil)
  }
  var body: some View {
    NewChatPage(subtitle: selectedProject?.displayName, progress: launchProgress) {
      if let failure { Text(failure).font(.caption).foregroundStyle(.red).padding(.bottom, 6) }
      if let destination { destination }
      repositoryMenu
      if sizeChoices.count > 1 { sizeMenu }
      Button {
        showOptions = true
      } label: {
        SetupSelectorLabel(icon: "arrow.triangle.branch", title: branchTitle)
      }.buttonStyle(.plain).disabled(locked)
      if needsConnection {
        Button {
          showAgents = true
        } label: {
          SetupSelectorLabel(icon: "person.badge.key", title: "Connect \(agentTitle) to start")
        }
        .buttonStyle(.plain).accessibilityIdentifier("cloud-launch-connect-agent")
      }
      if let blocker {
        Label(blocker, systemImage: "exclamationmark.triangle").font(.caption)
          .foregroundStyle(.orange).padding(.vertical, 4).padding(.leading, 6)
      }
      if !loaded && busy {
        ProgressView("Loading Cloud…").font(.caption).padding(.vertical, 6).padding(.leading, 6)
      }
      if pendingIntent != nil {
        HStack {
          Text("A saved workspace request is ready to retry.")
          Button("Discard", role: .destructive) { discardIntent() }
        }.font(.caption).padding(.vertical, 4)
      }
    } composer: {
      NewChatDraftComposer(
        text: prompt,
        enabled: !busy && selectedProject != nil && !provider.isEmpty && !agent.isEmpty
          && !model.isEmpty && blocker == nil,
        busy: busy, editable: pendingIntent == nil,
        options: AnyView(optionsMenu), models: AnyView(modelMenu)
      ) { Task { await launch() } }
    }
    .sheet(isPresented: $showOptions) { optionsSheet }
    .sheet(isPresented: $showAgents, onDismiss: { Task { await load() } }) {
      CloudSettingsView(workspace: workspace, focus: needsConnection ? selectedAgent : nil)
        .nativeSheetSize()
    }
    .onChange(of: project) {
      guard pendingIntent == nil else { return }
      branch = selectedProject?.defaultBranch ?? ""
    }
    .onChange(of: busy) { _, value in busyChanged(value) }
    .task {
      await load()
      #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--cloud-agents-preview") {
          showAgents = true
        }
      #endif
    }
    .task(id: launched) {
      // Keep the lifecycle label fresh while the workspace boots; stop once it is ready.
      while let id = launched, !Task.isCancelled {
        do { try await Task.sleep(for: .seconds(PollSchedule.cloudTransition)) } catch { return }
        if let chat = workspace.cloudChats.first(where: { $0.workspaceId == id }),
          chat.state == "ready", !chat.isTransitioning
        {
          return
        }
        // Returning to the foreground refreshes the catalog through the scene change.
        guard scenePhase == .active else { continue }
        await workspace.refreshCloud(reportErrors: false)
      }
    }
  }

  // MARK: Selectors

  private var branchTitle: String {
    let name = branch.isEmpty ? selectedProject?.defaultBranch ?? "" : branch
    return name.isEmpty ? "Default branch" : name
  }
  private var repositoryMenu: some View {
    Menu {
      Picker("Repository", selection: $project) {
        if selectedProject == nil { Text("Choose a repository").tag("") }
        ForEach(projects) { row in Text(row.displayName).tag(row.projectId) }
      }
      Button("Connect a repository…", systemImage: "plus") { showOptions = true }
    } label: {
      SetupSelectorLabel(
        icon: "folder",
        title: selectedProject?.displayName
          ?? (loaded && projects.isEmpty ? "Connect a repository" : "Choose a repository"))
    }
    .accessibilityIdentifier("cloud-launch-repository").disabled(locked)
  }
  private var sizeMenu: some View {
    Menu {
      ForEach(sizeChoices) { choice in
        Button {
          size = choice.id
        } label: {
          if choice.id == resolvedSize?.id {
            Label(sizeTitle(choice), systemImage: "checkmark")
          } else {
            Text(sizeTitle(choice))
          }
        }.disabled(!choice.available)
      }
    } label: {
      SetupSelectorLabel(icon: "cpu", title: resolvedSize.map(sizeTitle) ?? "Computer size")
    }
    .accessibilityIdentifier("cloud-launch-size").disabled(locked)
  }
  private func sizeTitle(_ choice: CloudSizeOption) -> String {
    // Plan sizes already describe their resources; provider sizes only name themselves.
    var title =
      options?.planSizes.isEmpty == false
      ? choice.label
      : "\(choice.label) · \(choice.vcpus) vCPU · \(choice.memoryMib / 1024) GB"
    if !choice.available {
      title += choice.requiresPlan.map { " · needs \(CloudErrorText.humanize($0))" } ?? ""
    }
    return title
  }
  private var optionsMenu: some View {
    Menu {
      Button("Branch and repository", systemImage: "arrow.triangle.branch") { showOptions = true }
      Button("Cloud agents", systemImage: "person.badge.key") { showAgents = true }
    } label: {
      Image(systemName: "slider.horizontal.3").frame(width: 36, height: 40)
    }
    .accessibilityLabel("Cloud options")
  }
  private var modelMenu: some View {
    ModelPickerButton(
      catalog: catalog, providerID: agent, modelID: model, modelOptions: $modelOptions
    ) { providerID, modelID in
      agent = providerID
      model = modelID
    }.disabled(locked || catalog.isEmpty)
  }
  private var optionsSheet: some View {
    NavigationStack {
      Form {
        Section {
          TextField("Branch", text: $branch).verbatimInput().autocorrectionDisabled()
        } header: {
          Text("Start from")
        } footer: {
          Text("The cloud computer checks out this branch of the repository.")
        }
        Section {
          TextField("Repository URL", text: $repository).verbatimInput().autocorrectionDisabled()
          TextField("Default branch", text: $repositoryBranch).verbatimInput()
            .autocorrectionDisabled()
          Toggle("Private repository", isOn: $isPrivate)
          Button("Connect repository") { Task { await connectRepository() } }
            .disabled(
              busy || repository.trimmingCharacters(in: .whitespaces).isEmpty
                || repositoryBranch.trimmingCharacters(in: .whitespaces).isEmpty)
        } header: {
          Text("Connect a repository")
        } footer: {
          Text("Private repositories need the Zuse GitHub app installed on their owner.")
        }
      }.settingsList().disabled(pendingIntent != nil).navigationTitle("Cloud options")
        .compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .sheetClose) {
            SheetCloseButton { showOptions = false }
          }
        }
    }.nativeSheetSize(width: 420, height: 480)
  }

  // MARK: Loading

  private func load() async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    defer { busy = false }
    do {
      // Capabilities add allocation blockers; context alone is enough when they fail.
      async let contextRequest = client.context()
      async let capabilitiesRequest = try? client.capabilities()
      let (context, capabilities) = try await (contextRequest, capabilitiesRequest)
      guard accountGeneration == workspace.account.generation, !Task.isCancelled else { return }
      apply(CloudLaunchOptions(context: context, capabilities: capabilities))
      if !loaded { try restoreIntent() }
      loaded = true
      failure = nil
    } catch is CancellationError {} catch {
      guard accountGeneration == workspace.account.generation else { return }
      workspace.account.recordAuthenticationFailure(error)
      loaded = true
      failure = error.localizedDescription
    }
  }
  /// Applies server choices, keeping the current picks while they are still offered.
  private func apply(_ value: CloudLaunchOptions) {
    options = value
    projects = value.context.projects.filter { $0.canCreate != false }
    if selectedProject == nil, pendingIntent == nil {
      project = projects.count == 1 ? projects[0].projectId : ""
    }
    switch value.provider() {
    case .success(let id):
      provider = id
      providerProblem = nil
    case .failure(let error):
      provider = ""
      providerProblem = error.message
    }
    let agents = ModelProvider.cloud(
      value.catalog(for: provider) ?? value.context.agentCatalog ?? CloudAgentCatalog(providers: [])
    )
    catalog = agents
    if pendingIntent == nil {
      if !agents.contains(where: { $0.id == agent }) {
        // Prefer an agent that can start right away.
        agent =
          agents.first { !value.requiresConnection || value.connection(forHarness: $0.id) != nil }?
          .id ?? agents.first?.id ?? ""
      }
      let models = agents.first { $0.id == agent }
      if models?.models.contains(where: { $0.id == model }) != true {
        model = models?.defaultModel ?? ""
        modelOptions = [:]
      }
      if let size, !sizeChoices.contains(where: { $0.id == size }) { self.size = nil }
    }
  }
  private var intentKey: String? {
    accountIdentity.map { CloudIntent.key(account: $0, operation: "launch") }
  }
  private func restoreIntent() throws {
    guard let key = intentKey, let pending = try CloudIntent.load(key: key) else { return }
    // A body saved by an older version may not match the current contract; drop it.
    guard let request = CloudWorkspaceCreateRequest(body: pending.body),
      request.agentConnectionId != nil || options?.requiresConnection != true
    else {
      try Vault.write(nil, key: key)
      return
    }
    pendingIntent = pending
    prompt.wrappedValue = request.firstMessage ?? ""
    project = request.projectId
    provider = request.providerId
    size = request.sizeId
    agent = request.agent
    model = request.model
    modelOptions = request.reasoning.map { ["reasoning": $0] } ?? [:]
    branch =
      request.baseRef.hasPrefix("refs/heads/")
      ? String(request.baseRef.dropFirst("refs/heads/".count)) : request.baseRef
  }
  private func discardIntent() {
    do {
      if let key = intentKey { try Vault.write(nil, key: key) }
      pendingIntent = nil
      if let options { apply(options) }
    } catch { failure = error.localizedDescription }
  }

  // MARK: Actions

  private func connectRepository() async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    failure = nil
    defer { busy = false }
    let request = CloudProjectConnectRequest(
      repositoryUrl: repository, defaultBranch: repositoryBranch, isPrivate: isPrivate)
    do {
      guard let account = accountIdentity else { throw ClientError("Sign in first.") }
      let key = CloudIntent.key(account: account, operation: "repository")
      let intent = try CloudIntent.prepare(request.body, key: key)
      let row = try await client.connectProject(body: intent.requestBody())
      try Vault.write(nil, key: key)
      let rows = try await client.projects()
      guard accountGeneration == workspace.account.generation, !Task.isCancelled else { return }
      projects = rows.filter { $0.canCreate != false }
      if !projects.contains(where: { $0.projectId == row.projectId }) { projects.append(row) }
      project = row.projectId
      repository = ""
      showOptions = false
    } catch {
      workspace.account.recordAuthenticationFailure(error)
      failure = error.localizedDescription
    }
  }
  private func makeRequest() throws -> CloudWorkspaceCreateRequest? {
    guard let options, let selectedProject else { return nil }
    var connectionID: String?
    if options.requiresConnection {
      guard let connection else {
        showAgents = true
        return nil
      }
      connectionID = connection.connectionId
    }
    let sizeID = try options.size(provider: provider, preferred: size).get()
    let levels =
      catalog.first { $0.id == agent }?.models.first { $0.id == model }?.reasoning?.options?.map(
        \.id) ?? []
    let reasoning = modelOptions["reasoning"].flatMap { levels.contains($0) ? $0 : nil }
    let text = prompt.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines)
    let name = branch.trimmingCharacters(in: .whitespaces)
    return CloudWorkspaceCreateRequest(
      projectId: selectedProject.projectId, providerId: provider,
      baseRef: CloudWorkspaceCreateRequest.baseRef(
        branch: name.isEmpty ? selectedProject.defaultBranch : name),
      agent: agent, model: model, agentConnectionId: connectionID, sizeId: sizeID,
      reasoning: reasoning, firstMessage: text.isEmpty ? nil : prompt.wrappedValue,
      initialMessageDelivery: options.serverAdmission(provider: provider)
        ? CloudWorkspaceCreateRequest.serverAdmitted : nil)
  }
  private func launch() async {
    guard accountGeneration == workspace.account.generation, !busy else { return }
    failure = nil
    let generation = accountGeneration
    let body: JSONValue
    do {
      if let pendingIntent {
        body = pendingIntent.body
      } else {
        guard let request = try makeRequest() else { return }
        body = request.body
      }
    } catch {
      failure = error.localizedDescription
      return
    }
    busy = true
    launching = true
    defer {
      busy = false
      launching = false
    }
    do {
      guard let key = intentKey else { throw ClientError("Sign in first.") }
      let intent = try pendingIntent ?? CloudIntent.prepare(body, key: key)
      pendingIntent = intent
      let result: CloudWorkspaceCreated
      do {
        result = try await client.createWorkspace(body: intent.requestBody())
      } catch let error as ClientError where Self.rejected(error) {
        // The server refused this request outright; nothing was created, so the choices
        // can change before trying again.
        try Vault.write(nil, key: key)
        pendingIntent = nil
        if error.code == "agent_connection_required" { showAgents = true }
        throw error
      }
      guard workspace.account.generation == generation else { return }
      try Vault.write(nil, key: key)
      pendingIntent = nil
      launchedPrompt = prompt.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines)
      workspace.openCloud(workspaceID: result.workspaceId, sessionID: result.initialSessionId)
      // The page closes when the chat is selected; a refused open returns to the form.
      guard workspace.error == nil else { return }
      launched = result.workspaceId
      prompt.wrappedValue = ""
      await workspace.refreshCloud()
    } catch {
      workspace.account.recordAuthenticationFailure(error)
      failure = error.localizedDescription
    }
  }
  /// Definite refusals: the request was validated and rejected, so no workspace exists.
  private static func rejected(_ error: ClientError) -> Bool {
    guard let code = error.code else { return false }
    return code == "agent_connection_required" || code.hasPrefix("invalid_")
      || [
        "cloud_provider_unavailable", "cloud_size_unavailable", "runtime_agent_unsupported",
        "runtime_permission_import_required", "access-unavailable",
      ].contains(code)
  }
}
