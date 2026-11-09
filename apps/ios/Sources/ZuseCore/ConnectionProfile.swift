import Foundation

/// Credentials are encoded only into the device's Keychain by the application.
public struct RemoteConnection: Codable, Identifiable, Equatable, Sendable {
  public var address: String
  public var token: String
  public var pairingLink: String?
  public var certificatePin: String?
  public var environmentID: String?
  public var label: String?
  public var id: String {
    // The endpoint distinguishes separate transports to the same environment.
    // Tokens never participate in display names, identities, or archive paths.
    guard var url = URLComponents(string: address) else { return address }
    url.query = nil
    url.fragment = nil
    url.user = nil
    url.password = nil
    return url.string ?? address
  }
  public var displayName: String {
    if let label, !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return label }
    return URLComponents(string: address)?.host ?? "Computer"
  }
  public var transportName: String {
    let host = URLComponents(string: address)?.host?.lowercased() ?? ""
    if host.hasSuffix(".ts.net") { return "Tailscale" }
    if PairingLink.isPrivateHost(host) { return "Local network" }
    return "Remote"
  }
  public init(
    address: String, token: String, certificatePin: String? = nil,
    environmentID: String? = nil, label: String? = nil, pairingLink: String? = nil
  ) {
    self.address = address
    self.token = token
    self.pairingLink = pairingLink
    self.certificatePin = certificatePin
    self.environmentID = environmentID
    self.label = label
  }
  public func validated() throws -> RemoteConnection {
    guard let url = URL(string: address), url.fragment == nil,
      URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.contains(where: {
        $0.name == "token"
      }) != true
    else { throw ClientError("Enter the endpoint without a token or pairing code in the address.") }
    try PairingLink.validate(url, webSocket: true)
    if certificatePin != nil && url.scheme != "wss" {
      throw ClientError("A pinned computer requires a secure WebSocket connection.")
    }
    return self
  }
}

public enum ConnectionProfiles {
  public static func saving(_ connection: RemoteConnection, in profiles: [RemoteConnection])
    -> [RemoteConnection]
  {
    var result = profiles.filter { $0.id != connection.id }
    result.insert(connection, at: 0)
    return result
  }
}
