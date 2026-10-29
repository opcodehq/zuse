import Foundation
import Testing

@testable import ZuseCore

private actor AccountFixture {
  let origin = "https://host.example"
  let wrongIdentity: Bool
  let renews: Bool
  var issued = 0
  var authentications = 0
  var last: AccountGrant?
  var firstExpiry: Int?
  init(wrongIdentity: Bool = false, renews: Bool = true) {
    self.wrongIdentity = wrongIdentity
    self.renews = renews
  }
  func grant() -> AccountGrant {
    issued += 1
    let initial = issued == 1 || !renews
    let now = Int(Date().timeIntervalSince1970 * 1000)
    if firstExpiry == nil { firstExpiry = now + 1800 }
    let value = AccountGrant(
      environmentID: "host", origin: origin, token: initial ? "initial" : "renewed",
      expiresAt: initial ? firstExpiry! : now + 60_000)
    last = value
    return value
  }
  func respond(_ value: JSONValue) -> JSONValue {
    if value["protocol"].string != nil {
      authentications += 1
      return .object([
        "protocol": .string("zuse-rpc-json-v1"), "authenticated": .bool(true),
        "deviceId": .string(wrongIdentity ? "imposter" : "host"),
        "expiresAt": .number(Double(last!.expiresAt)), "renewed": .bool(authentications > 1),
      ])
    }
    return .object(["id": value["id"], "ok": .object(["deviceId": .string("host")])])
  }
}

@Suite struct AccountTransportTests {
  private func client(_ fixture: AccountFixture) -> RPCClient {
    let incoming = AsyncThrowingStream<Data, Error>.makeStream()
    let transport = AccountTransport(
      expectedID: "host", grant: { await fixture.grant() },
      makeSocket: { _ in
        RPCTransport(
          start: {},
          receive: {
            var iterator = incoming.stream.makeAsyncIterator()
            guard let data = try await iterator.next() else { throw ClientError("Closed") }
            return data
          },
          send: { data in
            let frame = try JSONDecoder().decode(JSONValue.self, from: data)
            let response = await fixture.respond(frame)
            incoming.continuation.yield(try JSONEncoder().encode(response))
          }, close: { incoming.continuation.finish(throwing: ClientError("Closed")) })
      })
    return RPCClient(transport: transport.transport, requestTimeout: .seconds(3), canonical: true)
  }
  @Test func authenticatesAndRenewsBeforeOriginalExpiry() async throws {
    let fixture = AccountFixture()
    let rpc = client(fixture)
    try await rpc.connect()
    try await Task.sleep(for: .milliseconds(2600))
    #expect(await fixture.authentications == 2)
    #expect(try await rpc.call("EngineInfo")["deviceId"].string == "host")
    await rpc.close()
  }
  @Test func rejectsWrongComputerIdentity() async throws {
    let rpc = client(AccountFixture(wrongIdentity: true))
    await #expect(throws: ClientError.self) { try await rpc.connect() }
    await rpc.close()
  }
  @Test func idleConnectionExpiresWithoutANewerGrant() async throws {
    let rpc = client(AccountFixture(renews: false))
    try await rpc.connect()
    try await Task.sleep(for: .milliseconds(2600))
    await #expect(throws: ClientError.self) { _ = try await rpc.call("EngineInfo") }
    await rpc.close()
  }
  @Test func refusesRemotePlaintextAndRedactsGrant() throws {
    let value = AccountGrant(
      environmentID: "host", origin: "http://host.example", token: "private-secret",
      expiresAt: Int(Date().timeIntervalSince1970 * 1000) + 60_000)
    #expect(throws: ClientError.self) { _ = try value.socketURL() }
    #expect(!value.description.contains("private-secret"))
  }
}
