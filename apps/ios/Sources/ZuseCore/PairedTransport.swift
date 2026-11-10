import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// What the interface shows about a paired connection. Contains no secrets.
public struct PairedRouteStatus: Sendable, Equatable {
  public let state: String
  public let label: String?
  public let rttMs: Int?
  public let resumable: Bool
}

/// Lets the app tell live paired connections that the network may have changed.
public enum PairedNetwork {
  private static let lock = NSLock()
  nonisolated(unsafe) private static var listeners: [UUID: @Sendable () -> Void] = [:]
  /// Learned routes are kept here between launches. They hold no secrets.
  nonisolated(unsafe) private static var directory: String?

  public static func setRoutesDirectory(_ path: String) {
    lock.withLock { directory = path }
  }
  static var routesDirectory: String? { lock.withLock { directory } }
  public static func changed() {
    let current = lock.withLock { Array(listeners.values) }
    for listener in current { listener() }
  }
  static func observe(_ listener: @escaping @Sendable () -> Void) -> UUID {
    let token = UUID()
    lock.withLock { listeners[token] = listener }
    return token
  }
  static func remove(_ token: UUID) {
    lock.withLock { listeners[token] = nil }
  }
}

/// Owns the sockets of one paired connection. The Rust link decides which
/// routes to open, when to move and what to resend; this only performs that.
actor PairedTransport {
  private let bridge: NativePairing
  private let link: String
  private let session: URLSession
  private let onRoute: (@Sendable (PairedRouteStatus) -> Void)?
  private var handle: JSONValue = .null
  private var sockets: [Int: URLSessionWebSocketTask] = [:]
  private var readers: [Int: Task<Void, Never>] = [:]
  private var writeTails: [Int: Task<Void, Error>] = [:]
  private var wake: Task<Void, Never>?
  private var observer: UUID?
  private var started = false
  private var connected = false
  private var closed = false
  private var starters: [CheckedContinuation<Void, Error>] = []
  private var deliveries: [Data] = []
  private var receiver: CheckedContinuation<Data, Error>?

  init(
    link: String, session: URLSession, onRoute: (@Sendable (PairedRouteStatus) -> Void)? = nil
  ) throws {
    self.link = link
    self.session = session
    self.onRoute = onRoute
    bridge = try NativePairing()
  }
  nonisolated var transport: RPCTransport {
    RPCTransport(
      start: {}, receive: { try await self.receive() }, send: { try await self.send($0) },
      close: { Task { await self.close() } })
  }

  private func request(_ op: String, fields: [String: JSONValue] = [:]) {
    guard !closed, handle != .null else { return }
    var value = fields
    value["op"] = .string(op)
    value["handle"] = handle
    do { apply(try bridge.request(value)) } catch { fail() }
  }
  private func ensureStarted() async throws {
    guard !closed else { throw ClientError("Paired connection closed.") }
    if connected { return }
    if !started {
      started = true
      var open: [String: JSONValue] = ["op": .string("linkOpen"), "link": .string(link)]
      if let store = PairedNetwork.routesDirectory { open["store"] = .string(store) }
      do {
        let result = try bridge.request(open)
        handle = result["handle"]
        observer = PairedNetwork.observe { [weak self] in
          Task { await self?.networkChanged() }
        }
        apply(result)
      } catch {
        fail()
        throw ClientError("Paired connection unavailable. Reconnect with a valid invitation.")
      }
    }
    if connected { return }
    try await withCheckedThrowingContinuation { starters.append($0) }
  }
  private func networkChanged() { request("linkNetworkChanged") }

  private func apply(_ result: JSONValue) {
    for entry in result["open"].array {
      if let id = entry["socket"].int, let url = entry["url"].string { open(id, url) }
    }
    for entry in result["close"].array {
      if let id = entry.int { discard(id) }
    }
    for entry in result["write"].array {
      guard let id = entry["socket"].int else { continue }
      write(id, entry["records"].array.compactMap { $0.string.flatMap { Data(base64Encoded: $0) } })
    }
    for entry in result["deliver"].array {
      guard let data = entry.string.flatMap({ Data(base64Encoded: $0) }) else { continue }
      if let waiting = receiver {
        receiver = nil
        waiting.resume(returning: data)
      } else {
        deliveries.append(data)
      }
    }
    let status = result["status"]
    if let state = status["state"].string {
      onRoute?(
        PairedRouteStatus(
          state: state, label: status["label"].string, rttMs: status["rttMs"].int,
          resumable: status["resumable"].bool))
    }
    if result["connected"].bool {
      connected = true
      let waiting = starters
      starters = []
      for starter in waiting { starter.resume() }
    }
    if result["ended"].string != nil {
      handle = .null
      fail()
      return
    }
    schedule(after: result["waitMs"].int ?? 1000)
  }
  private func schedule(after milliseconds: Int) {
    wake?.cancel()
    wake = Task { [weak self] in
      try? await Task.sleep(for: .milliseconds(milliseconds))
      guard !Task.isCancelled else { return }
      await self?.tick()
    }
  }
  private func tick() { request("linkTick") }

  private func open(_ id: Int, _ address: String) {
    guard let url = URL(string: address) else {
      request("linkSocketClosed", fields: ["socket": .number(Double(id))])
      return
    }
    let socket = session.webSocketTask(with: url)
    socket.maximumMessageSize = 65_535
    socket.resume()
    sockets[id] = socket
    readers[id] = Task { [weak self] in
      do {
        while !Task.isCancelled {
          guard case .data(let record) = try await socket.receive(), record.count <= 65_535 else {
            throw ClientError("The paired host sent an invalid record.")
          }
          await self?.received(id, record)
        }
      } catch {
        await self?.socketClosed(id)
      }
    }
    // URLSession queues writes until the upgrade completes.
    request("linkSocketOpened", fields: ["socket": .number(Double(id))])
  }
  private func received(_ id: Int, _ record: Data) {
    guard sockets[id] != nil else { return }
    request(
      "linkRecord",
      fields: ["socket": .number(Double(id)), "record": .string(record.base64EncodedString())])
  }
  private func socketClosed(_ id: Int) {
    guard sockets[id] != nil else { return }
    discard(id)
    request("linkSocketClosed", fields: ["socket": .number(Double(id))])
  }
  private func write(_ id: Int, _ records: [Data]) {
    guard let socket = sockets[id], !records.isEmpty else { return }
    let previous = writeTails[id]
    writeTails[id] = Task { [weak self] in
      try await previous?.value
      do {
        for record in records {
          try Task.checkCancellation()
          try await socket.send(.data(record))
        }
      } catch {
        await self?.socketClosed(id)
        throw error
      }
    }
  }
  private func discard(_ id: Int) {
    readers.removeValue(forKey: id)?.cancel()
    writeTails.removeValue(forKey: id)?.cancel()
    sockets.removeValue(forKey: id)?.cancel(with: .goingAway, reason: nil)
  }

  private func send(_ data: Data) async throws {
    try await ensureStarted()
    guard !closed, handle != .null else { throw ClientError("Paired connection closed.") }
    do {
      let result = try bridge.request([
        "op": .string("linkSend"), "handle": handle,
        "payload": .string(data.base64EncodedString()),
      ])
      guard result["accepted"].bool else { throw ClientError("Paired connection closed.") }
      apply(result)
    } catch {
      fail()
      throw ClientError("Paired connection closed.")
    }
  }
  private func receive() async throws -> Data {
    do { try await ensureStarted() } catch {
      throw ClientError("Paired connection unavailable. Reconnect with a valid invitation.")
    }
    if !deliveries.isEmpty { return deliveries.removeFirst() }
    guard !closed else { throw ClientError("Paired connection closed.") }
    return try await withCheckedThrowingContinuation { receiver = $0 }
  }

  private func fail() {
    guard !closed else { return }
    closed = true
    if let observer { PairedNetwork.remove(observer) }
    observer = nil
    wake?.cancel()
    for id in Array(sockets.keys) { discard(id) }
    let error = ClientError("Paired connection closed.")
    let waiting = starters
    starters = []
    for starter in waiting { starter.resume(throwing: error) }
    receiver?.resume(throwing: error)
    receiver = nil
    if handle != .null {
      _ = try? bridge.request(["op": .string("linkClose"), "handle": handle])
      handle = .null
    }
  }
  func close() { fail() }
}
