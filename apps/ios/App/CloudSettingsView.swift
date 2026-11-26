import SwiftUI
import ZuseCore

/// Agent connections cloud chats run with: list, sign in, add a credential, disconnect.
struct CloudSettingsView: View {
  let workspace: WorkspaceModel
  /// The agent a launch is waiting on; the page says so at the top.
  let focus: CloudAgent?
  /// Runs after a connection changes, so the caller can reload what depends on it.
  let changed: () -> Void
  private let client: CloudClient
  private let accountGeneration: UUID
  private let accountIdentity: String?
  init(workspace: WorkspaceModel, focus: CloudAgent? = nil, changed: @escaping () -> Void = {}) {
    self.workspace = workspace
    self.focus = focus
    self.changed = changed
    client = workspace.cloud
    accountGeneration = workspace.account.generation
    accountIdentity = workspace.account.archiveIdentity
  }
  @Environment(\.dismiss) private var dismiss
  @Environment(\.openURL) private var openURL
  @Environment(\.scenePhase) private var scenePhase
  @State private var connections: [CloudAgentConnection] = []
  @State private var loaded = false
  @State private var listRequest = UUID()
  @State private var operation: CloudLoginOperation?
  @State private var loginRevision = UUID()
  @State private var loginPollOperation = ""
  @State private var loginPollStarted = ContinuousClock.now
  @State private var busy = false
  @State private var failure: String?
  @State private var notice: String?
  @State private var entry: CredentialEntry?
  @State private var disconnecting: CloudAgentConnection?
  private var signingIn: Bool { operation?.isPending == true }
  private var operationKey: String? {
    accountIdentity.map { "cloud-agent-login-" + ArchiveStore.storageID($0) }
  }
  var body: some View {
    NavigationStack {
      Form {
        if let focus, !connections.contains(where: { $0.agent == focus.rawValue && $0.isConnected })
        {
          Section {
            Label(
              "Connect \(focus.title) to start this cloud chat. Your draft is kept.",
              systemImage: "info.circle"
            ).font(.subheadline)
          }
        }
        if let failure {
          Section {
            Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
              .font(.subheadline)
          }
        }
        if let notice, failure == nil {
          Section {
            Label(notice, systemImage: "checkmark.circle").foregroundStyle(.secondary)
              .font(.subheadline)
          }
        }
        if let operation, operation.isPending { signInSection(operation) }
        ForEach(CloudAgent.allCases) { agent in agentSection(agent) }
        Section {
        } footer: {
          Text(
            "Cloud agents run with these connections. They're shared with your other Zuse apps, and disconnecting removes the credential from Zuse Cloud."
          )
        }
      }
      .settingsList().navigationTitle("Cloud agents").compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } }
        ToolbarItem(placement: .primaryAction) {
          if busy || !loaded {
            ProgressView().controlSize(.small)
          } else {
            Button("Refresh", systemImage: "arrow.clockwise") { Task { await refresh() } }
          }
        }
      }
      .refreshable { await refresh() }
      .task { await load() }
      .task(
        id: (operation?.operationId ?? "") + loginRevision.uuidString
          + (scenePhase == .active ? "" : "-background")
      ) { await poll() }
      .sheet(item: $entry) { entry in
        CredentialSheet(entry: entry) { secret in try await configure(entry, secret: secret) }
          .nativeSheetSize(width: 420, height: 360)
      }
      .confirmationDialog(
        "Disconnect this agent?",
        isPresented: Binding(
          get: { disconnecting != nil }, set: { if !$0 { disconnecting = nil } }),
        titleVisibility: .visible
      ) {
        if let connection = disconnecting {
          Button("Disconnect", role: .destructive) {
            disconnecting = nil
            Task { await disconnect(connection) }
          }
        }
      } message: {
        Text(
          "Cloud chats can't use this connection anymore, on any of your Zuse apps. Running chats keep going until they stop."
        )
      }
    }
  }

  // MARK: Sections

  @ViewBuilder private func agentSection(_ agent: CloudAgent) -> some View {
    let rows = connections.filter { $0.agent == agent.rawValue }
    Section {
      if rows.isEmpty && loaded {
        Text("Not connected").foregroundStyle(.secondary)
      }
      ForEach(rows) { connection in
        ConnectionRow(connection: connection, agent: agent) {
          reconnect(connection, agent: agent)
        } disconnect: {
          disconnecting = connection
        }
        .disabled(busy || signingIn)
      }
      ForEach(agent.methods, id: \.self) { method in
        Button {
          add(agent, method: method)
        } label: {
          Label(addTitle(method, agent: agent), systemImage: addSymbol(method))
        }
        .disabled(busy || signingIn)
        .accessibilityIdentifier("cloud-agent-\(agent.rawValue)-\(method.rawValue)")
      }
    } header: {
      HStack(spacing: 6) {
        ProviderMark(providerID: agent.rawValue, size: 13)
        Text(agent.title)
      }
    } footer: {
      if agent == .claude {
        Text("Use a setup token from claude setup-token to run on your Claude subscription.")
      }
    }
  }

  private func signInSection(_ operation: CloudLoginOperation) -> some View {
    Section {
      if let code = operation.verificationCode {
        HStack {
          Text(code).font(.system(.title3, design: .monospaced).weight(.semibold))
            .textSelection(.enabled)
          Spacer()
          Button("Copy", systemImage: "doc.on.doc") { NativePlatform.copy(code) }
            .labelStyle(.iconOnly).buttonStyle(.borderless)
            .accessibilityLabel("Copy sign-in code")
        }
      }
      if let page = operation.verificationPage {
        Button("Open sign-in page", systemImage: "arrow.up.right.square") { openURL(page) }
      }
      HStack(spacing: 8) {
        ProgressView().controlSize(.small)
        Text(
          operation.state == "queued"
            ? "Preparing sign-in…"
            : operation.verificationCode == nil
              ? "Waiting for sign-in…" : "Enter the code on the sign-in page…"
        ).foregroundStyle(.secondary)
      }
      Button("Cancel sign-in", role: .destructive) { Task { await cancel() } }.disabled(busy)
    } header: {
      Text("Sign in to " + (operation.cloudAgent?.title ?? "agent"))
    }
  }

  private func addTitle(_ method: CloudConnectionMethod, agent: CloudAgent) -> String {
    switch method {
    case .deviceCode: return "Sign in with your \(agent.title) account"
    case .setupToken: return "Add setup token"
    case .apiKey: return "Add API key"
    }
  }
  private func addSymbol(_ method: CloudConnectionMethod) -> String {
    switch method {
    case .deviceCode: return "person.crop.circle.badge.checkmark"
    case .setupToken: return "ticket"
    case .apiKey: return "key"
    }
  }

  // MARK: Actions

  private func add(_ agent: CloudAgent, method: CloudConnectionMethod) {
    if method == .deviceCode {
      Task { await start(agent) }
    } else {
      entry = CredentialEntry(agent: agent, method: method, connectionID: nil)
    }
  }
  private func reconnect(_ connection: CloudAgentConnection, agent: CloudAgent) {
    let method = connection.connectionMethod ?? .apiKey
    if method == .deviceCode {
      Task { await start(agent, connectionID: connection.connectionId) }
    } else {
      entry = CredentialEntry(agent: agent, method: method, connectionID: connection.connectionId)
    }
  }
  private func report(_ error: Error) {
    workspace.account.recordAuthenticationFailure(error)
    failure = error.localizedDescription
  }
  private func current(_ generation: UUID) -> Bool {
    generation == workspace.account.generation && !Task.isCancelled
  }
  private func accept(_ result: CloudLoginOperation) throws {
    guard let key = operationKey else { return }
    operation = result
    if result.isPending {
      try Vault.write(JSONEncoder().encode(result), key: key)
    } else {
      try Vault.write(nil, key: key)
    }
  }
  private func load() async {
    guard accountGeneration == workspace.account.generation else { return }
    let generation = accountGeneration
    await refresh()
    guard current(generation), let key = operationKey else { return }
    do {
      guard let saved = try Vault.read(key) else { return }
      let pending = try JSONDecoder().decode(CloudLoginOperation.self, from: saved)
      do {
        let result = try await client.agentLogin(operationID: pending.operationId)
        guard current(generation) else { return }
        try accept(result)
      } catch let error as ClientError where error.code == "access-unavailable" {
        // The operation is gone or no longer ours; forget it.
        try Vault.write(nil, key: key)
      }
    } catch is CancellationError {} catch {
      if current(generation) { report(error) }
    }
  }
  private func refresh() async {
    guard accountGeneration == workspace.account.generation else { return }
    let generation = accountGeneration
    let request = UUID()
    listRequest = request
    do {
      let result = try await client.agentConnections()
      guard current(generation), listRequest == request else { return }
      connections = result.connections.filter { $0.state != "revoked" }
      loaded = true
    } catch is CancellationError {} catch {
      if current(generation), listRequest == request {
        loaded = true
        report(error)
      }
    }
  }
  private func start(_ agent: CloudAgent, connectionID: String? = nil) async {
    guard accountGeneration == workspace.account.generation, !busy, !signingIn else { return }
    busy = true
    failure = nil
    notice = nil
    defer { busy = false }
    let generation = accountGeneration
    do {
      let result = try await client.startAgentLogin(
        CloudStartLoginRequest(
          agent: agent, idempotencyKey: UUID().uuidString, connectionId: connectionID))
      guard current(generation) else { return }
      try accept(result)
      if !result.isPending { await finish(result) }
    } catch is CancellationError {} catch {
      if current(generation) { report(error) }
    }
  }
  private func poll() async {
    guard let id = operation?.operationId, operation?.isPending == true else { return }
    // Polling pauses in the background; returning (often from the browser) checks right away.
    guard scenePhase == .active else { return }
    var immediate = loginPollOperation == id
    if !immediate {
      loginPollOperation = id
      loginPollStarted = .now
    }
    let generation = accountGeneration
    let revision = loginRevision
    var failures = 0
    do {
      while operation?.operationId == id, operation?.isPending == true {
        let elapsed = Double(loginPollStarted.duration(to: .now).components.seconds)
        let delay =
          failures == 0
          ? PollSchedule.loginDelay(elapsed: elapsed)
          : PollSchedule.backoff(attempt: failures, initial: 3, maximum: 30)
        if !immediate { try await Task.sleep(for: .seconds(delay)) }
        immediate = false
        guard current(generation), loginRevision == revision, operation?.operationId == id else {
          return
        }
        if busy { continue }
        do {
          let result = try await client.agentLogin(operationID: id)
          guard current(generation), loginRevision == revision, operation?.operationId == id
          else { return }
          try accept(result)
          failures = 0
          if !result.isPending { await finish(result) }
        } catch is CancellationError {
          return
        } catch {
          guard current(generation), loginRevision == revision else { return }
          report(error)
          if (error as? ClientError)?.requiresUserAction == true {
            if let key = operationKey { try? Vault.write(nil, key: key) }
            operation = nil
            return
          }
          failures += 1
        }
      }
    } catch is CancellationError {} catch {
      if current(generation) { report(error) }
    }
  }
  private func finish(_ result: CloudLoginOperation) async {
    if let message = result.outcomeMessage {
      failure = message
    } else {
      failure = nil
      notice = (result.cloudAgent?.title ?? "Agent") + " is connected."
    }
    await refresh()
    changed()
  }
  private func configure(_ entry: CredentialEntry, secret: String) async throws {
    guard accountGeneration == workspace.account.generation else { throw CancellationError() }
    let generation = accountGeneration
    busy = true
    failure = nil
    notice = nil
    defer { busy = false }
    let connection: CloudAgentConnection
    do {
      connection = try await client.configureAgent(
        CloudConfigureAgentRequest(
          agent: entry.agent, method: entry.method, secret: secret,
          idempotencyKey: UUID().uuidString, connectionId: entry.connectionID))
    } catch {
      workspace.account.recordAuthenticationFailure(error)
      throw error
    }
    guard current(generation) else { throw CancellationError() }
    if connection.state == "error" {
      throw ClientError(
        connection.errorCode.map(CloudErrorText.message(for:))
          ?? "That credential didn't work. Check it and try again.")
    }
    notice =
      connection.isConnected
      ? "\(entry.agent.title) is connected."
      : "\(entry.agent.title) is checking the new credential."
    await refresh()
    changed()
  }
  private func cancel() async {
    guard accountGeneration == workspace.account.generation, !busy, let id = operation?.operationId
    else { return }
    let generation = accountGeneration
    loginRevision = UUID()
    busy = true
    defer { busy = false }
    do {
      let result = try await client.cancelAgentLogin(operationID: id)
      guard current(generation), operation?.operationId == id else { return }
      try accept(result)
      failure = nil
      await refresh()
    } catch let error as ClientError where error.code == "access-unavailable" {
      if let key = operationKey { try? Vault.write(nil, key: key) }
      operation = nil
      await refresh()
    } catch { if current(generation) { report(error) } }
  }
  private func disconnect(_ connection: CloudAgentConnection) async {
    guard accountGeneration == workspace.account.generation, !busy, !signingIn else { return }
    let generation = accountGeneration
    busy = true
    failure = nil
    notice = nil
    defer { busy = false }
    do {
      try await client.disconnectAgent(connectionID: connection.connectionId)
      guard current(generation) else { return }
      connections.removeAll { $0.connectionId == connection.connectionId }
      await refresh()
      changed()
    } catch { if current(generation) { report(error) } }
  }
}

/// A credential to add, or to replace on an existing connection.
private struct CredentialEntry: Identifiable {
  let agent: CloudAgent
  let method: CloudConnectionMethod
  let connectionID: String?
  var id: String { agent.rawValue + method.rawValue + (connectionID ?? "") }
}

private struct ConnectionRow: View {
  let connection: CloudAgentConnection
  let agent: CloudAgent
  let reconnect: () -> Void
  let disconnect: () -> Void
  private var tint: Color {
    if connection.isConnected { return .green }
    if connection.needsAttention { return .orange }
    return .secondary
  }
  private var detail: String {
    var parts = [connection.connectionMethod?.title ?? connection.method]
    if connection.state == "error", let code = connection.errorCode {
      parts.append(CloudErrorText.message(for: code))
    } else {
      parts.append(connection.stateLabel)
    }
    return parts.joined(separator: " · ")
  }
  var body: some View {
    HStack(spacing: 12) {
      Circle().fill(tint).frame(width: 8, height: 8).accessibilityHidden(true)
      VStack(alignment: .leading, spacing: 2) {
        Text(connection.accountLabel ?? "\(agent.title) connection").lineLimit(1)
        Text(detail).font(.footnote).foregroundStyle(.secondary).lineLimit(2)
      }
      Spacer(minLength: 8)
      if connection.state == "pending" { ProgressView().controlSize(.small) }
      Menu {
        if !connection.isConnected || connection.connectionMethod != .deviceCode {
          Button(
            connection.connectionMethod == .deviceCode ? "Sign in again" : "Replace credential",
            systemImage: "arrow.clockwise", action: reconnect)
        }
        Button("Disconnect", systemImage: "xmark.circle", role: .destructive, action: disconnect)
      } label: {
        Image(systemName: "ellipsis").frame(width: 32, height: 32).contentShape(Rectangle())
      }
      .buttonStyle(.borderless).foregroundStyle(.secondary)
      .accessibilityLabel("Manage \(connection.accountLabel ?? agent.title)")
    }
    .accessibilityElement(children: .combine)
  }
}

private struct CredentialSheet: View {
  let entry: CredentialEntry
  let submit: (String) async throws -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var secret = ""
  @State private var saving = false
  @State private var failure: String?
  @FocusState private var focused: Bool
  private var trimmed: String { secret.trimmingCharacters(in: .whitespacesAndNewlines) }
  private var valid: Bool { trimmed.count >= 8 && !trimmed.contains(where: \.isNewline) }
  private var fieldTitle: String { entry.method == .setupToken ? "Setup token" : "API key" }
  var body: some View {
    NavigationStack {
      Form {
        Section {
          SecureField(fieldTitle, text: $secret).verbatimInput().autocorrectionDisabled()
            .focused($focused).submitLabel(.done).onSubmit { save() }
        } footer: {
          Text(hint)
        }
        if let failure {
          Section {
            Label(failure, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
              .font(.subheadline)
          }
        }
      }
      .settingsList()
      .navigationTitle((entry.connectionID == nil ? "Connect " : "Update ") + entry.agent.title)
      .compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Cancel") { dismiss() }.disabled(saving)
        }
        ToolbarItem(placement: .confirmationAction) {
          if saving {
            ProgressView().controlSize(.small)
          } else {
            Button("Connect") { save() }.disabled(!valid)
          }
        }
      }
      .interactiveDismissDisabled(saving)
      .onAppear { focused = true }
      .onDisappear { secret = "" }
    }
  }
  private var hint: String {
    switch entry.method {
    case .setupToken:
      return
        "On a computer signed in to Claude, run claude setup-token and paste the token here. It's sent over HTTPS to Zuse Cloud and isn't stored on this device."
    default:
      return
        "Paste an API key for \(entry.agent.title). It's sent over HTTPS to Zuse Cloud and isn't stored on this device."
    }
  }
  private func save() {
    guard valid, !saving else { return }
    saving = true
    failure = nil
    Task {
      defer { saving = false }
      do {
        try await submit(trimmed)
        secret = ""
        dismiss()
      } catch is CancellationError {} catch { failure = error.localizedDescription }
    }
  }
}
