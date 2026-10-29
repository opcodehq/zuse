import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// A private, short-lived bootstrap receipt. Persist public computer metadata separately.
public struct AccountGrant: Sendable, CustomStringConvertible {
  public let environmentID: String
  public let origin: String
  public let token: String
  public let expiresAt: Int
  public init(environmentID: String, origin: String, token: String, expiresAt: Int) {
    self.environmentID = environmentID
    self.origin = origin
    self.token = token
    self.expiresAt = expiresAt
  }
  public var description: String {
    "AccountGrant(environmentID: \(environmentID), expiresAt: \(expiresAt))"
  }
  func socketURL() throws -> URL {
    guard var url = URLComponents(string: origin), let host = url.host,
      url.scheme == "https"
        || (url.scheme == "http" && ["localhost", "127.0.0.1", "::1"].contains(host)),
      url.user == nil, url.password == nil, url.query == nil, url.fragment == nil,
      url.path.isEmpty || url.path == "/", !environmentID.isEmpty, !token.isEmpty,
      token.utf8.count <= 16_384, expiresAt > Int(Date().timeIntervalSince1970 * 1000)
    else { throw ClientError("Invalid account connection receipt.") }
    url.scheme = url.scheme == "https" ? "wss" : "ws"
    url.path = "/api/account-rpc"
    guard let address = url.url else { throw ClientError("Invalid account endpoint.") }
    return address
  }
}

actor AccountTransport {
  static let protocolName = "zuse-rpc-json-v1"
  private let expectedID: String
  private let grant: @Sendable () async throws -> AccountGrant
  private let makeSocket: @Sendable (URL) -> RPCTransport
  private var socket: RPCTransport?
  private var startTask: Task<Void, Error>?
  private var maintenance: Task<Void, Never>?
  private var renewalTask: Task<Void, Never>?
  private var closed = false
  private var endpoint: URL?
  private var until = 0
  private var expires = ContinuousClock.now
  private var renewAt = ContinuousClock.now
  private var awaiting: AccountGrant?
  private var ackDeadline = ContinuousClock.now
  private var pong: String?
  private var pongDeadline = ContinuousClock.now
  private var pingAt = ContinuousClock.now.advanced(by: .seconds(20))

  init(
    expectedID: String, grant: @escaping @Sendable () async throws -> AccountGrant,
    makeSocket: @escaping @Sendable (URL) -> RPCTransport
  ) {
    self.expectedID = expectedID
    self.grant = grant
    self.makeSocket = makeSocket
  }
  static func websocket(_ url: URL, session: URLSession) -> RPCTransport {
    var request = URLRequest(url: url)
    request.setValue(protocolName, forHTTPHeaderField: "Sec-WebSocket-Protocol")
    let socket = session.webSocketTask(with: request)
    socket.maximumMessageSize = Attachments.maximumBytes * 4 / 3 + 1024 * 1024
    return RPCTransport(
      start: { socket.resume() },
      receive: {
        guard case .string(let text) = try await socket.receive() else {
          throw ClientError("Unexpected account frame.")
        }
        return Data(text.utf8)
      }, send: { try await socket.send(.string(String(decoding: $0, as: UTF8.self))) },
      close: { socket.cancel(with: .goingAway, reason: nil) })
  }
  nonisolated var transport: RPCTransport {
    RPCTransport(
      start: {}, receive: { try await self.receive() }, send: { try await self.send($0) },
      close: { Task { await self.close() } })
  }
  private func write(_ value: JSONValue) async throws {
    guard !closed, let socket else { throw ClientError("Account connection closed.") }
    try await socket.send(JSONEncoder().encode(value))
  }
  private func authenticate(_ value: AccountGrant) async throws {
    try await write(
      .object(["protocol": .string(Self.protocolName), "connectToken": .string(value.token)]))
  }
  private func acknowledge(_ value: JSONValue, grant: AccountGrant, renewed: Bool) throws {
    let now = Int(Date().timeIntervalSince1970 * 1000)
    guard value["protocol"].string == Self.protocolName, value["authenticated"].bool,
      value["deviceId"].string == expectedID, value["renewed"].bool == renewed,
      let expiry = value["expiresAt"].int, expiry > now, expiry <= grant.expiresAt,
      grant.environmentID == expectedID
    else { throw ClientError("Account authorization was not confirmed.") }
    until = expiry
    let remaining = expiry - now
    expires = .now.advanced(by: .milliseconds(remaining))
    renewAt = .now.advanced(
      by: .milliseconds(max(1, remaining - min(30_000, max(1, remaining / 3)))))
  }
  private func ensureStarted() async throws {
    guard !closed else { throw ClientError("Account connection closed.") }
    if startTask == nil { startTask = Task { try await self.start() } }
    do { try await startTask?.value } catch {
      close()
      throw ClientError("Account connection unavailable. Select the computer again.")
    }
  }
  private func start() async throws {
    let timeout = Task { [weak self] in
      try? await Task.sleep(for: .seconds(10))
      if !Task.isCancelled { await self?.close() }
    }
    defer { timeout.cancel() }
    let first = try await grant()
    guard !closed, first.environmentID == expectedID else {
      throw ClientError("Computer identity changed.")
    }
    let url = try first.socketURL()
    endpoint = url
    socket = makeSocket(url)
    socket?.start()
    try await authenticate(first)
    guard let socket else { throw ClientError("Account connection closed.") }
    try acknowledge(
      JSONDecoder().decode(JSONValue.self, from: await socket.receive()), grant: first,
      renewed: false)
    maintenance = Task { [weak self] in
      while !Task.isCancelled {
        try? await Task.sleep(for: .milliseconds(250))
        guard !Task.isCancelled, let self else { return }
        do { try await self.tick() } catch {
          await self.close()
          return
        }
      }
    }
  }
  private func checkLive() throws {
    guard !closed, Int(Date().timeIntervalSince1970 * 1000) < until, ContinuousClock.now < expires,
      awaiting == nil || ContinuousClock.now < ackDeadline,
      pong == nil || ContinuousClock.now < pongDeadline
    else { throw ClientError("Account authorization ended.") }
  }
  private func tick() async throws {
    try checkLive()
    if renewalTask == nil && awaiting == nil && ContinuousClock.now >= renewAt {
      // Reserve renewal before awaiting the controller so actor reentrancy cannot duplicate it.
      renewAt = .now.advanced(by: .seconds(10))
      renewalTask = Task { [weak self] in
        guard let self else { return }
        do { try await self.renew() } catch { await self.close() }
      }
    }
    if pong == nil && ContinuousClock.now >= pingAt {
      let nonce = UUID().uuidString
      pong = nonce
      pongDeadline = .now.advanced(by: .seconds(10))
      try await write(.object(["protocol": .string(Self.protocolName), "ping": .string(nonce)]))
    }
  }
  private func renew() async throws {
    defer { renewalTask = nil }
    let next = try await grant()
    try checkLive()
    guard next.environmentID == expectedID, try next.socketURL() == endpoint else {
      throw ClientError("Computer endpoint changed.")
    }
    if next.expiresAt > until + 1000 {
      awaiting = next
      ackDeadline = .now.advanced(by: .seconds(10))
      try await authenticate(next)
    } else {
      renewAt = .now.advanced(by: .seconds(1))
    }
  }
  private func send(_ data: Data) async throws {
    try await ensureStarted()
    do {
      try checkLive()
      try await socket?.send(data)
    } catch {
      close()
      throw error
    }
  }
  private func receive() async throws -> Data {
    try await ensureStarted()
    do {
      while let socket {
        let data = try await socket.receive()
        try checkLive()
        let value = try JSONDecoder().decode(JSONValue.self, from: data)
        guard value["protocol"] != .null else { return data }
        guard value["protocol"].string == Self.protocolName else {
          throw ClientError("Invalid account control.")
        }
        if let nonce = value["pong"].string {
          guard nonce == pong else { throw ClientError("Invalid account heartbeat.") }
          pong = nil
          pingAt = .now.advanced(by: .seconds(20))
        } else {
          guard let pending = awaiting else {
            throw ClientError("Unexpected account acknowledgement.")
          }
          try acknowledge(value, grant: pending, renewed: true)
          awaiting = nil
        }
      }
      throw ClientError("Account connection closed.")
    } catch {
      close()
      throw error
    }
  }
  func close() {
    guard !closed else { return }
    closed = true
    startTask?.cancel()
    maintenance?.cancel()
    renewalTask?.cancel()
    socket?.close()
  }
}
