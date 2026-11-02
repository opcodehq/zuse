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
private func fixture(_ handler: @escaping @Sendable (URLRequest) -> (Int, JSONValue)) -> (
  CloudClient, URLSession, String
) {
  let host = UUID().uuidString.lowercased() + ".test"
  MockHTTP.configure(host, handler: handler)
  let configuration = URLSessionConfiguration.ephemeral
  configuration.protocolClasses = [MockHTTP.self]
  let session = URLSession(configuration: configuration)
  return (
    CloudClient(baseURL: URL(string: "https://" + host)!, session: session) { "access-token" },
    session, host
  )
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
    #expect(error.message == "Cloud Entitlement Required")
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
      "/v1/cloud/provider-connections", method: "POST",
      body: .object(["apiKey": .string("private-key")]))
  }
  #expect(probe.snapshot().isEmpty)
}
