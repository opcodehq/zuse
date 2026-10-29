import Foundation
import Testing

@testable import ZuseCore

@Test func rpcRepliesPreserveStructuredErrorsForRecovery() throws {
  let frame = try JSONDecoder().decode(
    JSONValue.self,
    from: Data(
      #"{"_tag":"Exit","requestId":"request","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"SessionNotFoundError","message":"No such chat"}}]}}"#
        .utf8))
  guard case .failure(let id, let error) = RPCReply(frame) else {
    Issue.record("Expected an RPC failure")
    return
  }
  #expect(id == "request")
  #expect(error.code == "SessionNotFoundError")
  #expect(error.localizedDescription == "No such chat")
}

@Test func rpcReplyChunksKeepOrderAndIgnoreUnrelatedFrames() throws {
  let frame: JSONValue = .object([
    "_tag": .string("Chunk"), "requestId": .string("stream"),
    "values": .array([.number(1), .number(2), .number(3)]),
  ])
  guard case .chunk(let id, let values) = RPCReply(frame) else {
    Issue.record("Expected a chunk")
    return
  }
  #expect(id == "stream")
  #expect(values.compactMap(\.int) == [1, 2, 3])
  #expect(RPCReply(.object(["_tag": .string("Pong")])) == nil)
}

@Test func permanentConnectionFailuresRequireAnExplicitAction() {
  #expect(ClientError("Sign in", code: "authentication-required").requiresUserAction)
  #expect(ClientError("Identity changed", code: "identity-changed").requiresUserAction)
  #expect(ClientError("Update", code: "protocol-incompatible").requiresUserAction)
  #expect(!ClientError("The connection timed out").requiresUserAction)
}
