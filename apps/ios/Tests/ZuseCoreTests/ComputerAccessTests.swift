import Foundation
import Testing

@testable import ZuseCore

@Test func pairingInvitationsKeepTheServerCodeAndExpiryTogether() throws {
  let result: JSONValue = .object([
    "browserUrl": .string("https://computer.tail.ts.net/#pair=ABCD2345"),
    "code": .string("ABCD2345"), "expiresAt": .string("2026-10-17T12:00:00.123Z"),
  ])
  let now = try #require(ServerTimestamp.parse("2026-10-17T11:59:00Z"))
  let invitation = try PairingInvitation(result, now: now)
  #expect(invitation.browserURL.host == "computer.tail.ts.net")
  #expect(invitation.code == "ABCD2345")
  #expect(invitation.expiresAt.timeIntervalSince(now) > 60)
  #expect(throws: ClientError.self) {
    _ = try PairingInvitation(result.setting("code", to: .string("OTHER")), now: now)
  }
}

@Test func expiredOrInsecurePairingInvitationsCannotBeShared() throws {
  let now = try #require(ServerTimestamp.parse("2026-10-17T12:00:00Z"))
  for (link, expires) in [
    ("https://computer.example/#pair=ABCD2345", "2026-10-17T11:00:00Z"),
    ("http://public.example/#pair=ABCD2345", "2026-10-17T13:00:00Z"),
    ("https://computer.example/#pair=ABCD2345", "invalid"),
    ("https://computer.example/", "2026-10-17T13:00:00Z"),
  ] {
    #expect(throws: ClientError.self) {
      _ = try PairingInvitation(
        .object([
          "browserUrl": .string(link), "expiresAt": .string(expires), "code": .string("ABCD2345"),
        ]), now: now)
    }
  }
}

@Test func pairedDevicesPreserveRevocationAndUsageInformation() throws {
  let devices = try JSONDecoder().decode(
    [PairedDevice].self,
    from: Data(
      #"""
      [{"id":"phone-token","deviceId":"phone","label":"My iPhone","createdAt":"2026-10-17T11:00:00.000Z","lastUsedAt":"2026-10-17T12:00:00Z"},
       {"id":"old-token","createdAt":"2026-10-17T11:00:00Z","revokedAt":"2026-10-17T12:00:00.000Z"}]
      """#.utf8))
  #expect(devices.filter { $0.revokedAt == nil }.map(\.id) == ["phone-token"])
  #expect(devices[0].displayName == "My iPhone")
  #expect(ServerTimestamp.parse(try #require(devices[0].lastUsedAt)) != nil)
  #expect(devices[1].displayName == "Paired device")
}
