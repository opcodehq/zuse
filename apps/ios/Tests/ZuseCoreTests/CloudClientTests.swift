import Foundation
import Testing

@testable import ZuseCore

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

private final class RequestProbe: @unchecked Sendable {
  let lock = NSLock()
  private var requests: [URLRequest] = []
  func record(_ request: URLRequest) {
    lock.lock()
    defer { lock.unlock() }
    requests.append(request)
  }
  func snapshot() -> [URLRequest] {
    lock.lock()
    defer { lock.unlock() }
    return requests
  }
}
private final class MockHTTP: URLProtocol, @unchecked Sendable {
  private final class Handlers: @unchecked Sendable {
    let lock = NSLock()
    var values: [String: @Sendable (URLRequest) -> (Int, JSONValue)] = [:]
    func set(_ host: String, _ value: (@Sendable (URLRequest) -> (Int, JSONValue))?) {
      lock.lock()
      defer { lock.unlock() }
      values[host] = value
    }
    func get(_ host: String) -> (@Sendable (URLRequest) -> (Int, JSONValue))? {
      lock.lock()
      defer { lock.unlock() }
      return values[host]
    }
  }
  private static let handlers = Handlers()
  static func configure(_ host: String, handler: (@Sendable (URLRequest) -> (Int, JSONValue))?) {
    handlers.set(host, handler)
  }
  override class func canInit(with request: URLRequest) -> Bool { true }
  override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
  override func startLoading() {
    guard let url = request.url, let host = url.host, let handler = Self.handlers.get(host) else {
      client?.urlProtocol(self, didFailWithError: ClientError("No fixture handler"))
      return
    }
    let (status, value) = handler(request)
    let response = HTTPURLResponse(
      url: url, statusCode: status, httpVersion: "HTTP/1.1",
      headerFields: ["Content-Type": "application/json"])!
    client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
    client?.urlProtocol(self, didLoad: (try? JSONEncoder().encode(value)) ?? Data())
    client?.urlProtocolDidFinishLoading(self)
  }
  override func stopLoading() {}
}
private func fixture(
  organization: String? = nil, _ handler: @escaping @Sendable (URLRequest) -> (Int, JSONValue)
) -> (CloudClient, URLSession, String) {
  let host = UUID().uuidString.lowercased() + ".test"
  MockHTTP.configure(host, handler: handler)
  let configuration = URLSessionConfiguration.ephemeral
  configuration.protocolClasses = [MockHTTP.self]
  let session = URLSession(configuration: configuration)
  return (
    CloudClient(
      baseURL: URL(string: "https://" + host)!, organizationID: organization, session: session
    ) { "access-token" },
    session, host
  )
}
private func body(_ request: URLRequest) throws -> JSONValue {
  var bytes = request.httpBody
  if bytes == nil, let stream = request.httpBodyStream {
    stream.open()
    defer { stream.close() }
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 4096)
    while true {
      let count = stream.read(&buffer, maxLength: buffer.count)
      if count <= 0 { break }
      data.append(contentsOf: buffer.prefix(count))
    }
    bytes = data
  }
  return try JSONDecoder().decode(JSONValue.self, from: #require(bytes))
}

@Test func cloudProofBindsTheDeviceAuthorizationToken() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .object([:]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  let proof = try DeviceProof(rawKey: DeviceProof.generateKey())
  _ = try await client.request(
    "/v1/environments/host/connect", method: "POST", proof: proof, deviceToken: "device-access")
  let request = try #require(probe.snapshot().first)
  #expect(request.value(forHTTPHeaderField: "Authorization") == "DPoP device-access")
  let actual = try #require(request.value(forHTTPHeaderField: "DPoP"))
  let expected = try proof.sign(method: "POST", url: request.url!, accessToken: "device-access")
  func claims(_ jwt: String) throws -> JSONValue {
    var encoded = String(jwt.split(separator: ".")[1]).replacingOccurrences(of: "-", with: "+")
      .replacingOccurrences(of: "_", with: "/")
    encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
    return try JSONDecoder().decode(JSONValue.self, from: #require(Data(base64Encoded: encoded)))
  }
  #expect(try claims(actual)["ath"] == claims(expected)["ath"])
  #expect(try claims(actual)["ath"].string != nil)
}

@Test func cloudConnectionWakesPausedWorkspaceWithoutReplacingIt() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (
      200,
      request.httpMethod == "POST"
        ? .object(["workspaceId": .string("workspace/one")]) : .object(["state": .string("paused")])
    )
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  try await client.prepareConnection(workspaceID: "workspace/one")
  let requests = probe.snapshot()
  #expect(requests.count == 2)
  #expect(requests[0].httpMethod == "GET")
  #expect(requests[1].url?.absoluteString.contains("workspace%2Fone/resume") == true)
  #expect(requests[1].httpMethod == "POST")
  #expect(
    requests.allSatisfy { $0.value(forHTTPHeaderField: "Authorization") == "Bearer access-token" })
}

@Test func cloudReadDoesNotResumeAnAlreadyReadyWorkspace() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .object(["state": .string("ready")]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  try await client.prepareConnection(workspaceID: "workspace")
  #expect(probe.snapshot().count == 1)
}

@Test func cloudAccessErrorsPreserveActionableDisposition() async throws {
  let (client, session, host) = fixture { _ in
    (403, .object(["error": .string("cloud_entitlement_required")]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  do {
    _ = try await client.request("/v1/cloud/projects")
    Issue.record("Expected access failure")
  } catch let error as ClientError {
    #expect(error.requiresUserAction)
    #expect(error.message == "Cloud entitlement required")
  }
}

@Test func cloudCredentialsCannotBeSentToAnotherOrigin() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .null)
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  await #expect(throws: ClientError.self) {
    _ = try await client.request("https://other.test/v1/cloud/projects")
  }
  await #expect(throws: ClientError.self) {
    _ = try await client.request("http://" + host + "/v1/cloud/projects")
  }
  #expect(probe.snapshot().isEmpty)
}

@Test func reconnectDoesNotUndoAPausedWorkspace() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .object(["state": .string("paused"), "desiredState": .string("paused")]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  do {
    try await client.prepareConnection(workspaceID: "workspace", wakeIfPaused: false)
    Issue.record("Automatic reconnection must respect pause")
  } catch let error as ClientError {
    #expect(error.code == "workspace-paused")
    #expect(error.requiresUserAction)
  }
  #expect(probe.snapshot().map(\.httpMethod) == ["GET"])
}

@Test func reconnectWaitsForAnAlreadyRequestedWakeWithoutSubmittingAnother() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .object(["state": .string("paused"), "desiredState": .string("ready")]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  try await client.prepareConnection(workspaceID: "workspace", wakeIfPaused: false)
  #expect(probe.snapshot().count == 1)
}

@Test func cloudLifecycleRequestsKeepTheirOriginalIdentityAndResource() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .object(["workspaceId": .string("workspace/one")]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  for action in CloudWorkspaceAction.allCases {
    _ = try await client.control(
      workspaceID: "workspace/one", action: action, commandID: "original-" + action.rawValue)
  }
  let requests = probe.snapshot()
  #expect(requests.count == 4)
  for (request, action) in zip(requests, CloudWorkspaceAction.allCases) {
    #expect(request.httpMethod == "POST")
    #expect(request.url?.absoluteString.contains("workspace%2Fone/" + action.rawValue) == true)
    var bytes = request.httpBody
    if bytes == nil, let stream = request.httpBodyStream {
      stream.open()
      defer { stream.close() }
      var buffer = [UInt8](repeating: 0, count: 4096)
      let count = stream.read(&buffer, maxLength: buffer.count)
      if count > 0 { bytes = Data(buffer.prefix(count)) }
    }
    let body = try JSONDecoder().decode(JSONValue.self, from: #require(bytes))
    #expect(body["workspaceId"].string == "workspace/one")
    #expect(body["commandId"].string == "original-" + action.rawValue)
  }
}

@Test func archivedCatalogCanBeRequestedExplicitly() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (200, .object(["workspaces": .array([])]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  _ = try await client.chats(scope: .all)
  #expect(probe.snapshot().first?.url?.path == "/v1/cloud/workspaces")
}

@Test func rustWorkspaceCatalogSeparatesActiveAndArchivedChats() async throws {
  let (client, session, host) = fixture { _ in
    let rows: [JSONValue] = ["ready", "archived", "deleted"].map { state in
      .object([
        "workspaceId": .string(state), "projectId": .string("project"), "chatId": .string("thread"),
        "initialSessionId": .string("session"), "state": .string(state),
      ])
    }
    return (200, .object(["workspaces": .array(rows)]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  #expect(try await client.chats().map(\.workspaceId) == ["ready"])
  #expect(try await client.chats(scope: .archived).map(\.workspaceId) == ["archived"])
  #expect(try await client.chats(scope: .all).map(\.workspaceId) == ["ready", "archived"])
  #expect(try await client.chats().first?.sessionID == "session")
}

@Test func lifecyclePresentationDistinguishesProgressAndAvailableActions() throws {
  func chat(_ state: String, desired: String?) throws -> CloudChat {
    try JSONValue.object([
      "workspaceId": .string("w"), "projectId": .string("p"), "chatId": .string("c"),
      "initialSessionId": .string("s"), "title": .string("Chat"),
      "repositoryDisplayName": .string("Repo"),
      "state": .string(state), "desiredState": desired.map(JSONValue.string) ?? .null,
      "runtimeState": .string("offline"),
    ]).decode(CloudChat.self)
  }
  #expect(try chat("ready", desired: "ready").availableActions == [.pause, .archive])
  #expect(try chat("paused", desired: "paused").availableActions == [.resume, .archive])
  #expect(try chat("archived", desired: "archived").availableActions == [.unarchive])
  let waking = try chat("paused", desired: "ready")
  #expect(
    waking.isTransitioning && waking.statusLabel == "Waking" && waking.availableActions.isEmpty)
  let archiving = try chat("paused", desired: "archived")
  #expect(archiving.isArchived && archiving.isTransitioning && archiving.statusLabel == "Archiving")
}

@Test func cloudActionCannotConfirmADifferentWorkspace() async throws {
  let (client, session, host) = fixture { _ in
    (200, .object(["workspaceId": .string("another-workspace")]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  await #expect(throws: ClientError.self) {
    _ = try await client.control(
      workspaceID: "selected-workspace", action: .pause, commandID: "original-pause")
  }
}

@Test func retiredAccountCredentialsPreventTheRequestFromBeingSent() async throws {
  let probe = RequestProbe()
  let (base, session, host) = fixture { request in
    probe.record(request)
    return (200, .null)
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  let retired = CloudClient(baseURL: base.baseURL, session: session) { throw CancellationError() }
  await #expect(throws: CancellationError.self) {
    _ = try await retired.request(
      "/v1/cloud/agents/configure", method: "POST",
      body: .object(["secret": .string("private-key")]))
  }
  #expect(probe.snapshot().isEmpty)
}

@Test func organizationScopeIsSentOnlyToCloudRoutes() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture(organization: "org_team") { request in
    probe.record(request)
    return (200, .object(["workspaces": .array([]), "environments": .array([])]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  _ = try await client.chats()
  _ = try await client.environments()
  let requests = probe.snapshot()
  #expect(requests[0].value(forHTTPHeaderField: "x-zuse-workspace") == "organization:org_team")
  // Personal computers refuse an organization scope (`environment_scope_not_supported`).
  #expect(requests[1].value(forHTTPHeaderField: "x-zuse-workspace") == nil)
  let (personal, personalSession, personalHost) = fixture { request in
    probe.record(request)
    return (200, .object(["workspaces": .array([])]))
  }
  defer {
    personalSession.invalidateAndCancel()
    MockHTTP.configure(personalHost, handler: nil)
  }
  _ = try await personal.chats()
  #expect(probe.snapshot().last?.value(forHTTPHeaderField: "x-zuse-workspace") == nil)
}

@Test func agentConnectionRoutesUseConnectionAndOperationIdentity() async throws {
  let probe = RequestProbe()
  let operation: JSONValue = .object([
    "operationId": .string("auth_op/1"), "connectionId": .string("conn_1"),
    "agent": .string("codex"), "state": .string("authorizing"),
    "verificationUrl": .string("https://auth.example/device"),
    "verificationCode": .string("ABCD-EFGH"), "expiresAt": .number(1_900_000_000_000),
    "errorCode": .null,
  ])
  let (client, session, host) = fixture { request in
    probe.record(request)
    switch (request.httpMethod ?? "", request.url?.path ?? "") {
    case ("POST", "/v1/cloud/agents/configure"):
      return (
        200,
        .object([
          "connectionId": .string("conn_2"), "agent": .string("claude"),
          "method": .string("setup-token"), "state": .string("connected"),
          "accountLabel": .null, "expiresAt": .null, "version": .number(1), "errorCode": .null,
        ])
      )
    case ("DELETE", "/v1/cloud/agents/conn_1"): return (200, .object(["ok": .bool(true)]))
    default: return (200, operation)
    }
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  let started = try await client.startAgentLogin(
    CloudStartLoginRequest(agent: .codex, idempotencyKey: "key-1"))
  #expect(started.isPending && started.verificationCode == "ABCD-EFGH")
  _ = try await client.agentLogin(operationID: "auth_op/1")
  _ = try await client.cancelAgentLogin(operationID: "auth_op/1")
  let configured = try await client.configureAgent(
    CloudConfigureAgentRequest(
      agent: .claude, method: .setupToken, secret: "sk-ant-oat-secret", idempotencyKey: "key-2"))
  #expect(configured.isConnected)
  try await client.disconnectAgent(connectionID: "conn_1")
  let requests = probe.snapshot()
  #expect(requests.map { $0.httpMethod ?? "" } == ["POST", "GET", "DELETE", "POST", "DELETE"])
  #expect(requests[0].url?.path == "/v1/cloud/agents/login")
  #expect(
    try body(requests[0])
      == .object(["agent": .string("codex"), "idempotencyKey": .string("key-1")]))
  for request in requests[1...2] {
    #expect(
      request.url?.absoluteString.hasSuffix("/v1/cloud/agents/operations/auth_op%2F1") == true)
  }
  #expect(
    try body(requests[3])
      == .object([
        "agent": .string("claude"), "method": .string("setup-token"),
        "secret": .string("sk-ant-oat-secret"), "idempotencyKey": .string("key-2"),
      ]))
  #expect(requests[4].url?.path == "/v1/cloud/agents/conn_1")
}

private final class Counter: @unchecked Sendable {
  private let lock = NSLock()
  private var value = 0
  func next() -> Int {
    lock.lock()
    defer { lock.unlock() }
    value += 1
    return value - 1
  }
}

@Test func connectionWaitsForTheRuntimeAndStopsOnArchivedOrFailedWorkspaces() async throws {
  let probe = RequestProbe()
  let reads = Counter()
  let (client, session, host) = fixture { request in
    probe.record(request)
    let state = reads.next() < 2 ? "provisioning" : "ready"
    return (200, .object(["workspaceId": .string("w"), "state": .string(state)]))
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  let ready = try await client.waitForRuntime(
    workspaceID: "w", prepared: .object(["state": .string("queued")]), interval: .milliseconds(1))
  #expect(ready["state"].string == "ready")
  #expect(probe.snapshot().count == 3)
  // Runtime ingress being up is enough, as on desktop.
  let early = try await client.waitForRuntime(
    workspaceID: "w",
    prepared: .object(["state": .string("setup"), "runtimeIngressReady": .bool(true)]))
  #expect(early["state"].string == "setup")
  let stopped: [JSONValue] = [
    .object(["state": .string("archived")]),
    .object(["state": .string("ready"), "desiredState": .string("archived")]),
    .object(["state": .string("failed"), "statusCode": .string("environment_setup_failed")]),
  ]
  for prepared in stopped {
    await #expect(throws: ClientError.self) {
      _ = try await client.waitForRuntime(workspaceID: "w", prepared: prepared)
    }
  }
  #expect(probe.snapshot().count == 3)
}

@Test func workspaceCreationSendsTheSavedBodyAndReadsBothIdentities() async throws {
  let probe = RequestProbe()
  let (client, session, host) = fixture { request in
    probe.record(request)
    return (
      200,
      .object([
        "workspace": .object([
          "workspaceId": .string("ws_1"), "chatId": .string("chat_1"),
          "initialSessionId": .string("session_1"),
        ]),
        "chatId": .string("chat_1"), "initialSessionId": .string("session_1"),
        "initialMessageDelivery": .string("server-admitted-v2"),
      ])
    )
  }
  defer {
    session.invalidateAndCancel()
    MockHTTP.configure(host, handler: nil)
  }
  let request = CloudWorkspaceCreateRequest(
    projectId: "proj", providerId: "e2b", baseRef: "refs/heads/main", agent: "claude-code",
    model: "claude-sonnet-5", agentConnectionId: "conn_1", sizeId: "default",
    firstMessage: "Fix the build",
    initialMessageDelivery: CloudWorkspaceCreateRequest.serverAdmitted)
  let created = try await client.createWorkspace(
    body: request.body.setting("idempotencyKey", to: .string("stable-key")))
  #expect(created.workspaceId == "ws_1" && created.initialSessionId == "session_1")
  #expect(created.chatId == "chat_1" && created.initialMessageDelivery == "server-admitted-v2")
  let sent = try body(#require(probe.snapshot().first))
  #expect(sent["idempotencyKey"].string == "stable-key")
  #expect(sent["agentConnectionId"].string == "conn_1")
  #expect(probe.snapshot().first?.url?.path == "/v1/cloud/workspaces")
}
