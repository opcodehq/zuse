import Foundation

/// A device paired with a computer, as `ListPairings` reports it (`PeerMetadata`).
/// Timestamps are Unix seconds. Unredeemed invitations carry an expiry.
public struct PairedDevice: Decodable, Identifiable, Sendable, Equatable {
  public let id: String
  public let label: String
  public let createdAt: UInt64
  public let expiresAt: UInt64?
  public let pairedAt: UInt64?

  public var displayName: String { label.isEmpty ? "Paired device" : label }
  public var isPaired: Bool { pairedAt != nil }
  public var pairedDate: Date? { pairedAt.map { Date(timeIntervalSince1970: TimeInterval($0)) } }
}

/// A fresh invitation from `CreatePairing`: one link another device opens or scans.
public struct PairingInvitation: Sendable, Equatable {
  public let pairID: String
  public let link: URL
  public let expiresAt: Date

  public init(_ result: JSONValue, now: Date = Date()) throws {
    guard let pairID = result["peer"]["id"].string, !pairID.isEmpty,
      let fragment = result["fragment"].string, fragment.hasPrefix("#pair="),
      let origin = result["origin"].string, let expiry = result["peer"]["expiresAt"].int
    else {
      throw ClientError(
        "This computer has no address another device can reach. Turn on Wi-Fi or Tailscale access in its Remote access settings."
      )
    }
    let address = origin.hasSuffix("/") ? origin + fragment : origin + "/" + fragment
    guard let link = URL(string: address), PairingLink.isNativeInvitation(address) else {
      throw ClientError("The computer returned an invalid pairing link. Try again.")
    }
    let expiresAt = Date(timeIntervalSince1970: TimeInterval(expiry))
    guard expiresAt > now else {
      throw ClientError("The computer returned an expired pairing link. Try again.")
    }
    self.pairID = pairID
    self.link = link
    self.expiresAt = expiresAt
  }

  /// The label the computer shows for the device that redeems this invitation.
  public static func label(at date: Date = Date()) -> String {
    "Device added " + date.formatted(.dateTime.month(.abbreviated).day().hour().minute())
  }
}
