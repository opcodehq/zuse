import Foundation

// Wire contracts for the Rust cloud API (`crates/protocol/src/cloud*.rs`). Decoding keeps
// unknown enum values as strings so a newer server can't break an older app.

/// The agents cloud connections exist for (`AgentId`, kebab-case).
public enum CloudAgent: String, CaseIterable, Codable, Sendable, Identifiable {
  case claude, codex, grok
  public var id: String { rawValue }
  public var title: String {
    switch self {
    case .claude: return "Claude"
    case .codex: return "Codex"
    case .grok: return "Grok"
    }
  }
  /// The harness the launch catalog lists this agent under (`HarnessId`).
  public var harness: String { self == .claude ? "claude-code" : rawValue }
  public init?(harness: String) {
    switch harness {
    case "claude-code", "claude": self = .claude
    case "codex": self = .codex
    case "grok": self = .grok
    default: return nil
    }
  }
  /// Methods the server accepts for this agent (`ConnectionMethod::supported`), preferred first.
  public var methods: [CloudConnectionMethod] {
    self == .claude ? [.setupToken, .apiKey] : [.deviceCode, .apiKey]
  }
}

public enum CloudConnectionMethod: String, Codable, Sendable {
  case apiKey = "api-key"
  case setupToken = "setup-token"
  case deviceCode = "device-code"
  public var title: String {
    switch self {
    case .apiKey: return "API key"
    case .setupToken: return "Setup token"
    case .deviceCode: return "Browser sign-in"
    }
  }
}

/// `AgentConnection` plus the `connectionScope` tag `/cloud/context` adds.
public struct CloudAgentConnection: Codable, Equatable, Identifiable, Sendable {
  public let connectionId: String
  public let agent: String
  public let method: String
  public let state: String
  public let accountLabel: String?
  public let expiresAt: Int?
  public let version: Int?
  public let errorCode: String?
  public let connectionScope: String?
  public let credentialUpdate: CloudLoginOperation?
  public var id: String { connectionId }
  public var cloudAgent: CloudAgent? { CloudAgent(rawValue: agent) }
  public var connectionMethod: CloudConnectionMethod? { CloudConnectionMethod(rawValue: method) }
  public var isConnected: Bool { state == "connected" }
  public var needsAttention: Bool { ["reconnect-required", "error"].contains(state) }
  public var stateLabel: String {
    switch state {
    case "connected": return "Connected"
    case "pending": return "Connecting…"
    case "reconnect-required": return "Sign in again"
    case "revoked": return "Disconnected"
    case "error": return "Needs attention"
    default: return CloudErrorText.humanize(state)
    }
  }
}

public struct CloudAgentConnectionList: Decodable, Sendable {
  public let connections: [CloudAgentConnection]
  public let supportsDeviceReconnect: Bool?
  public let supportsCredentialReconfiguration: Bool?
}

/// `LoginOperation`: a device-code sign-in the cloud runs on the user's behalf.
public struct CloudLoginOperation: Codable, Equatable, Sendable {
  public let operationId: String
  public let connectionId: String
  public let agent: String
  public let state: String
  public let verificationUrl: String?
  public let verificationCode: String?
  public let expiresAt: Int?
  public let errorCode: String?
  public var isPending: Bool { state == "queued" || state == "authorizing" }
  public var cloudAgent: CloudAgent? { CloudAgent(rawValue: agent) }
  /// The verification page, only when it is a plain HTTPS address.
  public var verificationPage: URL? {
    guard let verificationUrl, let url = URL(string: verificationUrl), url.scheme == "https",
      url.host != nil, url.user == nil, url.password == nil
    else { return nil }
    return url
  }
  /// Why a finished operation did not connect, or nil when it did (or is still running).
  public var outcomeMessage: String? {
    switch state {
    case "connected", "queued", "authorizing": return nil
    case "expired": return "The sign-in code expired. Start again."
    case "cancelled": return "Sign-in was cancelled."
    default: return errorCode.map(CloudErrorText.message(for:)) ?? "Agent sign-in failed."
    }
  }
}

/// `ConfigureConnection` (`deny_unknown_fields`). The secret travels as plain text over HTTPS.
public struct CloudConfigureAgentRequest: Sendable {
  public let agent: CloudAgent
  public let method: CloudConnectionMethod
  public let secret: String
  public let idempotencyKey: String
  public let connectionId: String?
  public init(
    agent: CloudAgent, method: CloudConnectionMethod, secret: String, idempotencyKey: String,
    connectionId: String? = nil
  ) {
    self.agent = agent
    self.method = method
    self.secret = secret
    self.idempotencyKey = idempotencyKey
    self.connectionId = connectionId
  }
  public var body: JSONValue {
    var fields: [String: JSONValue] = [
      "agent": .string(agent.rawValue), "method": .string(method.rawValue),
      "secret": .string(secret), "idempotencyKey": .string(idempotencyKey),
    ]
    if let connectionId { fields["connectionId"] = .string(connectionId) }
    return .object(fields)
  }
}

/// `StartConnectionLogin` (`deny_unknown_fields`).
public struct CloudStartLoginRequest: Sendable {
  public let agent: CloudAgent
  public let idempotencyKey: String
  public let connectionId: String?
  public init(agent: CloudAgent, idempotencyKey: String, connectionId: String? = nil) {
    self.agent = agent
    self.idempotencyKey = idempotencyKey
    self.connectionId = connectionId
  }
  public var body: JSONValue {
    var fields: [String: JSONValue] = [
      "agent": .string(agent.rawValue), "idempotencyKey": .string(idempotencyKey),
    ]
    if let connectionId { fields["connectionId"] = .string(connectionId) }
    return .object(fields)
  }
}

/// A repository connected to the account (`CloudProject`, as listed in context and projects).
public struct CloudProject: Codable, Equatable, Identifiable, Sendable {
  public let projectId: String
  public let displayName: String
  public let defaultBranch: String
  public let repositoryIdentity: String?
  public let repositoryUrl: String?
  public let visibility: String?
  public let state: String?
  public let canCreate: Bool?
  public var id: String { projectId }
  enum CodingKeys: String, CodingKey {
    case projectId, displayName, defaultBranch, repositoryIdentity, repositoryUrl, visibility
    case state, canCreate
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    projectId = try c.decode(String.self, forKey: .projectId)
    repositoryIdentity = try c.decodeIfPresent(String.self, forKey: .repositoryIdentity)
    repositoryUrl = try c.decodeIfPresent(String.self, forKey: .repositoryUrl)
    let name = try c.decodeIfPresent(String.self, forKey: .displayName)
    displayName = name.flatMap { $0.isEmpty ? nil : $0 } ?? repositoryIdentity ?? projectId
    let branch = try c.decodeIfPresent(String.self, forKey: .defaultBranch)
    defaultBranch = branch.flatMap { $0.isEmpty ? nil : $0 } ?? "main"
    visibility = try c.decodeIfPresent(String.self, forKey: .visibility)
    state = try c.decodeIfPresent(String.self, forKey: .state)
    canCreate = try c.decodeIfPresent(Bool.self, forKey: .canCreate)
  }
}

/// `ProjectConnectRequest`. `visibility` is lowercase; the server defaults the rest.
public struct CloudProjectConnectRequest: Sendable {
  public let repositoryUrl: String
  public let defaultBranch: String
  public let isPrivate: Bool
  public init(repositoryUrl: String, defaultBranch: String, isPrivate: Bool) {
    self.repositoryUrl = repositoryUrl
    self.defaultBranch = defaultBranch
    self.isPrivate = isPrivate
  }
  /// The body without its idempotency key, which a saved intent supplies.
  public var body: JSONValue {
    .object([
      "repositoryUrl": .string(repositoryUrl.trimmingCharacters(in: .whitespacesAndNewlines)),
      "defaultBranch": .string(defaultBranch.trimmingCharacters(in: .whitespacesAndNewlines)),
      "visibility": .string(isPrivate ? "private" : "public"),
    ])
  }
}

/// `CloudAgentCatalog`: launch choices for cloud computers.
public struct CloudAgentCatalog: Codable, Equatable, Sendable {
  public struct Model: Codable, Equatable, Sendable {
    public let id: String
    public let label: String
    public let description: String?
    public let reasoningLevels: [String]?
  }
  public struct Agent: Codable, Equatable, Sendable {
    public let id: String
    public let name: String
    public let defaultModel: String
    public let models: [Model]
    /// `CloudAgentModels::resolve_model`: exact id, or a Claude family alias.
    public func resolve(_ model: String) -> Model? {
      if let exact = models.first(where: { $0.id == model }) { return exact }
      guard id == "claude-code", ["sonnet", "opus", "haiku", "fable"].contains(model) else {
        return nil
      }
      return models.first { $0.id.hasPrefix("claude-\(model)-") }
    }
  }
  public let providers: [Agent]
  public init(providers: [Agent]) { self.providers = providers }
}

extension ModelProvider {
  /// The model picker's catalog for a cloud launch: one provider per agent harness.
  public static func cloud(_ catalog: CloudAgentCatalog) -> [ModelProvider] {
    let names = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra", "ultracode"]
    return catalog.providers.compactMap { agent in
      guard !agent.models.isEmpty else { return nil }
      let models = agent.models.map { model in
        // Prompt-prefix levels don't map to the launch request's `reasoning` field.
        let levels = (model.reasoningLevels ?? []).filter { names.contains($0) }
        let reasoning =
          levels.count > 1
          ? [
            Model.OptionDescriptor(
              id: "reasoning", kind: "select",
              defaultId: levels.contains("medium") ? "medium" : levels.first,
              options: levels.map {
                Model.OptionDescriptor.Option(id: $0, label: CloudErrorText.reasoningLabel($0))
              })
          ] : nil
        return Model(
          id: model.id, label: model.label, available: nil, defaultModel: nil,
          optionDescriptors: reasoning)
      }
      return ModelProvider(
        id: agent.id, name: agent.name,
        defaultModel: agent.resolve(agent.defaultModel)?.id ?? models[0].id, models: models)
    }
  }
}

/// `ComputerSize`: a provider's own size.
public struct CloudComputerSize: Codable, Equatable, Sendable {
  public let id: String
  public let label: String
  public let vcpus: Int
  public let memoryMib: Int
  public let diskGib: Int
}

/// `SizeOption`: a plan size for new computers, the same for every provider.
public struct CloudSizeOption: Codable, Equatable, Identifiable, Sendable {
  public let id: String
  public let label: String
  public let vcpus: Int
  public let memoryMib: Int
  public let machineRate: Int?
  public let available: Bool
  public let requiresPlan: String?
}

/// `ProviderCapabilities`. Provider ids are internal and never shown.
public struct CloudProviderCapability: Codable, Equatable, Sendable {
  public let id: String
  public let configured: Bool
  public let managedBaseReady: Bool
  public let commandAdmissionVersion: Int?
  public let preservesDisk: Bool
  public let sizes: [CloudComputerSize]
  public let unavailableReason: String?
  public var canCreate: Bool { configured && managedBaseReady && preservesDisk && !sizes.isEmpty }
}

/// `CloudCapabilities`, from `/cloud/capabilities` or `computerCapabilities` in context.
public struct CloudCapabilities: Codable, Equatable, Sendable {
  public let providers: [CloudProviderCapability]
  public let canConfigure: Bool?
  public let canAllocate: Bool?
  public let allocationBlockers: [String]?
  public let defaultProvider: String?
  public let computerSizes: [CloudSizeOption]?
}

/// `/cloud/context`: everything a launch picker needs, scoped to the request's workspace.
public struct CloudContext: Decodable, Sendable {
  public let ownerId: String?
  public let projects: [CloudProject]
  public let agentCatalog: CloudAgentCatalog?
  public let providerAgentCatalogs: [String: CloudAgentCatalog]?
  public let computerCapabilities: CloudCapabilities?
  /// Null when the server has no agent-connection authority; launches then omit a connection.
  public let agentConnections: [CloudAgentConnection]?
  public let commandAdmissionVersion: Int?
  public let launchAttachmentHoldVersion: Int?
  public let accountEvents: Bool?
}

/// `CloudWorkspaceCreateRequest` (`deny_unknown_fields`), without its idempotency key.
public struct CloudWorkspaceCreateRequest: Equatable, Sendable {
  /// Every field the server accepts; anything else is rejected.
  public static let acceptedFields: Set<String> = [
    "agentConnectionId", "environmentRevision", "attachments", "pendingAttachments", "projectId",
    "providerId", "baseRef", "agent", "model", "reasoning", "modelOptions", "idempotencyKey",
    "branch", "sizeId", "runtimeMode", "permissions", "secretBindings", "localDeviceId",
    "firstMessage", "initialMessageDelivery", "forkSource",
  ]
  public static let serverAdmitted = "server-admitted-v2"
  public var projectId: String
  public var providerId: String
  public var baseRef: String
  public var agent: String
  public var model: String
  public var agentConnectionId: String?
  public var sizeId: String?
  public var reasoning: String?
  public var firstMessage: String?
  public var initialMessageDelivery: String?
  public var runtimeMode = "approval-required"
  public init(
    projectId: String, providerId: String, baseRef: String, agent: String, model: String,
    agentConnectionId: String? = nil, sizeId: String? = nil, reasoning: String? = nil,
    firstMessage: String? = nil, initialMessageDelivery: String? = nil
  ) {
    self.projectId = projectId
    self.providerId = providerId
    self.baseRef = baseRef
    self.agent = agent
    self.model = model
    self.agentConnectionId = agentConnectionId
    self.sizeId = sizeId
    self.reasoning = reasoning
    self.firstMessage = firstMessage
    self.initialMessageDelivery = initialMessageDelivery
  }
  /// Read back a saved body; nil when it isn't one this app can retry as-is.
  public init?(body: JSONValue) {
    guard case .object(let fields) = body,
      Set(fields.keys).subtracting(["idempotencyKey"]).isSubset(of: Self.acceptedFields),
      let projectId = body["projectId"].string, let providerId = body["providerId"].string,
      let baseRef = body["baseRef"].string, let agent = body["agent"].string,
      let model = body["model"].string
    else { return nil }
    self.init(
      projectId: projectId, providerId: providerId, baseRef: baseRef, agent: agent, model: model,
      agentConnectionId: body["agentConnectionId"].string, sizeId: body["sizeId"].string,
      reasoning: body["reasoning"].string, firstMessage: body["firstMessage"].string,
      initialMessageDelivery: body["initialMessageDelivery"].string)
    runtimeMode = body["runtimeMode"].string ?? runtimeMode
  }
  public var body: JSONValue {
    var fields: [String: JSONValue] = [
      "projectId": .string(projectId), "providerId": .string(providerId),
      "baseRef": .string(baseRef), "agent": .string(agent), "model": .string(model),
      "runtimeMode": .string(runtimeMode),
    ]
    if let agentConnectionId { fields["agentConnectionId"] = .string(agentConnectionId) }
    if let sizeId { fields["sizeId"] = .string(sizeId) }
    if let reasoning { fields["reasoning"] = .string(reasoning) }
    if let firstMessage { fields["firstMessage"] = .string(firstMessage) }
    if let initialMessageDelivery {
      fields["initialMessageDelivery"] = .string(initialMessageDelivery)
    }
    return .object(fields)
  }
  /// `refs/heads/<branch>` for a branch name, as the desktop app sends it.
  public static func baseRef(branch: String) -> String {
    let name = branch.trimmingCharacters(in: .whitespacesAndNewlines)
    return name.hasPrefix("refs/") ? name : "refs/heads/" + name
  }
}

/// The create response: `{workspace, chatId, initialSessionId, initialMessageDelivery?}`.
public struct CloudWorkspaceCreated: Sendable {
  public let workspaceId: String
  public let chatId: String?
  public let initialSessionId: String
  public let initialMessageDelivery: String?
  public init(_ value: JSONValue) throws {
    guard let workspaceId = value["workspace"]["workspaceId"].string,
      let session = value["initialSessionId"].string
        ?? value["workspace"]["initialSessionId"].string
    else {
      throw ClientError(
        "The workspace response was incomplete. Refresh your cloud list before trying again.")
    }
    self.workspaceId = workspaceId
    chatId = value["chatId"].string ?? value["workspace"]["chatId"].string
    initialSessionId = session
    initialMessageDelivery = value["initialMessageDelivery"].string
  }
}

/// Launch choices derived from server data, mirroring the desktop picker's rules.
public struct CloudLaunchOptions: Sendable {
  public let context: CloudContext
  public let capabilities: CloudCapabilities?
  public init(context: CloudContext, capabilities: CloudCapabilities? = nil) {
    self.context = context
    self.capabilities = capabilities
  }
  private var computers: CloudCapabilities? { capabilities ?? context.computerCapabilities }
  private static let notReady =
    "Cloud computers are not ready for this account yet. Try again later or ask your administrator."
  /// The account's default provider when it can create computers, else the first that can.
  public func provider() -> Result<String, ClientError> {
    let rows = (computers?.providers ?? []).sorted { $0.id < $1.id }
    let ready = rows.filter { $0.canCreate && !(catalog(for: $0.id)?.providers.isEmpty ?? true) }
    if let id = computers?.defaultProvider, ready.contains(where: { $0.id == id }) {
      return .success(id)
    }
    if let first = ready.first { return .success(first.id) }
    let reason = rows.compactMap(\.unavailableReason).first {
      !$0.trimmingCharacters(in: .whitespaces).isEmpty
    }
    return .failure(ClientError(reason ?? Self.notReady, code: "cloud_provider_unavailable"))
  }
  /// Why new computers can't start right now, or nil when they can.
  public var allocationProblem: String? {
    guard computers?.canAllocate == false else { return nil }
    let blocker = computers?.allocationBlockers?.first
    return blocker.map(CloudErrorText.message(for:)) ?? Self.notReady
  }
  /// The launch catalog for a provider: its own when the server lists one per provider.
  public func catalog(for provider: String) -> CloudAgentCatalog? {
    if let catalogs = context.providerAgentCatalogs { return catalogs[provider] }
    return context.agentCatalog
  }
  /// Plan sizes the account can pick (empty from older servers).
  public var planSizes: [CloudSizeOption] { computers?.computerSizes ?? [] }
  /// Sizes to offer: plan sizes, or the provider's own when there are none.
  public func sizeChoices(provider: String) -> [CloudSizeOption] {
    if !planSizes.isEmpty { return planSizes }
    let row = computers?.providers.first { $0.id == provider }
    return (row?.sizes ?? []).filter {
      !$0.id.trimmingCharacters(in: .whitespaces).isEmpty && $0.vcpus > 0 && $0.memoryMib > 0
        && $0.diskGib > 0
    }.map {
      CloudSizeOption(
        id: $0.id, label: $0.label, vcpus: $0.vcpus, memoryMib: $0.memoryMib, machineRate: nil,
        available: true, requiresPlan: nil)
    }
  }
  /// The size to send: the preferred one while allowed, else `default`, else the first allowed.
  public func size(provider: String, preferred: String?) -> Result<String, ClientError> {
    let choices = sizeChoices(provider: provider)
    if let preferred, let choice = choices.first(where: { $0.id == preferred }) {
      return choice.available
        ? .success(choice.id)
        : .failure(
          ClientError(
            "\(choice.label) needs a different plan. Choose another size or upgrade in Billing.",
            code: "cloud_size_unavailable"))
    }
    let allowed = choices.filter(\.available)
    if let choice = allowed.first(where: { $0.id == "default" }) ?? allowed.first {
      return .success(choice.id)
    }
    return .failure(
      ClientError(
        "No cloud computer size is available right now. Try again shortly.",
        code: "cloud_size_unavailable"))
  }
  /// Whether launches use server-admitted first messages (`commandAdmissionVersion` 2 for both).
  public func serverAdmission(provider: String) -> Bool {
    context.commandAdmissionVersion == 2
      && (computers?.providers.contains { $0.id == provider && $0.commandAdmissionVersion == 2 }
        ?? false)
  }
  /// Whether the server gates launches on an agent connection.
  public var requiresConnection: Bool { context.agentConnections != nil }
  /// The usable connection for an agent harness: the workspace's own first, then personal,
  /// each in stable id order (the wire format has no default flag).
  public func connection(forHarness harness: String) -> CloudAgentConnection? {
    guard let agent = CloudAgent(harness: harness) else { return nil }
    let organization = context.ownerId?.hasPrefix("organization:") == true
    return (context.agentConnections ?? []).filter {
      $0.agent == agent.rawValue && $0.isConnected && !$0.connectionId.isEmpty
        && $0.connectionId.trimmingCharacters(in: .whitespaces) == $0.connectionId
    }.compactMap { row -> (Int, CloudAgentConnection)? in
      switch row.connectionScope {
      case "organization" where organization: return (0, row)
      case "personal": return (organization ? 1 : 0, row)
      case nil where !organization: return (0, row)
      default: return nil
      }
    }.min { ($0.0, $0.1.connectionId) < ($1.0, $1.1.connectionId) }?.1
  }
}

/// Readable text for the cloud API's snake_case error codes.
public enum CloudErrorText {
  public static func message(for code: String) -> String {
    switch code {
    case "agent_connection_required":
      return "Connect this agent before starting a cloud chat."
    case "agent_connection_not_found": return "That agent connection no longer exists."
    case "agent_login_not_found": return "That sign-in is no longer available. Start again."
    case "agent_connection_idempotency_conflict":
      return "A different sign-in is already in progress. Refresh and try again."
    case "cloud_agent_connection_writes_disabled":
      return "Agent connections aren't available for this account yet."
    case "invalid_agent_connection":
      return "That credential wasn't accepted. Check it and try again."
    case "invalid_device_login": return "Browser sign-in isn't available for this agent."
    case "auth_cancelled": return "Sign-in was cancelled."
    case "cloud_workspace_creation_disabled":
      return "Starting cloud chats isn't available for this account yet."
    case "cloud_provider_unavailable", "cloud_size_unavailable":
      return "Cloud computers aren't available right now. Try again shortly."
    case "runtime_agent_unsupported": return "This agent can't run in the cloud yet."
    case "organization_workspaces_disabled":
      return "Cloud isn't enabled for this organization."
    case "workspace_access_denied": return "Your role doesn't allow this. Ask an admin."
    default: return humanize(code)
    }
  }
  public static func humanize(_ code: String) -> String {
    let words = code.replacingOccurrences(of: "_", with: " ")
      .replacingOccurrences(of: "-", with: " ")
    return words.prefix(1).uppercased() + words.dropFirst()
  }
  static func reasoningLabel(_ level: String) -> String {
    switch level {
    case "xhigh": return "X-High"
    case "ultracode": return "Ultracode"
    default: return level.prefix(1).uppercased() + level.dropFirst()
    }
  }
}

extension CloudClient {
  /// One path segment: RFC 3986 unreserved characters stay, everything else is escaped.
  static func segment(_ value: String) -> String {
    let unreserved = CharacterSet(
      charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~")
    return value.addingPercentEncoding(withAllowedCharacters: unreserved) ?? ""
  }
  public func context() async throws -> CloudContext {
    try await request("/v1/cloud/context").decode(CloudContext.self)
  }
  public func capabilities() async throws -> CloudCapabilities {
    try await request("/v1/cloud/capabilities").decode(CloudCapabilities.self)
  }
  public func projects() async throws -> [CloudProject] {
    try await request("/v1/cloud/projects")["projects"].decode([CloudProject].self)
  }
  /// POST `/cloud/projects` with a saved intent's body (including its idempotency key).
  public func connectProject(body: JSONValue) async throws -> CloudProject {
    try await request("/v1/cloud/projects", method: "POST", body: body).decode(CloudProject.self)
  }
  /// POST `/cloud/workspaces` with a saved intent's body (including its idempotency key).
  public func createWorkspace(body: JSONValue) async throws -> CloudWorkspaceCreated {
    try CloudWorkspaceCreated(await request("/v1/cloud/workspaces", method: "POST", body: body))
  }
  public func agentConnections() async throws -> CloudAgentConnectionList {
    try await request("/v1/cloud/agents").decode(CloudAgentConnectionList.self)
  }
  public func configureAgent(_ request: CloudConfigureAgentRequest) async throws
    -> CloudAgentConnection
  {
    try await self.request("/v1/cloud/agents/configure", method: "POST", body: request.body)
      .decode(CloudAgentConnection.self)
  }
  public func startAgentLogin(_ request: CloudStartLoginRequest) async throws
    -> CloudLoginOperation
  {
    try await self.request("/v1/cloud/agents/login", method: "POST", body: request.body)
      .decode(CloudLoginOperation.self)
  }
  public func agentLogin(operationID: String) async throws -> CloudLoginOperation {
    try await request("/v1/cloud/agents/operations/" + Self.segment(operationID))
      .decode(CloudLoginOperation.self)
  }
  public func cancelAgentLogin(operationID: String) async throws -> CloudLoginOperation {
    try await request(
      "/v1/cloud/agents/operations/" + Self.segment(operationID), method: "DELETE"
    ).decode(CloudLoginOperation.self)
  }
  public func disconnectAgent(connectionID: String) async throws {
    _ = try await request("/v1/cloud/agents/" + Self.segment(connectionID), method: "DELETE")
  }
}
