import Foundation
import Security

/// Credentials are sealed to the cloud authority; the control plane receives only ciphertext.
public enum CloudSecret {
  public static func seal(_ secret: String, publicJWK: String) throws -> String {
    let jwk = try JSONDecoder().decode(JSONValue.self, from: Data(publicJWK.utf8))
    guard jwk["kty"].string == "RSA", let n = decode(jwk["n"].string),
      let e = decode(jwk["e"].string), n.count >= 256
    else { throw ClientError("The cloud encryption key is invalid.") }
    let encoded = der(tag: 0x30, bytes: integer(n) + integer(e))
    let attributes: [String: Any] = [
      kSecAttrKeyType as String: kSecAttrKeyTypeRSA,
      kSecAttrKeyClass as String: kSecAttrKeyClassPublic,
    ]
    var error: Unmanaged<CFError>?
    guard let key = SecKeyCreateWithData(encoded as CFData, attributes as CFDictionary, &error),
      SecKeyIsAlgorithmSupported(key, .encrypt, .rsaEncryptionOAEPSHA256)
    else { throw ClientError("The cloud encryption key is unsupported.") }
    guard
      let ciphertext = SecKeyCreateEncryptedData(
        key, .rsaEncryptionOAEPSHA256, Data(secret.utf8) as CFData, &error) as Data?
    else {
      throw ClientError("This credential could not be encrypted. Check its length and format.")
    }
    return ciphertext.base64EncodedString().replacingOccurrences(of: "+", with: "-")
      .replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
  }
  private static func decode(_ value: String?) -> Data? {
    guard var value else { return nil }
    value = value.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    value += String(repeating: "=", count: (4 - value.count % 4) % 4)
    return Data(base64Encoded: value)
  }
  private static func integer(_ data: Data) -> Data {
    var bytes = data
    while bytes.count > 1 && bytes.first == 0 { bytes.removeFirst() }
    if let first = bytes.first, first & 0x80 != 0 { bytes.insert(0, at: bytes.startIndex) }
    return der(tag: 0x02, bytes: bytes)
  }
  private static func der(tag: UInt8, bytes: Data) -> Data {
    var header = Data([tag])
    if bytes.count < 128 {
      header.append(UInt8(bytes.count))
    } else {
      var count = bytes.count
      var length: [UInt8] = []
      while count > 0 {
        length.insert(UInt8(count & 255), at: 0)
        count >>= 8
      }
      header.append(0x80 | UInt8(length.count))
      header.append(contentsOf: length)
    }
    return header + bytes
  }
}
