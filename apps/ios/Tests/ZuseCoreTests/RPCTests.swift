import Foundation
import Testing

@testable import ZuseCore

@Suite(.enabled(if: ProcessInfo.processInfo.environment["ZUSE_RPC_TEST_URL"] != nil))
struct RPCTests {
  func client() async throws -> RPCClient {
    let address = try #require(ProcessInfo.processInfo.environment["ZUSE_RPC_TEST_URL"])
    let url = try #require(URL(string: address))
    let client = try RPCClient(url: url)
    try await client.connect()
    return client
  }
  @Test func handshakeAndUnaryCall() async throws {
    let client = try await client()
    let response = try await client.call("fixture.echo", ["value": .string("hello")])
    #expect(response["value"].string == "hello")
    await client.close()
  }
  @Test func streamPreservesChunkOrder() async throws {
    let client = try await client()
    let stream = try await client.stream("fixture.stream")
    var values: [Int] = []
    for try await value in stream { if let number = value.int { values.append(number) } }
    #expect(values == [1, 2, 3])
    await client.close()
  }
  @Test func failuresAreSurfaced() async throws {
    let client = try await client()
    await #expect(throws: ClientError.self) { _ = try await client.call("fixture.failure") }
    await client.close()
  }
  @Test func socketCloseFailsOutstandingRequest() async throws {
    let client = try await client()
    await #expect(throws: (any Error).self) { _ = try await client.call("fixture.close") }
    await client.close()
  }
  @Test func chunkedAttachmentRoundTrip() async throws {
    let client = try await client()
    let bytes = Data((0..<(Attachments.chunkBytes + 37)).map { UInt8($0 % 251) })
    let reference = try await Attachments.upload(
      bytes, name: "example.bin", mimeType: "application/octet-stream", sessionID: "s", rpc: client)
    let id = try #require(reference["id"].string)
    let (downloaded, _, name) = try await Attachments.read(id, sessionID: "s", rpc: client)
    #expect(downloaded == bytes)
    #expect(name == "example.bin")
    await client.close()
  }
}
