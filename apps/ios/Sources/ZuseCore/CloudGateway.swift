import Foundation

enum CloudGateway {
  static let rust = "zuse-workspace-rust-v1"
  static let rpc = "zuse-rpc-v1"
  static let legacy = "zuse-workspace-v3"
  static let effect = "zuse-effect-rpc-v1"
  static func protocolName(_ workspace: JSONValue) throws -> String {
    switch workspace["rpcProtocol"].string {
    case rpc: return rust
    case nil, effect: return legacy
    default: throw ClientError("This cloud runtime requires a newer app.")
    }
  }
  static func validate(_ ticket: JSONValue, workspaceID: String, protocolName: String) throws -> URL
  {
    guard let text = ticket["wsUrl"].string, let url = URLComponents(string: text),
      url.scheme == "wss", url.host != nil, url.user == nil, url.password == nil,
      url.query == nil, url.fragment == nil,
      url.path == "/v1/cloud/workspaces/\(workspaceID)/gateway",
      ticket["workspaceId"].string == workspaceID, ticket["protocol"].string == protocolName,
      ticket["rpcProtocol"].string == (protocolName == rust ? rpc : effect),
      (ticket["generation"].int ?? 0) >= 1, (ticket["gatewayEpoch"].int ?? 0) >= 1,
      (ticket["expiresAt"].int ?? 0) > Int(Date().timeIntervalSince1970 * 1000),
      let credential = ticket["credential"].string, !credential.isEmpty,
      credential.utf8.count <= 16_384,
      credential.range(of: "^[A-Za-z0-9._~-]+$", options: .regularExpression) != nil,
      let address = url.url
    else { throw ClientError("Invalid cloud connection ticket.") }
    return address
  }
}
