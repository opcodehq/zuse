import Foundation
import Testing

@testable import ZuseCore

#if canImport(CryptoKit)
  import CryptoKit
#else
  import Crypto
#endif

@Test func pairingLinksRequireSecurePublicTransport() throws {
  let link = try PairingLink.parse("https://computer.example/#pair=ABCD-1234")
  #expect(link.webSocketURL.absoluteString == "wss://computer.example/rpc")
  #expect(link.code == "ABCD-1234")
  #expect(throws: ClientError.self) {
    _ = try PairingLink.parse("http://computer.example/#pair=secret")
  }
  #expect(throws: ClientError.self) {
    _ = try PairingLink.parse("https://user:password@computer.example/#pair=secret")
  }
  #expect(throws: ClientError.self) { _ = try PairingLink.parse("https://computer.example/") }
  let local = try PairingLink.parse("http://192.168.1.4:4859/#pair=ABCD1234")
  #expect(local.httpURL.absoluteString == "http://192.168.1.4:4859")
}

@Test func deviceProofBindsMethodPathAndFreshNonce() throws {
  let raw = DeviceProof.generateKey()
  let proof = try DeviceProof(rawKey: raw)
  let url = try #require(URL(string: "https://api.example/v1/connect?secret=hidden#fragment"))
  let token = try proof.sign(
    method: "post", url: url, now: Date(timeIntervalSince1970: 1234), nonce: "unique")
  let pieces = token.split(separator: ".")
  func decode(_ part: Substring) throws -> Data {
    var string = String(part).replacingOccurrences(of: "-", with: "+").replacingOccurrences(
      of: "_", with: "/")
    string += String(repeating: "=", count: (4 - string.count % 4) % 4)
    return try #require(Data(base64Encoded: string))
  }
  #expect(pieces.count == 3)
  let payload = try JSONDecoder().decode(JSONValue.self, from: decode(pieces[1]))
  #expect(payload["htm"].string == "POST")
  #expect(payload["htu"].string == "https://api.example/v1/connect")
  #expect(payload["iat"].int == 1234)
  #expect(payload["jti"].string == "unique")
  #expect(payload["ath"] == .null)
  let bound = try proof.sign(method: "POST", url: url, accessToken: "device-access")
  let boundPieces = bound.split(separator: ".")
  let boundPayload = try JSONDecoder().decode(JSONValue.self, from: decode(boundPieces[1]))
  let expectedHash = Data(SHA256.hash(data: Data("device-access".utf8))).base64EncodedString()
    .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
    .replacingOccurrences(of: "=", with: "")
  #expect(boundPayload["ath"].string == expectedHash)
  let boundSignature = try P256.Signing.ECDSASignature(rawRepresentation: decode(boundPieces[2]))
  #expect(
    try P256.Signing.PrivateKey(rawRepresentation: raw).publicKey.isValidSignature(
      boundSignature, for: Data("\(boundPieces[0]).\(boundPieces[1])".utf8)))
  let signature = try P256.Signing.ECDSASignature(rawRepresentation: decode(pieces[2]))
  let key = try P256.Signing.PrivateKey(rawRepresentation: raw)
  #expect(key.publicKey.isValidSignature(signature, for: Data("\(pieces[0]).\(pieces[1])".utf8)))
  #expect(try proof.sign(method: "POST", url: url) != proof.sign(method: "POST", url: url))
}

@Test func catalogsRespectAvailabilityAndCuratedDefaults() throws {
  let data = Data(
    #"{"providers":{"agent":{"models":[{"id":"old","label":"Old","available":false},{"id":"current","label":"Current","defaultModel":true}],"defaultModelId":"old"}}}"#
      .utf8)
  let providers = try ModelProvider.parse(JSONDecoder().decode(JSONValue.self, from: data))
  #expect(providers.count == 1)
  #expect(providers[0].models.map(\.id) == ["current"])
  #expect(providers[0].defaultModel == "current")
}
