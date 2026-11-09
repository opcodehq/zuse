import CryptoKit
import Network
import SwiftUI
import ZuseCore

@MainActor @Observable final class WorkspaceModel {
  let account = Account()
  var cloudChats: [CloudChat] = []
  private(set) var cloudRequestsBusy = Set<String>()
  private(set) var pendingCloudActions: [String: CloudWorkspaceAction] = [:]
  var cloudCatalogProblem: String?
  var shouldPollCloudCatalog: Bool {
    foreground && networkAvailable && !account.reauthenticationRequired
      && cloudChats.contains { $0.isTransitioning }
  }
  var environments: [RemoteEnvironment] = []
  private(set) var savedConnections: [RemoteConnection] = []
  private(set) var connectedComputer: String?
  private(set) var gitStates: [String: JSONValue] = [:]
  private var gitQueue: [(String, SessionRow)] = []
  private var gitQueued = Set<String>()
  private var gitWorker: Task<Void, Never>?
  var projects: [ProjectRow] = []
  var sessions: [SessionRow] = []
  var selectedProject: ProjectRow?
  var selectedSession: SessionRow?
  var transcript = Transcript()
  var error: String?
  var connectionState = "Not connected" {
    didSet { if connectionState != oldValue { updateConnectionIssue() } }
  }
  /// Set only when a failure outlasts automatic retries or needs the user; brief reconnects stay quiet.
  private(set) var connectionIssue = false
  private var connectionIssueTask: Task<Void, Never>?
  private func updateConnectionIssue() {
    switch connectionState {
    case "Connected", "Not connected", "Preview · offline":
      connectionIssueTask?.cancel()
      connectionIssueTask = nil
      connectionIssue = false
    case "Connection needs attention":
      connectionIssueTask?.cancel()
      connectionIssueTask = nil
      connectionIssue = true
    case "Offline · reconnecting":
      guard connectionIssueTask == nil, !connectionIssue else { return }
      connectionIssueTask = Task { [weak self] in
        try? await Task.sleep(for: .seconds(8))
        guard !Task.isCancelled else { return }
        self?.connectionIssue = true
      }
    default:
      // Connecting and synchronizing keep any grace period that is already running.
      break
    }
  }
  var connectionProblem: String?
  var loading = false
  var clearingAccount = false
  private var sessionActivity = SessionActivity()
  var sessionReady: Bool { sessionActivity.isReady }
  var composerEditable: Bool { sessionReady && !enqueueing }
  var sessionSelection: UUID { sessionActivity.generation }
  var sessionRestorationError: String? { sessionActivity.restorationError }
  var sending: Bool { sessionActivity.isActive(.send) }
  private var enqueueing: Bool { sessionActivity.isActive(.enqueue) }
  private var respondingInteractions = Set<String>()
  var paging: Bool { sessionActivity.isActive(.page) }
  var draft = ""
  private(set) var rpc: RPCClient?
  private var connectionTask: Task<Void, Never>?
  private var timelineTask: Task<Void, Never>?
  private var selection = UUID()
  private var source: Source?
  var attachments: [JSONValue] = []
  var uploading: Bool { sessionActivity.isActive(.upload) }
  var outbox: [PendingMessage] = []
  private var archive: ArchiveStore?
  private var archives: [String: ArchiveStore] = [:]
  private var catalogRequest = UUID()
  private var sessionCatalogTask: Task<Void, Never>?
  var catalogProblem: String?
  private var hydratedCatalog: String?
  private var draftTask: Task<Void, Never>?
  private var checkpointTask: Task<Void, Never>?
  private var checkpointRevision = UUID()
  private var recoveryTask: Task<Void, Never>?
  private var heartbeatTask: Task<Void, Never>?
  private var runtimeExpiryTask: Task<Void, Never>?
  private var canonicalRuntimeRow: JSONValue = .null
  private var hydrationTask: Task<Void, Never>?
  private var sessionGeneration: UUID { sessionActivity.generation }
  private var recoveryBackoff = RetryBackoff()
  private var healthySince: ContinuousClock.Instant?
  private var foreground = true
  private var networkAvailable = true
  private let network = NWPathMonitor()
  private var drafts: [String: String] = [:]
  enum Source {
    case cloud(String)
    case environment(String)
    case remote(RemoteConnection)
  }

  func restoreWorkspace() {
    guard source == nil, !ProcessInfo.processInfo.arguments.contains("--native-preview"),
      !ProcessInfo.processInfo.arguments.contains("--rpc-fixture")
    else { return }
    do {
      guard let data = try Vault.read("last-workspace") else { return }
      let saved = try JSONDecoder().decode(WorkspaceLocation.self, from: data)
      switch saved.kind {
      case .computer:
        if let connection = savedConnections.first(where: { $0.id == saved.id }) {
          openRemote(connection)
        }
      case .cloud, .environment:
        guard account.signedIn, account.archiveIdentity == saved.account else { return }
        startConnection(
          saved.kind == .cloud ? .cloud(saved.id) : .environment(saved.id), sessionID: nil,
          wakePausedCloud: false)
      }
    } catch { self.error = error.localizedDescription }
  }

  init() {
    do {
      if ProcessInfo.processInfo.arguments.contains("--native-preview")
        || ProcessInfo.processInfo.arguments.contains("--rpc-fixture")
      {
        savedConnections = []
      } else if let data = try Vault.read("computers") {
        savedConnections = try JSONDecoder().decode([RemoteConnection].self, from: data)
      } else if let data = try Vault.read("remote") {
        savedConnections = [try JSONDecoder().decode(RemoteConnection.self, from: data)]
      }
    } catch { self.error = error.localizedDescription }
    network.pathUpdateHandler = { [weak self] path in
      let available = path.status == .satisfied
      Task { @MainActor in
        guard let self else { return }
        let recovered = available && !self.networkAvailable
        self.networkAvailable = available
        if recovered && self.foreground && self.source != nil { self.reconnect() }
      }
    }
    network.start(queue: DispatchQueue(label: "sh.zuse.native.network"))
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--rpc-fixture"),
        let address = ProcessInfo.processInfo.environment["ZUSE_IOS_FIXTURE_URL"],
        let url = URL(string: address), url.host == "127.0.0.1"
      {
        openRemote(RemoteConnection(address: address, token: ""))
      }
      if ProcessInfo.processInfo.arguments.contains("--native-preview") {
        do { try loadPreview() } catch { self.error = error.localizedDescription }
      }
    #endif
  }
  var cloud: CloudClient {
    let generation = account.generation
    return CloudClient(baseURL: account.apiURL) { [account] in
      try await account.token(expectedGeneration: generation)
    }
  }
  func refreshCloud(reportErrors: Bool = true) async {
    guard account.signedIn else {
      cloudChats = []
      pendingCloudActions = [:]
      cloudRequestsBusy = []
      cloudCatalogProblem = nil
      return
    }
    let request = UUID()
    catalogRequest = request
    let accountURL = account.apiURL
    let accountGeneration = account.generation
    let client = cloud
    var catalog: ArchiveStore?
    do {
      let store = try makeArchive(for: .cloud("__catalog"))
      catalog = store
      if hydratedCatalog != account.archiveIdentity {
        let cached = try await store.load("catalog").metadata
        guard account.generation == accountGeneration, catalogRequest == request else { return }
        if let cached {
          cloudChats = (try? cached["chats"].decode([CloudChat].self)) ?? []
          environments = (try? cached["environments"].decode([RemoteEnvironment].self)) ?? []
        }
        hydratedCatalog = account.archiveIdentity
      }
    } catch {
      if reportErrors {
        self.error = error.localizedDescription
      } else {
        cloudCatalogProblem = error.localizedDescription
      }
    }
    guard account.generation == accountGeneration, catalogRequest == request, !Task.isCancelled
    else { return }
    let chatsTask = Task { try await client.chats(scope: .all) }
    let computersTask = Task { try await client.environments() }
    let (chatsResult, computersResult) = await (chatsTask.result, computersTask.result)
    guard account.signedIn, account.generation == accountGeneration, account.apiURL == accountURL,
      catalogRequest == request
    else { return }
    switch chatsResult {
    case .success(let rows):
      cloudChats = rows
      cloudCatalogProblem = nil
    case .failure(let failure):
      account.recordAuthenticationFailure(failure)
      if reportErrors {
        error = failure.localizedDescription
      } else {
        cloudCatalogProblem = failure.localizedDescription
      }
    }
    switch computersResult {
    case .success(let rows): environments = rows
    case .failure(let failure):
      account.recordAuthenticationFailure(failure)
      if reportErrors {
        error = failure.localizedDescription
      } else {
        cloudCatalogProblem = failure.localizedDescription
      }
    }
    do {
      var pending: [String: CloudWorkspaceAction] = [:]
      for id in Set(cloudChats.map(\.workspaceId)) {
        if let key = cloudActionIntentKey(id), let intent = try CloudIntent.load(key: key),
          let value = intent.body["action"].string,
          let action = CloudWorkspaceAction(rawValue: value)
        {
          pending[id] = action
        }
      }
      pendingCloudActions = pending
    } catch {
      if reportErrors {
        self.error = error.localizedDescription
      } else {
        cloudCatalogProblem = error.localizedDescription
      }
    }
    if let catalog {
      do {
        let chats = try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(cloudChats))
        let computers = try JSONDecoder().decode(
          JSONValue.self, from: JSONEncoder().encode(environments))
        try await catalog.saveMetadata(
          .object(["chats": chats, "environments": computers]), sessionID: "catalog")
      } catch {
        if catalogRequest == request {
          if reportErrors {
            self.error = error.localizedDescription
          } else {
            cloudCatalogProblem = error.localizedDescription
          }
        }
      }
    }
  }

  private func cloudActionIntentKey(_ id: String) -> String? {
    account.archiveIdentity.map { CloudIntent.key(account: $0, operation: "lifecycle:" + id) }
  }
  @discardableResult func controlCloud(_ chat: CloudChat, action: CloudWorkspaceAction) async
    -> Bool
  {
    guard let key = cloudActionIntentKey(chat.workspaceId) else {
      error = "Sign in to manage cloud workspaces."
      return false
    }
    guard cloudRequestsBusy.insert(chat.workspaceId).inserted else { return false }
    let generation = account.generation
    defer { if account.generation == generation { cloudRequestsBusy.remove(chat.workspaceId) } }
    do {
      let body: JSONValue = .object([
        "workspaceId": .string(chat.workspaceId), "action": .string(action.rawValue),
      ])
      if let previous = try CloudIntent.load(key: key), previous.body != body {
        if let name = previous.body["action"].string,
          let previousAction = CloudWorkspaceAction(rawValue: name)
        {
          pendingCloudActions[chat.workspaceId] = previousAction
        }
        throw ClientError(
          "A previous cloud action is unconfirmed. Retry it before choosing another action.")
      }
      let intent = try CloudIntent.prepare(body, key: key)
      pendingCloudActions[chat.workspaceId] = action
      if action.stopsWork, case .cloud(let activeID) = source, activeID == chat.workspaceId {
        if composerEditable, let archive, let session = selectedSession {
          try await archive.saveDraft(draft, sessionID: session.id)
        }
        guard account.generation == generation else { return false }
        if case .cloud(let currentID) = source, currentID == chat.workspaceId {
          try Vault.write(nil, key: "last-workspace")
          disconnect()
        }
      }
      _ = try await cloud.control(
        workspaceID: chat.workspaceId, action: action, commandID: intent.id)
      guard account.generation == generation else { return false }
      try Vault.write(nil, key: key)
      pendingCloudActions.removeValue(forKey: chat.workspaceId)
      await refreshCloud()
      guard account.generation == generation else { return false }
      if action == .resume { openCloud(chat) }
      return true
    } catch {
      if account.generation == generation {
        account.recordAuthenticationFailure(error)
        if (error as? ClientError)?.requiresUserAction == true {
          do {
            try Vault.write(nil, key: key)
            pendingCloudActions.removeValue(forKey: chat.workspaceId)
          } catch {
            self.error = error.localizedDescription
            return false
          }
        }
        self.error = error.localizedDescription
      }
      return false
    }
  }
  func resumeCloud(_ chat: CloudChat) async { _ = await controlCloud(chat, action: .resume) }
  func openCloud(workspaceID: String, sessionID: String) {
    do {
      guard let key = cloudActionIntentKey(workspaceID) else {
        throw ClientError("Sign in to open cloud chats.")
      }
      guard try CloudIntent.load(key: key) == nil else {
        throw ClientError("Retry this workspace's pending cloud action before opening it.")
      }
      guard cloudChats.first(where: { $0.workspaceId == workspaceID })?.isArchived != true else {
        throw ClientError("Restore this workspace from Archived before opening it.")
      }
      startConnection(.cloud(workspaceID), sessionID: sessionID)
    } catch { self.error = error.localizedDescription }
  }
  func openCloud(_ chat: CloudChat) {
    openCloud(workspaceID: chat.workspaceId, sessionID: chat.sessionID)
  }
  func openEnvironment(_ environment: RemoteEnvironment) {
    startConnection(.environment(environment.id), sessionID: nil)
  }
  private func deviceProof() throws -> DeviceProof {
    let key = try Vault.read("dpop-key") ?? DeviceProof.generateKey()
    try Vault.write(key, key: "dpop-key")
    return try DeviceProof(rawKey: key)
  }
  func openRemote(_ connection: RemoteConnection) {
    do { startConnection(.remote(try connection.validated()), sessionID: nil) } catch {
      self.error = error.localizedDescription
    }
  }
  func forgetComputer(_ connection: RemoteConnection) {
    do {
      // Clear restoration credentials before removing the visible catalog entry.
      if let data = try Vault.read("last-workspace") {
        let location = try? JSONDecoder().decode(WorkspaceLocation.self, from: data)
        if location == nil || (location?.kind == .computer && location?.id == connection.id) {
          try Vault.write(nil, key: "last-workspace")
        }
      }
      if let data = try Vault.read("remote") {
        let legacy = try? JSONDecoder().decode(RemoteConnection.self, from: data)
        if legacy == nil || legacy?.id == connection.id { try Vault.write(nil, key: "remote") }
      }
      let updated = savedConnections.filter { $0.id != connection.id }
      try Vault.write(JSONEncoder().encode(updated), key: "computers")
      savedConnections = updated
      if case .remote(let active) = source, active.id == connection.id { disconnect() }
    } catch { self.error = error.localizedDescription }
  }
  func renameComputer(_ connection: RemoteConnection, label: String) {
    do {
      var updated = connection
      updated.label = label.trimmingCharacters(in: .whitespacesAndNewlines)
      let profiles = savedConnections.map { $0.id == connection.id ? updated : $0 }
      try Vault.write(JSONEncoder().encode(profiles), key: "computers")
      savedConnections = profiles
    } catch { self.error = error.localizedDescription }
  }
  func pair(_ value: String, certificatePin: String? = nil) async throws {
    let current = selection
    if PairingLink.isNativeInvitation(value) {
      guard certificatePin == nil else {
        throw ClientError("Native invitations use the server's trusted TLS certificate.")
      }
      let bridge = try NativePairing()
      let start = try bridge.request(["op": .string("start"), "link": .string(value)])
      defer { _ = try? bridge.request(["op": .string("close"), "handle": start["handle"]]) }
      guard let address = start["url"].string else { throw ClientError("Missing paired endpoint.") }
      let connection = RemoteConnection(
        address: address, token: "", label: "Computer", pairingLink: value)
      openRemote(connection)
      return
    }
    let link = try PairingLink.parse(value)
    let deviceID: String
    if let data = try Vault.read("device-id"), let existing = String(data: data, encoding: .utf8) {
      deviceID = existing
    } else {
      deviceID = UUID().uuidString
      try Vault.write(Data(deviceID.utf8), key: "device-id")
    }
    let grant = try await PairingClient.redeem(
      link, deviceID: deviceID, label: NativePlatform.deviceName, certificatePin: certificatePin)
    guard current == selection, !Task.isCancelled else { throw CancellationError() }
    guard let token = grant["token"].string else { throw ClientError("Missing connection token.") }
    if let certificatePin, let returned = grant["transportCertificatePin"].string,
      returned != certificatePin
    {
      throw ClientError(
        "The computer identity changed during pairing. Generate a new code and try again.")
    }
    openRemote(
      RemoteConnection(
        address: link.webSocketURL.absoluteString, token: token,
        certificatePin: certificatePin,
        environmentID: grant["environmentId"].string))
  }
  func restoreRemote() {
    if let connection = savedConnections.first {
      openRemote(connection)
    } else {
      error = "No saved computer is available. Pair a computer first."
    }
  }
  private func rememberWorkspace(_ source: Source) throws {
    let location: WorkspaceLocation
    switch source {
    case .remote(let connection): location = WorkspaceLocation(kind: .computer, id: connection.id)
    case .cloud(let id):
      location = WorkspaceLocation(kind: .cloud, id: id, account: account.archiveIdentity)
    case .environment(let id):
      location = WorkspaceLocation(kind: .environment, id: id, account: account.archiveIdentity)
    }
    try Vault.write(JSONEncoder().encode(location), key: "last-workspace")
  }
  private func startConnection(
    _ source: Source, sessionID: String?, preserving: Bool = false, wakePausedCloud: Bool = true
  ) {
    let retainedSession = preserving ? selectedSession : nil
    let retainedTranscript = transcript
    let retainedDraft = draft
    persistDraft(immediate: true)
    if let session = selectedSession { drafts[session.id] = draft }
    disconnect()
    self.source = source
    if let retainedSession {
      selectedSession = retainedSession
      transcript = retainedTranscript
      draft = retainedDraft
    }
    let current = selection
    let accountGeneration = account.generation
    let cloudClient = cloud
    loading = true
    connectionState = "Connecting…"
    connectionProblem = nil
    connectionTask = Task {
      var pendingClient: RPCClient?
      do {
        guard current == selection, account.generation == accountGeneration, !Task.isCancelled
        else {
          return
        }
        let store = try makeArchive(for: source)
        archive = store
        let restoredID: String?
        if let sessionID {
          restoredID = sessionID
        } else {
          restoredID = try await store.load("__last-session").metadata?["id"].string
        }
        if let sessionID = restoredID, !preserving {
          let cached = try await store.load(sessionID)
          guard current == selection, account.generation == accountGeneration, !Task.isCancelled
          else { return }
          if let metadata = cached.metadata, let session = try? metadata.decode(SessionRow.self) {
            selectedSession = session
            transcript = Transcript(cached: cached.messages)
            draft = cached.draft
            outbox = cached.pending
            attachments = cached.attachments ?? []
          }
        }
        guard current == selection, account.generation == accountGeneration, !Task.isCancelled
        else {
          return
        }
        let client: RPCClient
        switch source {
        case .cloud(let id):
          client = try await cloudClient.connect(workspaceID: id, wakeIfPaused: wakePausedCloud)
        case .environment(let id):
          client = try await cloudClient.connectEnvironment(
            id, proof: deviceProof(),
            canonicalAccount: environments.first { $0.id == id }?.canonicalAccount == true)
        case .remote(let connection):
          guard var url = URLComponents(string: connection.address), let endpoint = url.url else {
            throw ClientError("Invalid connection address.")
          }
          try PairingLink.validate(endpoint, webSocket: true)
          var items = (url.queryItems ?? []).filter { $0.name != "token" }
          if !connection.token.isEmpty {
            items.append(URLQueryItem(name: "token", value: connection.token))
          }
          url.queryItems = items
          guard let address = url.url else { throw ClientError("Invalid remote address.") }
          if let invitation = connection.pairingLink {
            client = try RPCClient(pairingLink: invitation)
          } else if let pin = connection.certificatePin, address.scheme == "wss" {
            client = try RPCClient(
              url: address, session: PinnedConnection.session(pin: pin), ownsSession: true)
          } else {
            client = try RPCClient(url: address)
          }
          pendingClient = client
          do { try await client.connect() } catch {
            await client.close()
            throw error
          }
          let native = await client.canonical
          let descriptor = try await client.call(native ? "EngineInfo" : "connect.describe")
          let identity = descriptor[native ? "deviceId" : "environmentId"].string
          if let id = connection.environmentID {
            guard identity == id else {
              await client.close()
              throw ClientError(
                "The paired computer identity no longer matches.", code: "identity-changed")
            }
          }
          guard current == selection, account.generation == accountGeneration, !Task.isCancelled
          else {
            await client.close()
            return
          }
          connectedComputer =
            connection.label ?? descriptor["label"].string ?? connection.displayName
          if !ProcessInfo.processInfo.arguments.contains("--rpc-fixture") {
            var verified = connection
            verified.environmentID = identity ?? connection.environmentID
            verified.label = connection.label ?? descriptor["label"].string
            let profiles = ConnectionProfiles.saving(verified, in: savedConnections)
            try Vault.write(JSONEncoder().encode(profiles), key: "computers")
            try Vault.write(nil, key: "remote")
            savedConnections = profiles
            self.source = .remote(verified)
            connectedComputer = verified.displayName
          }
        }
        pendingClient = client
        guard current == selection, account.generation == accountGeneration, !Task.isCancelled
        else {
          await client.close()
          return
        }
        rpc = client
        healthySince = .now
        if !ProcessInfo.processInfo.arguments.contains("--rpc-fixture") {
          try rememberWorkspace(self.source ?? source)
        }
        if case .environment(let id) = source {
          connectedComputer = environments.first { $0.id == id }?.label ?? "Computer"
        } else if case .cloud = source {
          connectedComputer = "Cloud workspace"
        }
        connectionState = "Synchronizing…"
        let loadedProjects =
          try await client.canonical
          ? CanonicalWorkspace(client).projects()
          : client.call("workspace.list").decode([ProjectRow].self)
        guard current == selection, account.generation == accountGeneration, !Task.isCancelled
        else {
          await client.close()
          return
        }
        projects = loadedProjects
        if let sessionID = restoredID {
          do {
            let session =
              try await client.canonical
              ? CanonicalWorkspace(client).session(id: sessionID)
              : client.call("session.get", ["sessionId": .string(sessionID)]).decode(
                SessionRow.self)
            guard session.id == sessionID else {
              throw ClientError(
                "The computer returned a different chat. Reconnect to reload its catalog.")
            }
            guard current == selection, account.generation == accountGeneration, !Task.isCancelled
            else {
              await client.close()
              return
            }
            selectedProject = projects.first { $0.id == session.projectId }
            await loadSessions()
            guard current == selection, account.generation == accountGeneration, !Task.isCancelled
            else { return }
            selectSession(session)
          } catch let failure as ClientError where failure.code == "SessionNotFoundError" {
            guard current == selection, account.generation == accountGeneration else { return }
            try await store.saveMetadata(.null, sessionID: "__last-session")
            selectedSession = nil
            transcript = Transcript()
            outbox = []
            attachments = []
            connectionProblem =
              "The previous chat is no longer on this computer. Choose another chat or create one."
            connectionState = "Connected"
            selectedProject = projects.first
            await loadSessions()
          }
        } else {
          connectionState = "Connected"
          if let project = projects.first {
            selectedProject = project
            await loadSessions()
          }
        }
      } catch {
        if let pendingClient { await pendingClient.close() }
        guard current == selection, account.generation == accountGeneration, !Task.isCancelled
        else { return }
        account.recordAuthenticationFailure(error)
        connectionState = "Offline · reconnecting"
        connectionProblem = error.localizedDescription
        if let rpc { await rpc.close() }
        rpc = nil
        if let failure = error as? ClientError, failure.requiresUserAction {
          connectionState = "Connection needs attention"
        } else {
          scheduleRecovery()
        }
      }
      if current == selection, account.generation == accountGeneration {
        loading = false
        if rpc != nil { sessionCatalogTask = Task { await self.refreshSessionCatalog() } }
      }
    }
  }
  private func gitKey(_ session: SessionRow) -> String {
    selection.uuidString + ":" + account.generation.uuidString + ":" + (operationScope ?? "offline")
      + ":" + session.projectId + ":" + (session.worktreeId ?? "main")
  }
  func gitState(for session: SessionRow) -> JSONValue { gitStates[gitKey(session)] ?? .null }
  func requestGitState(_ session: SessionRow, refresh: Bool = false) {
    guard connectionState == "Connected", rpc != nil else { return }
    let key = gitKey(session)
    guard refresh || gitStates[key] == nil, gitQueued.insert(key).inserted else { return }
    gitQueue.append((key, session))
    guard gitWorker == nil else { return }
    gitWorker = Task {
      defer { gitWorker = nil }
      while !gitQueue.isEmpty && !Task.isCancelled {
        let (key, session) = gitQueue.removeFirst()
        defer { gitQueued.remove(key) }
        guard gitKey(session) == key, connectionState == "Connected", let rpc else { continue }
        var payload: [String: JSONValue] = ["folderId": .string(session.projectId)]
        if let tree = session.worktreeId { payload["worktreeId"] = .string(tree) }
        let value = try? await rpc.call("git.prState", payload)
        guard gitKey(session) == key, !Task.isCancelled else { continue }
        if gitStates.count >= 256 { gitStates.removeAll() }
        gitStates[key] = value ?? gitStates[key] ?? .null
      }
    }
  }
  func loadSessions() async {
    guard let rpc, let project = selectedProject else { return }
    let current = selection
    let result = await SessionCatalog.load([project.id]) { projectID in
      if await rpc.canonical {
        return try await CanonicalWorkspace(rpc).sessions(projectID: projectID)
      }
      return try await rpc.call("session.list", ["projectId": .string(projectID)]).decode(
        [SessionRow].self)
    }
    guard current == selection, selectedProject?.id == project.id, !Task.isCancelled else { return }
    guard result.failedProjectIDs.isEmpty else {
      catalogProblem = "This project's chats could not be loaded. Pull to refresh."
      return
    }
    sessions = sessions.filter { $0.projectId != project.id } + result.rows
  }
  func refreshSessionCatalog() async {
    guard let rpc else { return }
    let current = selection
    let result = await SessionCatalog.load(projects.map(\.id)) { projectID in
      if await rpc.canonical {
        return try await CanonicalWorkspace(rpc).sessions(projectID: projectID)
      }
      return try await rpc.call("session.list", ["projectId": .string(projectID)]).decode(
        [SessionRow].self)
    }
    guard current == selection, !Task.isCancelled else { return }
    let failed = Set(result.failedProjectIDs)
    sessions = result.rows + sessions.filter { failed.contains($0.projectId) }
    catalogProblem =
      failed.isEmpty ? nil : "Some project chats could not be loaded. Pull to refresh."
  }
  func selectSession(_ session: SessionRow) {
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--native-preview") {
        do { try loadPreview(selected: session) } catch { self.error = error.localizedDescription }
        return
      }
    #endif
    guard selectedSession?.id != session.id || !sessionReady else { return }
    persistDraft(immediate: true)
    if let old = selectedSession { drafts[old.id] = draft }
    timelineTask?.cancel()
    hydrationTask?.cancel()
    checkpointTask?.cancel()
    checkpointTask = nil
    heartbeatTask?.cancel()
    runtimeExpiryTask?.cancel()
    sessionActivity.restart()
    let generation = sessionGeneration
    let sameSession = selectedSession?.id == session.id
    selectedSession = session
    selectedProject = projects.first { $0.id == session.projectId } ?? selectedProject
    if !sameSession {
      transcript = Transcript()
      draft = drafts[session.id] ?? ""
      outbox = []
      attachments = []
    }
    hydrationTask = Task {
      do {
        guard let archive else {
          throw ClientError("Reconnect to restore this chat's local storage.")
        }
        let record = try await archive.load(session.id)
        guard generation == sessionGeneration, !Task.isCancelled else { return }
        if !sameSession {
          transcript = Transcript(cached: record.messages)
          draft = record.draft
        }
        outbox = record.pending
        attachments = record.attachments ?? []
        let metadata = try JSONDecoder().decode(
          JSONValue.self, from: JSONEncoder().encode(session))
        try await archive.saveMetadata(metadata, sessionID: session.id)
        guard generation == sessionGeneration, !Task.isCancelled else { return }
        try await archive.saveMetadata(metadata, sessionID: "__last-session")
        guard generation == sessionGeneration, !Task.isCancelled else { return }
        sessionActivity.restored(generation: generation)
        subscribe(session.id)
      } catch {
        guard generation == sessionGeneration, !Task.isCancelled else { return }
        sessionActivity.failedRestoration(error.localizedDescription, generation: generation)
      }
    }
  }
  func retrySessionRestoration() {
    guard sessionRestorationError != nil, let session = selectedSession else { return }
    selectSession(session)
  }
  private func subscribe(_ sessionID: String) {
    guard let rpc else { return }
    let generation = sessionGeneration
    timelineTask = Task {
      do {
        if await rpc.canonical {
          startCanonicalRuntime(sessionID: sessionID, generation: generation)
          var canonical = CanonicalTranscript()
          let stream = try await rpc.stream(
            "WatchDocMessages", ["chatId": .string(sessionID), "openingTail": .bool(true)])
          for try await update in stream {
            try Task.checkCancellation()
            guard generation == sessionGeneration, selectedSession?.id == sessionID else { return }
            try canonical.apply(update)
            transcript.replaceCanonical(
              canonical.messages(sessionID: sessionID), interactions: canonical.interactions)
            checkpointTranscript()
            if !update["historyPending"].bool {
              connectionState = "Connected"
              Task { await flushOutbox() }
            }
          }
          throw ClientError("The transcript connection ended. Reconnect to continue.")
        }
        let stream = try await rpc.stream(
          "session.events", ["sessionId": .string(sessionID), "includeRuntime": .bool(true)])
        for try await frame in stream {
          try Task.checkCancellation()
          guard generation == sessionGeneration, selectedSession?.id == sessionID else { return }
          try transcript.apply(frame, sessionID: sessionID)
          checkpointTranscript()
          if frame["kind"].string == "synchronized" {
            connectionState = "Connected"
            startHeartbeat(sessionID: sessionID, generation: generation)
            Task { await flushOutbox() }
          }
        }
        if !Task.isCancelled {
          throw ClientError("The transcript connection ended. Reconnect to continue.")
        }
      } catch {
        guard !Task.isCancelled, generation == sessionGeneration, selectedSession?.id == sessionID
        else { return }
        connectionState = "Offline · reconnecting"
        connectionProblem = error.localizedDescription
        scheduleRecovery()
      }
    }
  }
  func reconnect() {
    guard let source else { return }
    guard !enqueueing else {
      scheduleRecovery()
      return
    }
    let sessionID = selectedSession?.id
    if let session = selectedSession { drafts[session.id] = draft }
    startConnection(source, sessionID: sessionID, preserving: true, wakePausedCloud: false)
  }
  func loadOlder() async {
    guard !paging, let before = transcript.olderSequence, let session = selectedSession, let rpc
    else { return }
    guard let lease = sessionActivity.begin(.page) else { return }
    let generation = lease.generation
    defer { sessionActivity.end(lease) }
    do {
      let page = try await rpc.call(
        "session.messages.page",
        [
          "sessionId": .string(session.id), "beforeSequence": .number(Double(before)),
          "limit": .number(100),
        ])
      guard generation == sessionGeneration else { return }
      try transcript.prependPage(page)
      checkpointTranscript()
    } catch { if generation == sessionGeneration { self.error = error.localizedDescription } }
  }
  var canSend: Bool {
    sessionReady && selectedSession != nil && archive != nil && !sending && !enqueueing
      && !uploading
  }
  var selectedModelOptions: [String: String] {
    get {
      sessionModelOptions[(operationScope ?? "preview") + ":" + (selectedSession?.id ?? "")] ?? [:]
    }
    set {
      sessionModelOptions[(operationScope ?? "preview") + ":" + (selectedSession?.id ?? "")] =
        newValue
    }
  }
  private var sessionModelOptions: [String: [String: String]] = [:]
  func send() async {
    guard canSend, let session = selectedSession, let archive else { return }
    draftTask?.cancel()
    let originalDraft = draft
    let submittedAttachments = attachments
    let text = originalDraft.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty || !attachments.isEmpty else { return }
    guard let lease = sessionActivity.begin(.enqueue) else { return }
    let generation = lease.generation
    defer { sessionActivity.end(lease) }
    do {
      let input: JSONValue? =
        submittedAttachments.isEmpty
        ? nil
        : .object([
          "text": .string(text), "attachments": .array(submittedAttachments),
          "fileRefs": .array([]),
          "skillRefs": .array([]), "annotations": .array([]),
        ])
      let pending = PendingMessage(
        sessionID: session.id, text: text, input: input, modelOptions: selectedModelOptions)
      try await archive.enqueue(pending)
      guard generation == sessionGeneration else { return }
      if draft == originalDraft { draft = "" }
      attachments.removeAll { submittedAttachments.contains($0) }
      let remainingDraft = draft
      let remainingAttachments = attachments
      try await archive.saveComposer(
        remainingDraft, attachments: remainingAttachments, sessionID: session.id)
      let pendingMessages = try await archive.load(session.id).pending
      guard generation == sessionGeneration else { return }
      outbox = pendingMessages
      sessionActivity.end(lease)
      await flushOutbox()
    } catch { self.error = error.localizedDescription }
  }
  func respondToUI(_ message: ChatMessage, text: String) async throws {
    guard sessionReady, let archive, selectedSession?.id == message.sessionId else {
      throw ClientError("Open this chat to respond.")
    }
    let generation = sessionGeneration
    try await archive.enqueue(
      PendingMessage(sessionID: message.sessionId, text: text), responseTo: message.id)
    guard generation == sessionGeneration else { return }
    let pendingMessages = try await archive.load(message.sessionId).pending
    guard generation == sessionGeneration else { return }
    outbox = pendingMessages
    await flushOutbox()
  }
  func flushOutbox(retryID: String? = nil) async {
    guard !sending, connectionState == "Connected", let session = selectedSession, let rpc,
      let archive
    else { return }
    guard let lease = sessionActivity.begin(.send) else { return }
    let generation = lease.generation
    defer { sessionActivity.end(lease) }
    do {
      let queued = try await archive.load(session.id).pending
      for message in queued {
        guard generation == sessionGeneration, connectionState == "Connected" else { return }
        guard message.delivery == .queued || message.id == retryID else { continue }
        guard let attempt = try await archive.markAttempted(message.id, sessionID: session.id)
        else {
          continue
        }
        if await rpc.canonical {
          try await CanonicalWorkspace(rpc).send(attempt)
        } else {
          _ = try await rpc.call("messages.send", attempt.payload)
        }
        try await archive.remove(message.id, sessionID: session.id)
      }
    } catch {
      if generation == sessionGeneration { self.error = error.localizedDescription }
    }
    if generation == sessionGeneration {
      let pendingMessages = try? await archive.load(session.id).pending
      if generation == sessionGeneration, let pendingMessages { outbox = pendingMessages }
    }
  }
  func removePending(_ message: PendingMessage) async {
    guard let archive, !sending else { return }
    let generation = sessionGeneration
    do {
      try await archive.remove(message.id, sessionID: message.sessionID)
      let pending = try await archive.load(message.sessionID).pending
      if generation == sessionGeneration { outbox = pending }
    } catch { self.error = error.localizedDescription }
  }
  func stop() async {
    guard let session = selectedSession, let rpc else { return }
    if await rpc.canonical {
      do { try await CanonicalWorkspace(rpc).stop(chatID: session.id) } catch {
        self.error = error.localizedDescription
      }
      return
    }
    guard let turn = transcript.activeTurnID else { return }
    do {
      _ = try await rpc.call(
        "messages.interrupt",
        [
          "sessionId": .string(session.id), "commandId": .string(UUID().uuidString),
          "expectedTurnId": .string(turn),
        ])
    } catch { self.error = error.localizedDescription }
  }
  var operationScope: String? {
    switch source {
    case .remote(let connection): return connection.id
    case .environment(let id): return "environment:" + id
    case .cloud(let id): return (account.archiveIdentity ?? "") + ":cloud:" + id
    case nil: return nil
    }
  }
  func createChat(_ creation: ChatCreation) async throws {
    guard let rpc, connectionState == "Connected" else {
      throw ClientError("Reconnect to a computer before creating a chat.")
    }
    let generation = selection
    let session: SessionRow
    if await rpc.canonical {
      session = try await CanonicalWorkspace(rpc).create(creation)
      if let prompt = creation.initialPrompt, !prompt.isEmpty, let archive {
        let key = "initial:" + creation.operationID
        let record = try await archive.load(session.id)
        if record.responses?[key] == nil {
          try await archive.enqueue(
            PendingMessage(sessionID: session.id, text: prompt), responseTo: key)
        }
      }
    } else {
      let response = try await rpc.call("chat.create", creation.payload)
      session = try response["initialSession"].decode(SessionRow.self)
    }
    guard generation == selection, !Task.isCancelled else { throw CancellationError() }
    selectedProject = projects.first { $0.id == session.projectId }
    await loadSessions()
    guard generation == selection, !Task.isCancelled else { throw CancellationError() }
    selectSession(session)
  }
  func setModel(_ model: String) async {
    guard sessionReady, let session = selectedSession, let rpc, connectionState == "Connected",
      transcript.activeTurnID == nil
    else { return }
    let generation = sessionGeneration
    do {
      if await rpc.canonical {
        let updated = try await CanonicalWorkspace(rpc).setModel(chatID: session.id, model: model)
        guard generation == sessionGeneration else { return }
        selectedSession = updated
        sessions = sessions.map { $0.id == updated.id ? updated : $0 }
        return
      }

      _ = try await rpc.call(
        "session.setModel", ["sessionId": .string(session.id), "model": .string(model)])
      let updated = try await rpc.call("session.get", ["sessionId": .string(session.id)]).decode(
        SessionRow.self)
      guard generation == sessionGeneration else { return }
      selectedSession = updated
      sessions = sessions.map { $0.id == updated.id ? updated : $0 }
    } catch { if generation == sessionGeneration { self.error = error.localizedDescription } }
  }
  func addProject(path: String) async throws {
    guard let rpc else { throw ClientError("Connect to a computer first.") }
    let generation = selection
    let project: ProjectRow
    if await rpc.canonical {
      project = try await CanonicalWorkspace(rpc).addProject(path: path)
    } else {
      project = try await rpc.call("workspace.add", ["path": .string(path)]).decode(ProjectRow.self)
    }
    guard generation == selection else { throw CancellationError() }
    projects.removeAll { $0.id == project.id }
    projects.append(project)
    selectedProject = project
    await loadSessions()
  }
  func attach(_ data: Data, name: String, mimeType: String) async {
    guard composerEditable, !uploading, let rpc, let session = selectedSession,
      connectionState == "Connected"
    else {
      return
    }
    guard let lease = sessionActivity.begin(.upload) else { return }
    let generation = lease.generation
    defer { sessionActivity.end(lease) }
    do {
      let ref = try await Attachments.upload(
        data, name: name, mimeType: mimeType, sessionID: session.id, rpc: rpc)
      guard generation == sessionGeneration else { return }
      attachments.append(ref)
      try await archive?.saveAttachments(attachments, sessionID: session.id)
    } catch { if generation == sessionGeneration { self.error = error.localizedDescription } }
  }
  func removeAttachment(_ id: String) {
    guard composerEditable else { return }
    attachments.removeAll { $0["id"].string == id }
    guard let archive, let session = selectedSession else { return }
    let refs = attachments
    Task {
      do { try await archive.saveAttachments(refs, sessionID: session.id) } catch {
        self.error = error.localizedDescription
      }
    }
  }
  func savedInteractionResponse(method: String, itemID: String) async throws -> InteractionResponse?
  {
    guard let archive, let session = selectedSession else { return nil }
    let generation = sessionGeneration
    let response = try await archive.interactionResponse(
      method: method, itemID: itemID, sessionID: session.id)
    guard generation == sessionGeneration else { throw CancellationError() }
    return response
  }
  private func respondToInteraction(method: String, itemID: String, fields: [String: JSONValue])
    async throws
  {
    guard let session = selectedSession, let rpc, let archive, connectionState == "Connected" else {
      throw ClientError("Reconnect before responding to the agent.")
    }
    let identity = session.id + ":" + InteractionResponse.key(method: method, itemID: itemID)
    guard respondingInteractions.insert(identity).inserted else {
      throw ClientError("This response is already being submitted.")
    }
    defer { respondingInteractions.remove(identity) }
    let generation = sessionGeneration
    let response = try await archive.prepareInteractionResponse(
      method: method, itemID: itemID, sessionID: session.id, fields: fields)
    guard generation == sessionGeneration, connectionState == "Connected" else {
      throw CancellationError()
    }
    if !response.confirmed {
      _ = try await rpc.call(response.method, response.payload)
      try await archive.confirmInteractionResponse(response)
    }
  }
  func retryInteractionResponse(_ response: InteractionResponse) async throws {
    guard selectedSession?.id == response.sessionID else { throw CancellationError() }
    try await respondToInteraction(
      method: response.method, itemID: response.itemID, fields: response.fields)
  }
  func answerQuestion(_ itemID: String, answers: [JSONValue]) async throws {
    if let rpc, await rpc.canonical, let session = selectedSession {
      guard let question = transcript.interactions.first(where: { $0["id"].string == itemID })
      else { throw ClientError("This question is no longer pending.") }
      try await CanonicalWorkspace(rpc).answer(
        chatID: session.id, requestID: itemID, questions: question["questions"].array,
        answers: answers)
      return
    }
    try await respondToInteraction(
      method: "session.answerQuestion", itemID: itemID, fields: ["answers": .array(answers)])
  }
  func cancelQuestion(_ itemID: String) async throws {
    guard let session = selectedSession, let rpc, connectionState == "Connected" else {
      throw ClientError("Reconnect before cancelling this question.")
    }
    _ = try await rpc.call(
      "session.cancelQuestion", ["sessionId": .string(session.id), "itemId": .string(itemID)])
  }
  func respondToPlan(_ itemID: String, outcome: String, feedback: String) async throws {
    var fields: [String: JSONValue] = ["outcome": .string(outcome)]
    if !feedback.isEmpty { fields["feedback"] = .string(feedback) }
    try await respondToInteraction(method: "session.plan.respond", itemID: itemID, fields: fields)
  }
  func decidePermission(_ id: String, allow: Bool) async {
    guard let rpc else { return }
    do {
      _ = try await rpc.call(
        "permission.decide",
        [
          "requestId": .string(id),
          "decision": .object(["_tag": .string(allow ? "AllowOnce" : "Deny")]),
        ])
    } catch { self.error = error.localizedDescription }
  }
  func disconnect(clearSaved: Bool = false) {
    if clearSaved {
      do { try Vault.write(nil, key: "last-workspace") } catch {
        self.error = error.localizedDescription
      }
    }
    selection = UUID()
    sessionActivity.restart()
    recoveryTask?.cancel()
    heartbeatTask?.cancel()
    runtimeExpiryTask?.cancel()
    hydrationTask?.cancel()
    checkpointTask?.cancel()
    checkpointTask = nil
    connectionTask?.cancel()
    sessionCatalogTask?.cancel()
    catalogProblem = nil
    timelineTask?.cancel()
    if let rpc { Task { await rpc.close() } }
    rpc = nil
    source = nil
    connectedComputer = nil
    if let archive { Task { await archive.releaseMemory() } }
    archive = nil
    attachments = []
    outbox = []
    projects = []
    sessions = []
    selectedSession = nil
    selectedProject = nil
    transcript = Transcript()
    connectionState = "Not connected"
    loading = false
  }
  func signIn() async throws {
    let previous = account.archiveIdentity
    if composerEditable, let archive, let session = selectedSession {
      try await archive.saveDraft(draft, sessionID: session.id)
    }
    try await account.signIn()
    if previous != account.archiveIdentity {
      disconnect()
      archives = [:]
      cloudChats = []
      pendingCloudActions = [:]
      cloudRequestsBusy = []
      cloudCatalogProblem = nil
      environments = []
      sessions = []
      projects = []
      hydratedCatalog = nil
      drafts = [:]
      draft = ""
    }
    await refreshCloud()
    if source != nil { reconnect() } else { restoreWorkspace() }
  }
  func signOut() async {
    clearingAccount = true
    catalogRequest = UUID()
    hydratedCatalog = nil
    defer { clearingAccount = false }
    let accountDirectory = account.archiveIdentity.map {
      archiveRoot.appendingPathComponent(ArchiveStore.storageID($0))
    }
    let retiredArchives = Array(archives.values)
    archives = [:]
    draftTask?.cancel()
    checkpointTask?.cancel()
    disconnect()
    cloudChats = []
    pendingCloudActions = [:]
    cloudRequestsBusy = []
    cloudCatalogProblem = nil
    environments = []
    drafts = [:]
    draft = ""
    outbox = []
    do {
      try account.signOut()
      for store in retiredArchives { try await store.erase() }
      if let accountDirectory, FileManager.default.fileExists(atPath: accountDirectory.path) {
        try FileManager.default.removeItem(at: accountDirectory)
      }
    } catch { self.error = error.localizedDescription }
  }
  private var archiveRoot: URL {
    FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
      .appendingPathComponent("NativeArchives")
  }
  private func makeArchive(for source: Source) throws -> ArchiveStore {
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--rpc-fixture") {
        let scope = "fixture"
        if let existing = archives[scope] { return existing }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
          "ZuseNativeFixture-" + UUID().uuidString)
        let key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
        let store = try ArchiveStore(directory: directory, key: key)
        archives[scope] = store
        return store
      }
    #endif
    let identity: String
    switch source {
    case .cloud(let workspaceID):
      guard let accountID = account.archiveIdentity else {
        throw ClientError("Sign in again to open this account's offline storage.")
      }
      identity = "cloud|\(accountID)|\(workspaceID)"
    case .environment(let id):
      guard let accountID = account.archiveIdentity else {
        throw ClientError("Sign in to access this computer.")
      }
      identity = "environment|\(accountID)|\(id)"
    case .remote(let remote): identity = "remote|\(remote.address)"
    }
    let scope = ArchiveStore.storageID(identity)
    if let existing = archives[scope] { return existing }
    let keyName = "archive-key-" + scope
    let key: Data
    if let stored = try Vault.read(keyName) {
      key = stored
    } else {
      key = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
      try Vault.write(key, key: keyName)
    }
    let owner: String
    switch source {
    case .remote: owner = "remote"
    case .cloud, .environment: owner = ArchiveStore.storageID(account.archiveIdentity ?? "")
    }
    let directory = archiveRoot.appendingPathComponent(owner).appendingPathComponent(scope)
    let store = try ArchiveStore(directory: directory, key: key)
    archives[scope] = store
    return store
  }
  func persistDraft(immediate: Bool = false) {
    draftTask?.cancel()
    guard composerEditable, let session = selectedSession, let archive else { return }
    let value = draft
    draftTask = Task {
      if !immediate { try? await Task.sleep(for: .milliseconds(250)) }
      guard !Task.isCancelled else { return }
      do { try await archive.saveDraft(value, sessionID: session.id) } catch {
        self.error = error.localizedDescription
      }
    }
  }
  private func checkpointTranscript() {
    checkpointRevision = UUID()
    guard checkpointTask == nil, let session = selectedSession, let archive else { return }
    let generation = sessionGeneration
    checkpointTask = Task {
      defer { if generation == sessionGeneration { checkpointTask = nil } }
      do {
        while !Task.isCancelled, generation == sessionGeneration {
          try await Task.sleep(for: .milliseconds(500))
          guard generation == sessionGeneration else { return }
          let revision = checkpointRevision
          let messages = transcript.messages
          try await archive.saveMessages(messages, sessionID: session.id)
          let pendingMessages = try await archive.load(session.id).pending
          guard generation == sessionGeneration else { return }
          outbox = pendingMessages
          if checkpointRevision == revision { return }
        }
      } catch is CancellationError {} catch {
        if generation == sessionGeneration { self.error = error.localizedDescription }
      }
    }
  }
  private func scheduleRecovery() {
    recoveryTask?.cancel()
    heartbeatTask?.cancel()
    runtimeExpiryTask?.cancel()
    guard foreground, networkAvailable, source != nil else { return }
    let healthyFor = healthySince.map { Double($0.duration(to: .now).components.seconds) } ?? 0
    let delay = recoveryBackoff.nextDelay(healthyForSeconds: healthyFor)
    healthySince = nil
    recoveryTask = Task {
      try? await Task.sleep(for: .seconds(delay))
      guard !Task.isCancelled else { return }
      reconnect()
    }
  }
  private func startCanonicalRuntime(sessionID: String, generation: UUID) {
    heartbeatTask?.cancel()
    runtimeExpiryTask?.cancel()
    canonicalRuntimeRow = .null
    guard let rpc else { return }
    runtimeExpiryTask = Task {
      while !Task.isCancelled {
        try? await Task.sleep(for: .seconds(5))
        guard !Task.isCancelled, generation == sessionGeneration else { return }
        transcript.applyCanonicalRuntime(canonicalRuntimeRow, sessionID: sessionID)
      }
    }
    heartbeatTask = Task {
      do {
        let stream = try await rpc.stream("WatchSessions")
        for try await frame in stream {
          try Task.checkCancellation()
          guard generation == sessionGeneration, selectedSession?.id == sessionID else { return }
          canonicalRuntimeRow = frame.array.first { $0["chatId"].string == sessionID } ?? .null
          transcript.applyCanonicalRuntime(canonicalRuntimeRow, sessionID: sessionID)
        }
        throw ClientError("Runtime status stream ended. Reconnect to continue.")
      } catch {
        guard !Task.isCancelled, generation == sessionGeneration else { return }
        connectionState = "Offline · reconnecting"
        connectionProblem = error.localizedDescription
        scheduleRecovery()
      }
    }
  }

  private func startHeartbeat(sessionID: String, generation: UUID) {
    heartbeatTask?.cancel()
    runtimeExpiryTask?.cancel()
    guard let rpc else { return }
    heartbeatTask = Task {
      while !Task.isCancelled {
        do {
          try await Task.sleep(for: .seconds(15))
          let head = try await rpc.call("session.events.head", ["sessionId": .string(sessionID)])
          guard generation == sessionGeneration else { return }
          if let epoch = head["streamEpoch"].string, let current = transcript.epoch,
            epoch != current
          {
            scheduleRecovery()
            return
          }
          if (head["throughVersion"].int ?? 0) > transcript.version {
            try await Task.sleep(for: .seconds(2))
            guard generation == sessionGeneration else { return }
            if (head["throughVersion"].int ?? 0) > transcript.version {
              scheduleRecovery()
              return
            }
          }
        } catch {
          guard !Task.isCancelled, generation == sessionGeneration else { return }
          connectionState = "Offline · reconnecting"
          connectionProblem = error.localizedDescription
          scheduleRecovery()
          return
        }
      }
    }
  }
  func sceneChanged(active: Bool) {
    foreground = active
    if active {
      if source != nil { reconnect() }
      Task { await refreshCloud() }
    } else {
      persistDraft(immediate: true)
      recoveryTask?.cancel()
      heartbeatTask?.cancel()
      runtimeExpiryTask?.cancel()
    }
  }

}
