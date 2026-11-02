import Foundation

public enum ServerTimestamp {
  public static func parse(_ value: String) -> Date? {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions.insert(.withFractionalSeconds)
    return formatter.date(from: value) ?? ISO8601DateFormatter().date(from: value)
  }
}

public struct PairedDevice: Decodable, Identifiable, Sendable {
  public let id: String
  public let deviceId: String?
  public let label: String?
  public let createdAt: String
  public let lastUsedAt: String?
  public let revokedAt: String?
  public var displayName: String { label ?? deviceId ?? "Paired device" }
}

public struct PairingInvitation: Sendable {
  public let browserURL: URL
  public let code: String
  public let expiresAt: Date
  public init(_ result: JSONValue, now: Date = Date()) throws {
    guard let address = result["browserUrl"].string, let url = URL(string: address),
      let code = result["code"].string, !code.isEmpty,
      let timestamp = result["expiresAt"].string, let expiration = ServerTimestamp.parse(timestamp),
      expiration > now
    else { throw ClientError("The computer did not return a current pairing code. Try again.") }
    let link = try PairingLink.parse(address)
    guard link.code == code else { throw ClientError("The pairing link and code did not match.") }
    browserURL = url
    self.code = code
    expiresAt = expiration
  }
}
