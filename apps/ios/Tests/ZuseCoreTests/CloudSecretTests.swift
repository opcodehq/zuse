import Foundation
import Security
import Testing

@testable import ZuseCore

@Test func cloudCredentialUsesAuthorityRSAOAEPKey() throws {
  let attributes: [String: Any] = [
    kSecAttrKeyType as String: kSecAttrKeyTypeRSA, kSecAttrKeySizeInBits as String: 2048,
  ]
  let privateKey = try #require(SecKeyCreateRandomKey(attributes as CFDictionary, nil))
  let publicKey = try #require(SecKeyCopyPublicKey(privateKey))
  let der = [UInt8](try #require(SecKeyCopyExternalRepresentation(publicKey, nil) as Data?))
  var offset = 0
  func header(_ tag: UInt8) throws -> Int {
    #expect(der[offset] == tag)
    offset += 1
    let first = Int(der[offset])
    offset += 1
    if first < 128 { return first }
    var length = 0
    for _ in 0..<(first & 127) {
      length = length * 256 + Int(der[offset])
      offset += 1
    }
    return length
  }
  _ = try header(0x30)
  let nLength = try header(0x02)
  var n = Data(der[offset..<(offset + nLength)])
  offset += nLength
  if n.first == 0 { n.removeFirst() }
  let eLength = try header(0x02)
  let e = Data(der[offset..<(offset + eLength)])
  func encode(_ data: Data) -> String {
    data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(
      of: "/", with: "_"
    ).replacingOccurrences(of: "=", with: "")
  }
  let jwk = JSONValue.object([
    "kty": .string("RSA"), "n": .string(encode(n)), "e": .string(encode(e)),
  ]).pretty
  let encrypted = try CloudSecret.seal("example-test-credential", publicJWK: jwk)
  var base64 = encrypted.replacingOccurrences(of: "-", with: "+").replacingOccurrences(
    of: "_", with: "/")
  base64 += String(repeating: "=", count: (4 - base64.count % 4) % 4)
  let cipher = try #require(Data(base64Encoded: base64))
  let clear = try #require(
    SecKeyCreateDecryptedData(privateKey, .rsaEncryptionOAEPSHA256, cipher as CFData, nil) as Data?)
  #expect(String(decoding: clear, as: UTF8.self) == "example-test-credential")
}
