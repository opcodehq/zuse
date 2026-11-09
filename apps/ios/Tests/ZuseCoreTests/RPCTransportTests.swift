import Foundation
import Testing

@testable import ZuseCore

/// Only socket I/O is replaced. Production serialization, request routing, cancellation,
/// timers, gateway handling, backpressure and attachment code all run unchanged.
private actor CanonicalUploadState {
  private var bytes = Data()
  private var sequence = 0
  private var chats: [String: JSONValue] = [:]
  private var createdBranches = Set<String>()
  func respond(_ frame: JSONValue) throws -> JSONValue? {
    let params = frame["params"]
    switch frame["method"].string {
    case "UploadChunk":
      guard params["seq"].int == sequence, let encoded = params["data"].string,
        let data = Data(base64Encoded: encoded)
      else { throw ClientError("Invalid upload sequence") }
      bytes.append(data)
      sequence += 1
      return .object(["ok": .bool(true)])
    case "UploadCommit": return .object(["path": .string("/host/uploads/test.bin")])
    case "ReadAttachmentChunk":
      let start = params["offset"].int ?? 0
      let end = min(start + 45 * 1024, bytes.count)
      return .object([
        "data": .string(bytes.subdata(in: start..<end).base64EncodedString()),
        "nextOffset": .number(Double(end)), "done": .bool(end == bytes.count),
        "mimeType": .string("application/octet-stream"), "name": .string("test.bin"),
      ])
    case "ListRefs":
      return .array([
        .object(["name": .string("main"), "current": .bool(true)]),
        .object([
          "name": .string("topic"), "current": .bool(false),
          "worktreePath": .string("/host/worktrees/topic"),
        ]),
      ])
    case "CreateWorktree":
      guard createdBranches.insert(params["branch"].string ?? "").inserted else {
        throw ClientError("Worktree already exists")
      }
      return .object([
        "worktree": .object(["path": .string("/host/worktrees/fresh"), "branch": params["branch"]])
      ])
    case "WatchChats":
      return .array(chats.values.sorted { ($0["id"].string ?? "") < ($1["id"].string ?? "") })
    case "Mutate":
      if params["op"].string == "createChat", let id = params["chatId"].string {
        chats[id] = .object([
          "id": .string(id), "spaceId": params["spaceId"], "deviceId": params["deviceId"],
          "cwd": params["cwd"], "branch": params["branch"], "config": params["config"],
        ])
      }
      return .object(["ok": .bool(true)])
    case "QueueCommand": return .object(["commandId": .string("queued")])
    default: return nil
    }
  }
}

private struct MemoryRPC: Sendable {
  private let canonicalUploads = CanonicalUploadState()
  let incoming = AsyncThrowingStream<Data, Error>.makeStream()
  let outgoing = AsyncStream<JSONValue>.makeStream()
  var handshakeVersion = RPCClient.wireVersion
  var transport: RPCTransport {
    let incoming = incoming
    let outgoing = outgoing
    let version = handshakeVersion
    let canonicalUploads = canonicalUploads
    return RPCTransport(
      start: {},
      receive: {
        var iterator = incoming.stream.makeAsyncIterator()
        guard let bytes = try await iterator.next() else { throw CancellationError() }
        return bytes
      },
      send: { bytes in
        try Task.checkCancellation()
        let frame = try JSONDecoder().decode(JSONValue.self, from: bytes)
        outgoing.continuation.yield(frame)
        if let result = try await canonicalUploads.respond(frame) {
          incoming.continuation.yield(
            try JSONEncoder().encode(
              JSONValue.object([
                "id": frame["id"], frame["method"].string == "WatchChats" ? "item" : "ok": result,
              ])))
        }
        if frame["method"].string == "EngineInfo" {
          incoming.continuation.yield(
            try JSONEncoder().encode(
              JSONValue.object(["id": frame["id"], "ok": .object(["deviceId": .string("host")])])))
        }
        if frame["method"].string == "WatchSpaces" {
          incoming.continuation.yield(
            try JSONEncoder().encode(
              JSONValue.object([
                "id": frame["id"],
                "item": .array([.object(["id": .string("p"), "path": .string("/host/project")])]),
              ])))
        }
        if frame["method"].string == "ListHarnesses" {
          incoming.continuation.yield(
            try JSONEncoder().encode(
              JSONValue.object([
                "id": frame["id"],
                "ok": .array([
                  .object([
                    "id": .string("codex"), "name": .string("Codex"), "installed": .bool(true),
                  ])
                ]),
              ])))
        }
        if frame["method"].string == "ListModels" {
          incoming.continuation.yield(
            try JSONEncoder().encode(
              JSONValue.object([
                "id": frame["id"],
                "ok": .array([.object(["id": .string("model"), "label": .string("Model")])]),
              ])))
        }
        if frame["tag"].string == "connect.handshake" {
          incoming.continuation.yield(
            try JSONEncoder().encode(
              Self.success(
                frame,
                value: .object([
                  "protocolVersion": .number(Double(version))
                ]))))
        }
      },
      close: {
        incoming.continuation.finish()
        outgoing.continuation.yield(.object(["_tag": .string("TransportClosed")]))
      })
  }
  static func success(_ request: JSONValue, value: JSONValue = .null) -> JSONValue {
    .object([
      "_tag": .string("Exit"), "requestId": request["id"],
      "exit": .object(["_tag": .string("Success"), "value": value]),
    ])
  }
  func push(_ frame: JSONValue) throws {
    incoming.continuation.yield(try JSONEncoder().encode(frame))
  }
  func next(tag: String) async throws -> JSONValue {
    for await frame in outgoing.stream {
      if frame["method"].string == tag || frame["tag"].string == tag || frame["_tag"].string == tag
      {
        return frame
      }
    }
    throw ClientError("The test socket ended before receiving " + tag)
  }
}

@Suite(.timeLimit(.minutes(1)))
struct RPCTransportTests {
  @Test func canonicalAttachmentBytesAndQuestionLabelsRoundTrip() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, canonical: true)
    try await client.connect()
    let bytes = Data((0..<130001).map { UInt8($0 % 251) })
    let ref = try await Attachments.upload(
      bytes, name: "test.bin", mimeType: "application/octet-stream", sessionID: "chat", rpc: client)
    let (received, _, name) = try await Attachments.read(
      ref["id"].string!, sessionID: "chat", rpc: client)
    #expect(received == bytes)
    #expect(name == "test.bin")
    try await CanonicalWorkspace(client).answer(
      chatID: "chat", requestID: "request",
      questions: [
        .object(["id": .string("q"), "options": .array([.string("Allow"), .string("Deny")])])
      ], answers: [.object(["questionIndex": .number(0), "selected": .array([.number(1)])])])
    let command = try await fixture.next(tag: "QueueCommand")
    #expect(command["params"]["command"]["kind"].string == "respondInput")
    #expect(
      command["params"]["command"]["answers"].array.first?["labels"] == .array([.string("Deny")]))
    await client.close()
  }

  @Test func canonicalCreationKeepsIdentityAndHonorsCheckoutChoice() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, canonical: true)
    try await client.connect()
    let workspace = CanonicalWorkspace(client)
    let (branches, trees) = try await workspace.sources(projectID: "p")
    #expect(branches.first?.current == true)
    #expect(trees.first?["id"].string == "/host/worktrees/topic")
    let main = ChatCreation(projectID: "p", providerID: "codex", model: "model")
    let created = try await workspace.create(main)
    #expect(created.id == main.chatID)
    #expect(created.projectId == "p")
    #expect(created.providerId == "codex")
    let again = try await workspace.create(main)
    #expect(again.id == created.id)
    let existing = ChatCreation(
      projectID: "p", providerID: "codex", model: "model", worktreeID: "/host/worktrees/topic")
    #expect(try await workspace.create(existing).id == existing.chatID)
    let fresh = ChatCreation(
      projectID: "p", providerID: "codex", model: "model", freshWorktree: true)
    #expect(try await workspace.create(fresh).id == fresh.chatID)
    #expect(try await workspace.create(fresh).id == fresh.chatID)
    await client.close()
  }

  @Test func canonicalWorkspaceAndCatalogUseHostProjections() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, canonical: true)
    try await client.connect()
    let projects = try await CanonicalWorkspace(client).projects()
    #expect(projects.first?.name == "project")
    #expect(projects.last?.id == CanonicalWorkspace.projectless)
    let providers = try await ModelProvider.load(client)
    #expect(providers.first?.id == "codex")
    #expect(providers.first?.models.first?.id == "model")
    await client.close()
  }

  @Test func canonicalNullReplyUsesNumericRequestAndCanonicalHandshake() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, canonical: true)
    try await client.connect()
    let handshake = try await fixture.next(tag: "EngineInfo")
    #expect(handshake["id"].int == 1)
    let pending = Task { try await client.call("Null", ["value": .string("payload")]) }
    let request = try await fixture.next(tag: "Null")
    #expect(request["id"].int == 2)
    #expect(request["params"]["value"].string == "payload")
    try fixture.push(.object(["id": request["id"], "ok": .null]))
    #expect(try await pending.value == .null)
    await client.close()
  }

  @Test func requestsRequireAnOpenedConnection() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport)
    await #expect(throws: ClientError.self) { _ = try await client.call("messages.send") }
    try await client.connect()
    await client.close()
  }

  @Test func socketFailureSettlesEveryOutstandingRequest() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport)
    try await client.connect()
    let first = Task { try await client.call("first") }
    _ = try await fixture.next(tag: "first")
    let second = Task { try await client.call("second") }
    _ = try await fixture.next(tag: "second")
    fixture.incoming.continuation.finish(throwing: URLError(.networkConnectionLost))
    await #expect(throws: URLError.self) { _ = try await first.value }
    await #expect(throws: URLError.self) { _ = try await second.value }
    await #expect(throws: ClientError.self) { _ = try await client.call("next") }
  }

  @Test func aStalledSocketWriteCannotOutliveTheRequestTimeout() async throws {
    let fixture = MemoryRPC()
    let base = fixture.transport
    let transport = RPCTransport(
      start: base.start, receive: base.receive,
      send: { bytes in
        let frame = try JSONDecoder().decode(JSONValue.self, from: bytes)
        if frame["tag"].string == "stalled" {
          fixture.outgoing.continuation.yield(frame)
          try await Task.sleep(for: .seconds(60))
        } else {
          try await base.send(bytes)
        }
      }, close: base.close)
    let client = RPCClient(transport: transport, requestTimeout: .milliseconds(250))
    try await client.connect()
    let request = Task { try await client.call("stalled", ["commandId": .string("original")]) }
    _ = try await fixture.next(tag: "stalled")
    await #expect(throws: ClientError.self) { _ = try await request.value }
    _ = try await fixture.next(tag: "TransportClosed")
    await #expect(throws: ClientError.self) { _ = try await client.call("next") }
  }

  @Test func concurrentRequestsRouteOutOfOrderRepliesWithoutMixingResults() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport)
    try await client.connect()
    let first = Task { try await client.call("first", ["commandId": .string("one")]) }
    let firstRequest = try await fixture.next(tag: "first")
    let second = Task { try await client.call("second", ["commandId": .string("two")]) }
    let secondRequest = try await fixture.next(tag: "second")
    try fixture.push(
      .array([
        MemoryRPC.success(secondRequest, value: .string("second result")),
        MemoryRPC.success(firstRequest, value: .string("first result")),
      ]))
    #expect(try await first.value == .string("first result"))
    #expect(try await second.value == .string("second result"))
    await client.close()
  }

  @Test func cancellingAConsumerInterruptsItsRequestWithoutClosingTheConnection() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport)
    try await client.connect()
    let stream = try await client.stream("events")
    let request = try await fixture.next(tag: "events")
    let consumer = Task {
      for try await _ in stream {}
    }
    consumer.cancel()
    _ = try? await consumer.value
    let interrupt = try await fixture.next(tag: "Interrupt")
    #expect(interrupt["requestId"] == request["id"])
    let next = Task { try await client.call("next") }
    let nextRequest = try await fixture.next(tag: "next")
    try fixture.push(MemoryRPC.success(nextRequest, value: .string("still connected")))
    #expect(try await next.value == .string("still connected"))
    await client.close()
  }

  @Test func timedOutMutationsAreInterruptedAndKeepTheirOutcomeUnknown() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, requestTimeout: .milliseconds(250))
    try await client.connect()
    let call = Task { try await client.call("messages.send", ["commandId": .string("original")]) }
    let request = try await fixture.next(tag: "messages.send")
    do {
      _ = try await call.value
      Issue.record("Expected the request to time out")
    } catch let error as ClientError {
      #expect(error.message.contains("outcome may be unknown"))
    }
    let interrupt = try await fixture.next(tag: "Interrupt")
    #expect(interrupt["requestId"] == request["id"])
    #expect(request["payload"]["commandId"].string == "original")
    await client.close()
  }

  @Test func aStreamThatNeverStartsTimesOutAndReleasesItsServerRequest() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, requestTimeout: .milliseconds(250))
    try await client.connect()
    let stream = try await client.stream("events")
    let request = try await fixture.next(tag: "events")
    await #expect(throws: ClientError.self) {
      for try await _ in stream {}
    }
    let interrupt = try await fixture.next(tag: "Interrupt")
    #expect(interrupt["requestId"] == request["id"])
    await client.close()
  }

  @Test func establishedStreamsKeepRunningBeyondTheInitialReplyDeadline() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, requestTimeout: .milliseconds(100))
    try await client.connect()
    let stream = try await client.stream("events")
    let request = try await fixture.next(tag: "events")
    try fixture.push(
      .object([
        "_tag": .string("Chunk"), "requestId": request["id"], "values": .array([.number(1)]),
      ]))
    _ = try await fixture.next(tag: "Ack")
    try await Task.sleep(for: .milliseconds(200))
    try fixture.push(MemoryRPC.success(request, value: .number(2)))
    var values: [Int] = []
    for try await value in stream { if let number = value.int { values.append(number) } }
    #expect(values == [1, 2])
    await client.close()
  }

  @Test func finalStreamValueCannotSilentlyOverflowTheTranscriptBuffer() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport)
    try await client.connect()
    let stream = try await client.stream("events")
    let request = try await fixture.next(tag: "events")
    try fixture.push(
      .array([
        .object([
          "_tag": .string("Chunk"), "requestId": request["id"],
          "values": .array((0..<512).map { .number(Double($0)) }),
        ]),
        MemoryRPC.success(request, value: .number(512)),
      ]))
    _ = try await fixture.next(tag: "TransportClosed")
    var count = 0
    do {
      for try await _ in stream { count += 1 }
      Issue.record("Expected a buffer overflow error")
    } catch let error as ClientError {
      #expect(error.message.contains("exceeded the buffer"))
    }
    #expect(count == 512)
    await #expect(throws: ClientError.self) { _ = try await client.call("next") }
  }

  @Test func gatewayResetFailsPendingRequestsAndRequiresANewConnection() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, waitsForGateway: true)
    let connection = Task { try await client.connect() }
    try fixture.push(
      .object([
        "_zuseGateway": .string("availability"), "state": .string("available"),
      ]))
    try await connection.value
    let request = Task { try await client.call("pending") }
    _ = try await fixture.next(tag: "pending")
    try fixture.push(
      .object([
        "_zuseGateway": .string("availability"), "state": .string("starting"), "reset": .bool(true),
      ]))
    await #expect(throws: ClientError.self) { _ = try await request.value }
    await #expect(throws: ClientError.self) { _ = try await client.call("next") }
  }

  @Test func canonicalGatewayWaitsAndRejectsRuntimeReplacement() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport, waitsForGateway: true, canonical: true)
    let connecting = Task { try await client.connect() }
    try fixture.push(
      .object(["_zuseGateway": .string("availability"), "state": .string("pending")]))
    try fixture.push(
      .object([
        "_zuseGateway": .string("availability"), "state": .string("available"),
        "reset": .bool(false),
      ]))
    try await connecting.value
    let pending = Task { try await client.call("pending") }
    _ = try await fixture.next(tag: "pending")
    try fixture.push(
      .object([
        "_zuseGateway": .string("availability"), "state": .string("available"),
        "reset": .bool(true),
      ]))
    await #expect(throws: ClientError.self) { _ = try await pending.value }
    await #expect(throws: ClientError.self) { _ = try await client.call("QueueCommand") }
    await client.close()
  }

  @Test func unavailableGatewayTimesOutWithoutSendingAnRPCMutation() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(
      transport: fixture.transport, waitsForGateway: true, readinessTimeout: .milliseconds(100))
    await #expect(throws: ClientError.self) { try await client.connect() }
    // No handshake is sent until availability is confirmed.
    let first = try await fixture.next(tag: "TransportClosed")
    #expect(first["_tag"].string == "TransportClosed")
    await #expect(throws: ClientError.self) { _ = try await client.call("messages.send") }
  }

  @Test func incompatibleHandshakeRetiresTheConnection() async throws {
    var fixture = MemoryRPC()
    fixture.handshakeVersion = RPCClient.wireVersion - 1
    let client = RPCClient(transport: fixture.transport)
    do {
      try await client.connect()
      Issue.record("Expected an incompatible handshake error")
    } catch let error as ClientError {
      #expect(error.code == "protocol-incompatible")
    }
    await #expect(throws: ClientError.self) { _ = try await client.call("messages.send") }
  }

  @Test func attachmentChunksRoundTripThroughTheRealWireClient() async throws {
    let fixture = MemoryRPC()
    let client = RPCClient(transport: fixture.transport)
    try await client.connect()
    let bytes = Data((0..<(Attachments.chunkBytes + 37)).map { UInt8($0 % 251) })
    let upload = Task {
      try await Attachments.upload(
        bytes, name: "example.bin", mimeType: "application/octet-stream", sessionID: "session",
        rpc: client)
    }
    var uploaded = Data()
    var uploadID: String?
    for index in 0..<2 {
      let request = try await fixture.next(tag: "attachments.uploadChunk")
      let payload = request["payload"]
      #expect(payload["offset"].int == uploaded.count)
      #expect(payload["totalBytes"].int == bytes.count)
      #expect(payload["sessionId"].string == "session")
      if let uploadID { #expect(payload["uploadId"].string == uploadID) }
      uploadID = payload["uploadId"].string
      let encoded = try #require(payload["bytes"].string)
      uploaded.append(try #require(Data(base64Encoded: encoded)))
      try fixture.push(
        MemoryRPC.success(
          request,
          value: index == 0
            ? .null
            : .object([
              "id": .string("attachment"), "mimeType": .string("application/octet-stream"),
            ])))
    }
    let reference = try await upload.value
    #expect(uploaded == bytes)
    #expect(reference["id"].string == "attachment")
    let read = Task { try await Attachments.read("attachment", sessionID: "session", rpc: client) }
    let request = try await fixture.next(tag: "attachments.read")
    try fixture.push(
      MemoryRPC.success(
        request,
        value: .object([
          "bytes": .string(uploaded.base64EncodedString()), "originalName": .string("example.bin"),
          "mimeType": .string("application/octet-stream"),
        ])))
    let (downloaded, _, name) = try await read.value
    #expect(downloaded == bytes)
    #expect(name == "example.bin")
    await client.close()
  }
}

@Suite struct NativePairingTests {
  private static let bridgeAvailable: Bool = {
    #if canImport(ZusePairing)
      return true
    #else
      return ProcessInfo.processInfo.environment["ZUSE_NATIVE_PAIRING_LIBRARY"] != nil
    #endif
  }()

  @Test(.enabled(if: Self.bridgeAvailable))
  func loadedBridgeValidatesInvitationsAndRetiresFailedSessions() throws {
    let native = try NativePairing()
    let secret = Data(repeating: 0, count: 32).base64EncodedString().replacingOccurrences(
      of: "+", with: "-"
    ).replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    let wire: JSONValue = .object([
      "v": .number(1), "id": .string(String(repeating: "0", count: 32)), "secret": .string(secret),
    ])
    let fragment = try JSONEncoder().encode(wire).base64EncodedString().replacingOccurrences(
      of: "+", with: "-"
    ).replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    let start = try native.request([
      "op": .string("start"), "link": .string("https://host.example/#pair=" + fragment),
    ])
    let handle = start["handle"]
    defer { _ = try? native.request(["op": .string("close"), "handle": handle]) }
    #expect(
      start["url"].string == "wss://host.example/api/paired-rpc/"
        + String(repeating: "0", count: 32))
    #expect(!start.pretty.contains(secret))
    #expect(throws: ClientError.self) {
      _ = try native.request(["op": .string("finish"), "handle": handle, "record": .string("AA==")])
    }
    #expect(throws: ClientError.self) {
      _ = try native.request(["op": .string("poll"), "handle": handle])
    }
  }
}
