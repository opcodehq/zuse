import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// One Noise generation per socket. The native bridge also proves host responsiveness.
actor PairedTransport {
  private let bridge: NativePairing
  private let link: String
  private let session: URLSession
  private var handle: JSONValue = .null
  private var socket: URLSessionWebSocketTask?
  private var startTask: Task<Void, Error>?
  private var writeTail: Task<Void, Error>?
  private var heartbeatTask: Task<Void, Never>?
  private var closed = false

  init(link: String, session: URLSession) throws {
    self.link = link
    self.session = session
    bridge = try NativePairing()
  }
  nonisolated var transport: RPCTransport {
    RPCTransport(
      start: {}, receive: { try await self.receive() }, send: { try await self.send($0) },
      close: { Task { await self.close() } })
  }
  private func request(_ op: String, fields: [String: JSONValue] = [:]) throws -> JSONValue {
    guard !closed else { throw ClientError("Paired connection closed.") }
    var value = fields
    value["op"] = .string(op)
    value["handle"] = handle
    return try bridge.request(value)
  }
  private func write(_ result: JSONValue) async throws {
    guard let socket, !closed else { throw ClientError("Paired connection closed.") }
    let records = try result["records"].array.map { record -> Data in
      guard let text = record.string, let data = Data(base64Encoded: text) else {
        throw ClientError("Invalid paired record.")
      }
      return data
    }
    guard !records.isEmpty else { return }
    let previous = writeTail
    let task = Task {
      try await previous?.value
      for record in records {
        try Task.checkCancellation()
        try await socket.send(.data(record))
      }
    }
    writeTail = task
    try await task.value
  }
  private func readRecord() async throws -> Data {
    guard let socket, !closed else { throw ClientError("Paired connection closed.") }
    guard case .data(let record) = try await socket.receive(), record.count <= 65_535 else {
      throw ClientError("The paired host sent an invalid record.")
    }
    return record
  }
  private func ensureStarted() async throws {
    guard !closed else { throw ClientError("Paired connection closed.") }
    if startTask == nil { startTask = Task { try await self.authenticate() } }
    do { try await startTask?.value } catch {
      close()
      throw ClientError("Paired connection unavailable. Reconnect with a valid invitation.")
    }
  }
  private func authenticate() async throws {
    let start = try bridge.request(["op": .string("start"), "link": .string(link)])
    handle = start["handle"]
    guard let address = start["url"].string.flatMap(URL.init(string:)) else {
      throw ClientError("Invalid paired endpoint.")
    }
    socket = session.webSocketTask(with: address)
    socket?.maximumMessageSize = 65_535
    socket?.resume()
    let deadline = Task { [weak self] in
      try? await Task.sleep(for: .seconds(10))
      if !Task.isCancelled { await self?.close() }
    }
    defer { deadline.cancel() }
    try await write(start)
    let first = try await readRecord()
    let finish = try request("finish", fields: ["record": .string(first.base64EncodedString())])
    try await write(finish)
    while true {
      let record = try await readRecord()
      let result = try request("receive", fields: ["record": .string(record.base64EncodedString())])
      if result["ready"].bool { break }
    }
    heartbeatTask = Task { [weak self] in
      while !Task.isCancelled {
        try? await Task.sleep(for: .seconds(1))
        guard !Task.isCancelled, let self else { return }
        do { try await self.poll() } catch {
          await self.close()
          return
        }
      }
    }
  }
  private func poll() async throws { try await write(request("poll")) }
  private func send(_ data: Data) async throws {
    try await ensureStarted()
    do {
      try await write(request("send", fields: ["payload": .string(data.base64EncodedString())]))
    } catch {
      close()
      throw ClientError("Paired connection closed.")
    }
  }
  private func receive() async throws -> Data {
    try await ensureStarted()
    do {
      while true {
        let record = try await readRecord()
        let result = try request(
          "receive", fields: ["record": .string(record.base64EncodedString())])
        if let payload = result["payload"].string {
          guard let data = Data(base64Encoded: payload) else {
            throw ClientError("Invalid paired message.")
          }
          return data
        }
      }
    } catch {
      close()
      throw ClientError("Paired connection closed.")
    }
  }
  func close() {
    guard !closed else { return }
    closed = true
    startTask?.cancel()
    heartbeatTask?.cancel()
    writeTail?.cancel()
    socket?.cancel(with: .goingAway, reason: nil)
    _ = try? bridge.request(["op": .string("close"), "handle": handle])
    handle = .null
  }
}
