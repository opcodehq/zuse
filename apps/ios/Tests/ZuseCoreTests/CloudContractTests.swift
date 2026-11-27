import Foundation
import Testing

@testable import ZuseCore

// Fixtures follow the Rust serializers: `routes.rs::context` and `agent_connections`,
// `cloud_computers::CloudCapabilities`, `cloud_connections::{AgentConnection, LoginOperation}`.

private func json(_ text: String) throws -> JSONValue {
  try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
}

private let capabilities = #"""
  {"providers":[
    {"id":"boxd","configured":true,"managedBaseReady":false,"commandAdmissionVersion":2,
     "preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,
     "sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],
     "unavailableReason":"Not enabled for this account."},
    {"id":"e2b","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,
     "preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,
     "sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],
     "unavailableReason":null}],
   "canConfigure":true,"canAllocate":true,"allocationBlockers":[],
   "computerSizes":[
    {"id":"default","label":"Standard · 4 vCPU · 16 GB","vcpus":4,"memoryMib":16384,"machineRate":4,"available":true},
    {"id":"large","label":"Large · 8 vCPU · 32 GB","vcpus":8,"memoryMib":32768,"machineRate":8,"available":false,"requiresPlan":"cloud-max"}]}
  """#

private let catalog = #"""
  {"providers":[
    {"id":"claude-code","name":"Claude","defaultModel":"sonnet","models":[
      {"id":"claude-sonnet-5","label":"Sonnet 5","reasoningLevels":["low","medium","high","ultrathink"],"options":[]},
      {"id":"claude-haiku-5","label":"Haiku 5"}]},
    {"id":"codex","name":"Codex","defaultModel":"gpt-codex","models":[
      {"id":"gpt-codex","label":"Codex","reasoningLevels":["low","medium","high"],"options":[]}]}]}
  """#

private func connection(_ id: String, agent: String, state: String = "connected", scope: String?)
  -> String
{
  let tag = scope.map { #","connectionScope":"\#($0)""# } ?? ""
  return
    #"{"connectionId":"\#(id)","agent":"\#(agent)","method":"api-key","state":"\#(state)","accountLabel":null,"expiresAt":null,"version":1,"errorCode":null\#(tag)}"#
}

private func context(
  owner: String = "user_1", connections: [String]? = nil, admission: Int? = 2,
  defaultProvider: String? = nil
) throws -> CloudContext {
  var computers = try json(capabilities)
  if let defaultProvider {
    computers = computers.setting("defaultProvider", to: .string(defaultProvider))
  }
  let rows = connections.map { "[" + $0.joined(separator: ",") + "]" } ?? "null"
  let text = #"""
    {"actor":{"accountId":"user_1"},"scope":{"kind":"personal"},"ownerId":"\#(owner)",
     "accountEvents":true,
     "projects":[{"projectId":"proj_1","repositoryIdentity":"github.com/acme/web",
       "repositoryUrl":"https://github.com/acme/web","displayName":"acme/web","defaultBranch":"trunk",
       "visibility":"private","state":"ready","activeBuilds":{},"latestBuilds":{},"createdAt":1,"updatedAt":2},
      {"projectId":"proj_2","displayName":"","defaultBranch":"","repositoryIdentity":"github.com/acme/api"}],
     "agentCatalog":\#(catalog),"providerAgentCatalogs":{"e2b":\#(catalog),"boxd":\#(catalog)},
     "computerCapabilities":\#(String(decoding: try JSONEncoder().encode(computers), as: UTF8.self)),
     "commandAdmissionVersion":\#(admission.map(String.init) ?? "null"),
     "launchAttachmentHoldVersion":null,"agentConnections":\#(rows)}
    """#
  return try json(text).decode(CloudContext.self)
}

@Suite struct CloudContractTests {
  @Test func contextDecodesProjectsCatalogAndConnections() throws {
    let value = try context(connections: [connection("conn_a", agent: "codex", scope: "personal")])
    #expect(value.ownerId == "user_1")
    #expect(value.projects.map(\.projectId) == ["proj_1", "proj_2"])
    #expect(value.projects[0].defaultBranch == "trunk")
    // Missing names and branches fall back rather than failing the whole context.
    #expect(value.projects[1].displayName == "github.com/acme/api")
    #expect(value.projects[1].defaultBranch == "main")
    #expect(value.agentCatalog?.providers.map(\.id) == ["claude-code", "codex"])
    #expect(value.providerAgentCatalogs?["e2b"] != nil)
    #expect(value.computerCapabilities?.providers.map(\.id) == ["boxd", "e2b"])
    #expect(value.agentConnections?.first?.connectionScope == "personal")
    #expect(value.commandAdmissionVersion == 2)
    #expect(try context(connections: nil).agentConnections == nil)
  }

  @Test func capabilitiesUseProviderIDsAndPlanSizes() throws {
    let value = try json(capabilities).decode(CloudCapabilities.self)
    #expect(value.providers.map(\.id) == ["boxd", "e2b"])
    #expect(value.providers.map(\.canCreate) == [false, true])
    #expect(value.computerSizes?.map(\.id) == ["default", "large"])
    #expect(value.computerSizes?[1].requiresPlan == "cloud-max")
    #expect(value.canAllocate == true && value.defaultProvider == nil)
  }

  @Test func agentsMapToTheirHarnesses() throws {
    let value = try json(connection("conn_a", agent: "claude", state: "connected", scope: nil))
      .decode(CloudAgentConnection.self)
    #expect(value.cloudAgent == .claude && value.isConnected)
    #expect(CloudAgent(harness: "claude-code") == .claude)
    #expect(CloudAgent.claude.harness == "claude-code")
    #expect(CloudAgent(harness: "cursor") == nil)
  }

  @Test func projectConnectRequestsEncodeOnlyAcceptedFields() {
    let project = CloudProjectConnectRequest(
      repositoryUrl: " https://github.com/acme/web ", defaultBranch: "main", isPrivate: false)
    #expect(project.body["repositoryUrl"].string == "https://github.com/acme/web")
    #expect(project.body["visibility"].string == "public")
  }

  @Test func workspaceCreateRequestMatchesTheDenyUnknownFieldsContract() throws {
    let request = CloudWorkspaceCreateRequest(
      projectId: "proj_1", providerId: "e2b",
      baseRef: CloudWorkspaceCreateRequest.baseRef(
        branch: "main"),
      agent: "claude-code", model: "claude-sonnet-5", agentConnectionId: "conn_a",
      sizeId: "default", reasoning: "high", firstMessage: "Hello",
      initialMessageDelivery: CloudWorkspaceCreateRequest.serverAdmitted)
    let body = request.body
    guard case .object(let fields) = body else {
      Issue.record("Expected an object")
      return
    }
    #expect(Set(fields.keys).isSubset(of: CloudWorkspaceCreateRequest.acceptedFields))
    #expect(body["baseRef"].string == "refs/heads/main")
    #expect(body["runtimeMode"].string == "approval-required")
    #expect(body["initialMessageDelivery"].string == "server-admitted-v2")
    #expect(body["agentConnectionId"].string == "conn_a")
    #expect(body["idempotencyKey"] == .null)
    // Optional fields are omitted, never sent as null.
    let minimal = CloudWorkspaceCreateRequest(
      projectId: "p", providerId: "boxd", baseRef: "refs/heads/main", agent: "codex", model: "m"
    ).body
    #expect(minimal["sizeId"] == .null && minimal["agentConnectionId"] == .null)
    guard case .object(let minimalFields) = minimal else { return }
    #expect(minimalFields.count == 6)
    // A saved body round-trips; one with retired fields is refused.
    let saved = body.setting("idempotencyKey", to: .string("key"))
    #expect(CloudWorkspaceCreateRequest(body: saved) == request)
    #expect(CloudWorkspaceCreateRequest(body: body.setting("sealedSecret", to: .null)) == nil)
    #expect(CloudWorkspaceCreateRequest.baseRef(branch: "refs/tags/v1") == "refs/tags/v1")
  }

  @Test func createdResponseNeedsWorkspaceAndSession() throws {
    let created = try CloudWorkspaceCreated(
      json(
        #"{"workspace":{"workspaceId":"w","chatId":"c","initialSessionId":"s"},"chatId":"c","initialSessionId":"s"}"#
      ))
    #expect(created.workspaceId == "w" && created.initialSessionId == "s")
    #expect(created.initialMessageDelivery == nil)
    #expect(throws: ClientError.self) { try CloudWorkspaceCreated(json(#"{"workspace":{}}"#)) }
  }

  @Test func launchPicksAReadyProviderAndPlanSize() throws {
    let options = CloudLaunchOptions(context: try context())
    // boxd isn't ready, so the ready provider wins even without a default.
    #expect(try options.provider().get() == "e2b")
    #expect(
      try CloudLaunchOptions(context: context(defaultProvider: "boxd")).provider().get() == "e2b")
    #expect(try options.size(provider: "e2b", preferred: nil).get() == "default")
    #expect(try options.size(provider: "e2b", preferred: "missing").get() == "default")
    #expect(throws: ClientError.self) {
      try options.size(provider: "e2b", preferred: "large").get()
    }
    #expect(options.serverAdmission(provider: "e2b"))
    #expect(
      !CloudLaunchOptions(context: try context(admission: nil)).serverAdmission(provider: "e2b"))
    #expect(options.allocationProblem == nil)
    let blocked = try json(capabilities).setting("canAllocate", to: .bool(false)).setting(
      "allocationBlockers", to: .array([.string("cloud_workspace_creation_disabled")])
    ).decode(CloudCapabilities.self)
    #expect(
      CloudLaunchOptions(context: try context(), capabilities: blocked).allocationProblem
        == "Starting cloud chats isn't available for this account yet.")
  }

  @Test func launchWithoutReadyProvidersExplainsWhy() throws {
    let none = try json(capabilities).setting(
      "providers",
      to: .array([try json(capabilities)["providers"].array[0]])
    ).decode(CloudCapabilities.self)
    let options = CloudLaunchOptions(context: try context(), capabilities: none)
    #expect(throws: ClientError.self) { try options.provider().get() }
    if case .failure(let error) = options.provider() {
      #expect(error.message == "Not enabled for this account.")
    }
  }

  @Test func connectionChoiceIsScopedAndStable() throws {
    let personal = CloudLaunchOptions(
      context: try context(connections: [
        connection("z", agent: "codex", scope: "personal"),
        connection("b", agent: "codex", scope: "personal"),
        connection("r", agent: "claude", state: "revoked", scope: "personal"),
      ]))
    #expect(personal.requiresConnection)
    #expect(personal.connection(forHarness: "codex")?.connectionId == "b")
    #expect(personal.connection(forHarness: "claude-code") == nil)
    #expect(personal.connection(forHarness: "cursor") == nil)
    let team = CloudLaunchOptions(
      context: try context(
        owner: "organization:org_1",
        connections: [
          connection("a-personal", agent: "codex", scope: "personal"),
          connection("z-team", agent: "codex", scope: "organization"),
          connection("unlabeled", agent: "grok", scope: nil),
        ]))
    #expect(team.connection(forHarness: "codex")?.connectionId == "z-team")
    // Unlabeled rows only count in a personal context.
    #expect(team.connection(forHarness: "grok") == nil)
    #expect(!CloudLaunchOptions(context: try context(connections: nil)).requiresConnection)
  }

  @Test func cloudCatalogBecomesPickerProviders() throws {
    let providers = ModelProvider.cloud(try json(catalog).decode(CloudAgentCatalog.self))
    #expect(providers.map(\.id) == ["claude-code", "codex"])
    // The `sonnet` alias resolves to the family's model, as `resolve_model` does.
    #expect(providers[0].defaultModel == "claude-sonnet-5")
    let reasoning = try #require(providers[0].models[0].reasoning)
    // Prompt-prefix levels can't be sent as `reasoning`, so they aren't offered.
    #expect(reasoning.options?.map(\.id) == ["low", "medium", "high"])
    #expect(reasoning.defaultId == "medium")
    #expect(providers[0].models[1].reasoning == nil)
  }

  @Test func shippedClaudeCatalogDecodes() throws {
    // The managed base catalog the server serves (`infra/cloud/managed-base-claude-catalog.json`).
    let root = URL(fileURLWithPath: #filePath).resolvingSymlinksInPath()
      .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
      .deletingLastPathComponent().deletingLastPathComponent()
    let file = root.appendingPathComponent("infra/cloud/managed-base-claude-catalog.json")
    guard let data = try? Data(contentsOf: file) else { return }
    let agent = try JSONDecoder().decode(CloudAgentCatalog.Agent.self, from: data)
    let providers = ModelProvider.cloud(CloudAgentCatalog(providers: [agent]))
    #expect(providers.first?.id == "claude-code")
    #expect(providers.first?.models.contains { $0.id == providers.first?.defaultModel } == true)
  }

  @Test func errorCodesReadAsSentences() {
    #expect(
      CloudErrorText.message(for: "agent_connection_required")
        == "Connect this agent before starting a cloud chat.")
    #expect(CloudErrorText.message(for: "something_new") == "Something new")
  }
}
