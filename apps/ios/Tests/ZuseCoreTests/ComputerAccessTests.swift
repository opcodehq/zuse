import Foundation
import Testing

@testable import ZuseCore

private let fragment =
  "#pair=eyJ2IjoxLCJpZCI6IjAxMjM0NTY3ODlhYmNkZWYwMTIzNDU2Nzg5YWJjZGVmIiwic2VjcmV0IjoiYWJjIn0"

@Test func pairingInvitationsJoinTheOriginAndFragment() throws {
  let now = Date(timeIntervalSince1970: 1_000)
  let invitation = try PairingInvitation(
    .object([
      "peer": .object(["id": .string("pair-1"), "expiresAt": .number(1_600)]),
      "fragment": .string(fragment), "origin": .string("http://100.101.102.103:27657"),
    ]), now: now)
  #expect(invitation.pairID == "pair-1")
  #expect(invitation.link.absoluteString == "http://100.101.102.103:27657/" + fragment)
  #expect(invitation.expiresAt.timeIntervalSince(now) == 600)
}

@Test func pairingInvitationsNeedAReachableCurrentLink() throws {
  let now = Date(timeIntervalSince1970: 1_000)
  let peer: JSONValue = .object(["id": .string("pair-1"), "expiresAt": .number(1_600)])
  for result: JSONValue in [
    .object(["peer": peer, "fragment": .string(fragment), "origin": .null]),
    .object([
      "peer": peer, "fragment": .string("#pair=not-json"), "origin": .string("http://10.0.0.2"),
    ]),
    .object([
      "peer": .object(["id": .string("pair-1"), "expiresAt": .number(900)]),
      "fragment": .string(fragment), "origin": .string("http://10.0.0.2"),
    ]),
  ] {
    #expect(throws: ClientError.self) { _ = try PairingInvitation(result, now: now) }
  }
}

@Test func pairedDevicesDecodeTheEnginePeerList() throws {
  let devices = try JSONDecoder().decode(
    [PairedDevice].self,
    from: Data(
      #"""
      [{"id":"a","label":"My iPhone","createdAt":10,"expiresAt":null,"pairedAt":20},
       {"id":"b","label":"","createdAt":30,"expiresAt":630,"pairedAt":null}]
      """#.utf8))
  #expect(devices.filter(\.isPaired).map(\.id) == ["a"])
  #expect(devices[0].pairedDate == Date(timeIntervalSince1970: 20))
  #expect(devices[1].displayName == "Paired device")
}
