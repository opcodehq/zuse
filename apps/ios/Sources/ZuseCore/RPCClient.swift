import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// The wire client owns multiplexing and recovery; this boundary owns only socket I/O.
struct RPCTransport: Sendable {
  let start: @Sendable () -> Void
  let receive: @Sendable () async throws -> Data
  let send: @Sendable (Data) async throws -> Void
  let close: @Sendable () -> Void
}

/// Effect JSON RPC transport. Each connection owns its requests; mutations are never replayed.
public actor RPCClient {
  public static let wireVersion = 7
  public let canonical: Bool
  private var nextID = 0
  private var accountIdentity: String?
  private var cloudExpiresAt: Int?
  private var authorizationExpiry: Task<Void, Never>?
  private let transport: RPCTransport
  private let requestTimeout: Duration
  private let readinessTimeout: Duration
  private var reader: Task<Void, Never>?
  private var pending: [String: AsyncThrowingStream<JSONValue, Error>.Continuation] = [:]
  private var timers: [String: Task<Void, Never>] = [:]
  private var writers: [String: Task<Void, Never>] = [:]
  private var startingStreams = Set<String>()
  private var closed = false
  private var gateway = false
  private var available: Bool
  private var readiness: CheckedContinuation<Void, Error>?
  private var readinessTimer: Task<Void, Never>?

  public init(pairingLink: String, session: URLSession = .shared) throws {
    let paired = try PairedTransport(link: pairingLink, session: session)
    transport = paired.transport
    canonical = true
    available = true
    requestTimeout = .seconds(30)
    readinessTimeout = .seconds(45)
  }

  public init(
    accountEnvironmentID: String, grant: @escaping @Sendable () async throws -> AccountGrant,
    session: URLSession = .shared
  ) {
    accountIdentity = accountEnvironmentID
    transport =
      AccountTransport(
        expectedID: accountEnvironmentID, grant: grant,
        makeSocket: { AccountTransport.websocket($0, session: session) }
      ).transport
    canonical = true
    available = true
    requestTimeout = .seconds(30)
    readinessTimeout = .seconds(45)
  }

  public init(
    url: URL, protocols: [String] = [], session: URLSession = .shared, ownsSession: Bool = false,
    token: String? = nil, cloudExpiresAt: Int? = nil
  ) throws {
    guard url.scheme == "wss" || url.scheme == "ws", url.host != nil else {
      throw ClientError("Enter a valid WebSocket address.")
    }
    guard var components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
      throw ClientError("Invalid connection address.")
    }
    self.cloudExpiresAt = cloudExpiresAt
    let isCanonical = url.path == "/api/rpc" || protocols.contains(CloudGateway.rust)
    canonical = isCanonical
    let credential = token ?? components.queryItems?.first(where: { $0.name == "token" })?.value
    var query = (components.queryItems ?? []).filter {
      $0.name != "wireVersion" && (!isCanonical || $0.name != "token")
    }
    if !canonical {
      query.append(URLQueryItem(name: "wireVersion", value: String(Self.wireVersion)))
    }
    components.queryItems = query
    guard let address = components.url else { throw ClientError("Invalid connection address.") }
    var request = URLRequest(url: address)
    if !protocols.isEmpty {
      request.setValue(
        protocols.joined(separator: ", "), forHTTPHeaderField: "Sec-WebSocket-Protocol")
    }
    if canonical {
      var origin = components
      origin.scheme = url.scheme == "wss" ? "https" : "http"
      origin.path = ""
      origin.queryItems = nil
      origin.fragment = nil
      request.setValue(origin.string, forHTTPHeaderField: "Origin")
      if let credential {
        request.setValue("Bearer \(credential)", forHTTPHeaderField: "Authorization")
      }
    }
    let socket = session.webSocketTask(with: request)
    // Attachment reads carry base64 for files up to the shared 100 MB limit.
    socket.maximumMessageSize = Attachments.maximumBytes * 4 / 3 + 1024 * 1024
    transport = RPCTransport(
      start: { socket.resume() },
      receive: {
        switch try await socket.receive() {
        case .data(let bytes): return bytes
        case .string(let text): return Data(text.utf8)
        @unknown default: throw ClientError("The runtime sent an unsupported socket message.")
        }
      },
      send: { bytes in
        try await socket.send(
          isCanonical ? .string(String(decoding: bytes, as: UTF8.self)) : .data(bytes))
      },
      close: {
        socket.cancel(with: .goingAway, reason: nil)
        if ownsSession { session.invalidateAndCancel() }
      })
    requestTimeout = .seconds(30)
    readinessTimeout = .seconds(45)
    gateway = protocols.contains(CloudGateway.legacy) || protocols.contains(CloudGateway.rust)
    available = !gateway
  }

  init(
    transport: RPCTransport, waitsForGateway: Bool = false,
    requestTimeout: Duration = .seconds(30), readinessTimeout: Duration = .seconds(45),
    canonical: Bool = false
  ) {
    self.canonical = canonical
    self.transport = transport
    self.requestTimeout = requestTimeout
    self.readinessTimeout = readinessTimeout
    gateway = waitsForGateway
    available = !waitsForGateway
  }

  public func connect() async throws {
    try Task.checkCancellation()
    guard reader == nil, !closed else { throw ClientError("This connection is already in use.") }
    if let cloudExpiresAt {
      let remaining = cloudExpiresAt - Int(Date().timeIntervalSince1970 * 1000)
      guard remaining > 0 else { throw ClientError("Cloud authorization expired.") }
      authorizationExpiry = Task { [weak self] in
        try? await Task.sleep(for: .milliseconds(remaining))
        if !Task.isCancelled {
          await self?.close(
            error: ClientError("Cloud authorization expired. Reconnect to continue."))
        }
      }
    }
    transport.start()
    reader = Task { [weak self, transport] in
      do {
        while !Task.isCancelled {
          let data = try await transport.receive()
          let value = try JSONDecoder().decode(JSONValue.self, from: data)
          if case .array(let frames) = value {
            for frame in frames { try await self?.receive(frame) }
          } else {
            try await self?.receive(value)
          }
        }
      } catch { await self?.close(error: error) }
    }
    do {
      if !available {
        try await withTaskCancellationHandler {
          try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, Error>) in
            guard !closed else {
              continuation.resume(throwing: ClientError("The connection is unavailable."))
              return
            }
            guard !available else {
              continuation.resume()
              return
            }
            readiness = continuation
            readinessTimer = Task { [weak self, readinessTimeout] in
              try? await Task.sleep(for: readinessTimeout)
              guard !Task.isCancelled else { return }
              await self?.close(
                error: ClientError("The cloud runtime is not ready. Try reconnecting."))
            }
          }
        } onCancel: {
          Task { await self.close() }
        }
      }
      if canonical {
        let info = try await call("EngineInfo")
        if let accountIdentity, info["deviceId"].string != accountIdentity {
          throw ClientError("Computer identity changed.")
        }
        return
      }
      let welcome = try await call(
        "connect.handshake", ["protocolVersion": .number(Double(Self.wireVersion))])
      guard welcome["protocolVersion"].int == Self.wireVersion else {
        close()
        throw ClientError(
          "Update the app and runtime to compatible versions.", code: "protocol-incompatible")
      }
    } catch {
      close(error: error)
      throw error
    }
  }

  public func call(_ method: String, _ payload: [String: JSONValue] = [:]) async throws -> JSONValue
  {
    let stream = try await request(method, payload, timeout: requestTimeout)
    var result: JSONValue = .null
    for try await value in stream { result = value }
    try Task.checkCancellation()
    return result
  }
  /// Read one watch value, then explicitly release the server subscription.
  public func snapshot(_ method: String, _ payload: [String: JSONValue] = [:]) async throws
    -> JSONValue
  {
    guard canonical else { throw ClientError("Snapshots require the canonical engine transport.") }
    nextID += 1
    let id = String(nextID)
    let stream = try await request(method, payload, timeout: requestTimeout, reservedID: id)
    do {
      var iterator = stream.makeAsyncIterator()
      guard let value = try await iterator.next() else {
        throw ClientError("The snapshot ended without a value.")
      }
      await cancel(id)
      return value
    } catch {
      await cancel(id)
      throw error
    }
  }

  public func stream(_ method: String, _ payload: [String: JSONValue] = [:]) async throws
    -> AsyncThrowingStream<JSONValue, Error>
  {
    try await request(method, payload, timeout: nil)
  }
  private func request(
    _ method: String, _ payload: [String: JSONValue], timeout: Duration?, reservedID: String? = nil
  )
    async throws
    -> AsyncThrowingStream<JSONValue, Error>
  {
    try Task.checkCancellation()
    guard reader != nil, !closed, available else {
      throw ClientError("The connection is unavailable.")
    }
    if reservedID == nil { nextID += 1 }
    let id = reservedID ?? (canonical ? String(nextID) : UUID().uuidString)
    let (stream, continuation) = AsyncThrowingStream<JSONValue, Error>.makeStream(
      bufferingPolicy: .bufferingOldest(512))
    pending[id] = continuation
    continuation.onTermination = { [weak self] _ in Task { await self?.cancel(id) } }
    if timeout == nil { startingStreams.insert(id) }
    let deadline = timeout ?? requestTimeout
    timers[id] = Task { [weak self] in
      try? await Task.sleep(for: deadline)
      guard !Task.isCancelled else { return }
      await self?.expire(id)
    }
    let frame: JSONValue =
      canonical
      ? .object([
        "id": .number(Double(id) ?? 0), "method": .string(method), "params": .object(payload),
      ])
      : .object([
        "_tag": .string("Request"), "id": .string(id), "tag": .string(method),
        "payload": .object(payload), "headers": .array([]),
      ])
    // Return the stream before writing: its timeout/cancellation must also cover a stuck send.
    writers[id] = Task { [weak self] in
      await self?.writeRequest(frame, id: id)
    }
    return stream
  }
  private func writeRequest(_ frame: JSONValue, id: String) async {
    guard pending[id] != nil, !closed, !Task.isCancelled else { return }
    defer { writers.removeValue(forKey: id) }
    do {
      try await send(frame)
    } catch {
      // A cancelled writer was already settled by close, cancellation, or its timeout.
      if !Task.isCancelled { fail(id, error: error) }
    }
  }
  private func receive(_ frame: JSONValue) async throws {
    if frame["_zuseGateway"].string == "availability" {
      if frame["reset"].bool || (available && gateway) {
        throw ClientError("The workspace runtime changed. Reconnect to continue.")
      }
      if frame["state"].string == "available" {
        available = true
        readinessTimer?.cancel()
        readiness?.resume()
        readiness = nil
      }
      guard ["pending", "available"].contains(frame["state"].string ?? "") else {
        throw ClientError("Invalid cloud runtime state.")
      }
      return
    }
    if gateway && !available { throw ClientError("Cloud runtime was not ready.") }
    if canonical {
      guard case .object(let fields) = frame, let number = frame["id"].int else { return }
      let id = String(number)
      guard let continuation = pending[id] else { return }
      if let error = fields["err"] {
        fail(id, error: ClientError(error.string ?? "Request failed"))
      } else if let value = fields["ok"] {
        pending.removeValue(forKey: id)
        startingStreams.remove(id)
        timers.removeValue(forKey: id)?.cancel()
        writers.removeValue(forKey: id)?.cancel()
        if case .dropped = continuation.yield(value) {
          throw ClientError("Response buffer exceeded")
        }
        continuation.finish()
      } else if let value = fields["item"] {
        if startingStreams.remove(id) != nil { timers.removeValue(forKey: id)?.cancel() }
        if case .dropped = continuation.yield(value) {
          throw ClientError("Response buffer exceeded")
        }
      } else if frame["done"].bool {
        pending.removeValue(forKey: id)
        startingStreams.remove(id)
        timers.removeValue(forKey: id)?.cancel()
        writers.removeValue(forKey: id)?.cancel()
        continuation.finish()
      }
      return
    }
    if frame["_tag"].string == "Ping" {
      try await send(.object(["_tag": .string("Pong")]))
      return
    }
    if frame["_tag"].string == "Pong" { return }
    if frame["_tag"].string == "Defect" {
      throw ClientError("The runtime rejected the connection.")
    }
    guard let reply = RPCReply(frame) else { return }
    switch reply {
    case .chunk(let id, let values):
      guard let continuation = pending[id] else { return }
      if startingStreams.remove(id) != nil { timers.removeValue(forKey: id)?.cancel() }
      for value in values {
        if case .dropped = continuation.yield(value) {
          throw ClientError("Transcript updates exceeded the buffer. Reconnecting is required.")
        }
      }
      try await send(.object(["_tag": .string("Ack"), "requestId": .string(id)]))
    case .success(let id, let value):
      guard let continuation = pending.removeValue(forKey: id) else { return }
      startingStreams.remove(id)
      timers.removeValue(forKey: id)?.cancel()
      writers.removeValue(forKey: id)?.cancel()
      if value != .null, case .dropped = continuation.yield(value) {
        let error = ClientError("Transcript updates exceeded the buffer. Reconnecting is required.")
        continuation.finish(throwing: error)
        throw error
      }
      continuation.finish()
    case .failure(let id, let error):
      fail(id, error: error)
    }
  }
  private func send(_ frame: JSONValue) async throws {
    try await transport.send(JSONEncoder().encode(frame))
  }
  private func fail(_ id: String, error: Error) {
    startingStreams.remove(id)
    timers.removeValue(forKey: id)?.cancel()
    writers.removeValue(forKey: id)?.cancel()
    pending.removeValue(forKey: id)?.finish(throwing: error)
  }
  private func cancel(_ id: String) async {
    guard let continuation = pending.removeValue(forKey: id) else { return }
    startingStreams.remove(id)
    timers.removeValue(forKey: id)?.cancel()
    writers.removeValue(forKey: id)?.cancel()
    continuation.finish(throwing: CancellationError())
    try? await send(
      canonical
        ? .object(["id": .number(Double(id) ?? 0), "cancel": .bool(true)])
        : .object(["_tag": .string("Interrupt"), "requestId": .string(id)]))
  }
  private func expire(_ id: String) async {
    guard pending[id] != nil else { return }
    // Do not cancel this timer's own task before sending the remote interruption.
    timers.removeValue(forKey: id)
    let writing = writers[id] != nil
    let error = ClientError(
      "The request timed out. Its outcome may be unknown; reconnect before retrying.")
    fail(id, error: error)
    if writing {
      close(error: error)
    } else {
      try? await send(
        canonical
          ? .object(["id": .number(Double(id) ?? 0), "cancel": .bool(true)])
          : .object(["_tag": .string("Interrupt"), "requestId": .string(id)]))
    }
  }
  public func close(error: Error = CancellationError()) {
    guard !closed else { return }
    closed = true
    authorizationExpiry?.cancel()
    reader?.cancel()
    reader = nil
    readinessTimer?.cancel()
    readiness?.resume(throwing: error)
    readiness = nil
    let continuations = pending.values
    pending = [:]
    startingStreams = []
    for continuation in continuations { continuation.finish(throwing: error) }
    for timer in timers.values { timer.cancel() }
    timers = [:]
    for writer in writers.values { writer.cancel() }
    writers = [:]
    transport.close()
  }
}
