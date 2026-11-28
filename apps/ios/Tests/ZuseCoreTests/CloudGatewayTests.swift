import Foundation
import Testing

@testable import ZuseCore

@Suite struct CloudGatewayTests {
  @Test func validatesWorkspaceProtocolRouteGenerationAndExpiry() throws {
    let origin = URL(string: "https://cloud.example")!
    let ticket: [String: JSONValue] = [
      "workspaceId": .string("workspace"),
      "wsUrl": .string("wss://cloud.example/v1/cloud/workspaces/workspace/gateway"),
      "protocol": .string(CloudGateway.rust), "rpcProtocol": .string(CloudGateway.rpc),
      "credential": .string("private-ticket"), "generation": .number(1), "gatewayEpoch": .number(1),
      "expiresAt": .number(Date().timeIntervalSince1970 * 1000 + 60_000),
    ]
    _ = try CloudGateway.validate(
      .object(ticket), workspaceID: "workspace", protocolName: CloudGateway.rust, origin: origin)
    #expect(
      try CloudGateway.protocolName(.object(["rpcProtocol": .string(CloudGateway.rpc)]))
        == CloudGateway.rust)
    let invalid: [(String, JSONValue)] = [
      ("workspaceId", .string("other")), ("rpcProtocol", .string(CloudGateway.effect)),
      ("generation", .number(0)), ("gatewayEpoch", .number(0)), ("expiresAt", .number(0)),
      ("wsUrl", .string("wss://cloud.example/other?token=leak")),
      // The gateway must live on the API origin itself, not another host or port.
      ("wsUrl", .string("wss://attacker.example/v1/cloud/workspaces/workspace/gateway")),
      ("wsUrl", .string("wss://cloud.example:8443/v1/cloud/workspaces/workspace/gateway")),
      ("wsUrl", .string("ws://cloud.example/v1/cloud/workspaces/workspace/gateway")),
    ]
    for (key, value) in invalid {
      var changed = ticket
      changed[key] = value
      #expect(throws: ClientError.self) {
        _ = try CloudGateway.validate(
          .object(changed), workspaceID: "workspace", protocolName: CloudGateway.rust,
          origin: origin)
      }
    }
    #expect(throws: ClientError.self) {
      _ = try CloudGateway.protocolName(.object(["rpcProtocol": .string("unknown")]))
    }
  }
}
