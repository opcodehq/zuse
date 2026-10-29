import Foundation

#if canImport(CryptoKit)
  import CryptoKit
#else
  import Crypto
#endif
#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

public struct RemoteEnvironment: Codable, Identifiable, Sendable {
  public let environmentId: String
  public let label: String?
  public let providerKind: String
  public let wireProtocolVersion: Int?
  public let capabilities: JSONValue?
  public var id: String { environmentId }
  public var canonicalAccount: Bool {
    wireProtocolVersion == 1
      && capabilities?["features"].array.contains(.string("zuse-rpc-json-v1")) == true
  }
}

public struct DeviceProof: Sendable {
  private let key: P256.Signing.PrivateKey
  public init(rawKey: Data) throws { key = try P256.Signing.PrivateKey(rawRepresentation: rawKey) }
  public static func generateKey() -> Data { P256.Signing.PrivateKey().rawRepresentation }
  public var publicJWK: JSONValue {
    let point = key.publicKey.x963Representation
    return .object([
      "kty": .string("EC"), "crv": .string("P-256"),
      "x": .string(Self.encode(point.subdata(in: 1..<33))),
      "y": .string(Self.encode(point.subdata(in: 33..<65))),
    ])
  }
  public func sign(
    method: String, url: URL, accessToken: String? = nil, now: Date = Date(),
    nonce: String = UUID().uuidString
  )
    throws -> String
  {
    guard var target = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
      throw ClientError("Invalid proof URL.")
    }
    target.query = nil
    target.fragment = nil
    guard let address = target.url else { throw ClientError("Invalid proof URL.") }
    let point = key.publicKey.x963Representation
    let header: JSONValue = .object([
      "alg": .string("ES256"), "typ": .string("dpop+jwt"),
      "jwk": .object([
        "kty": .string("EC"), "crv": .string("P-256"),
        "x": .string(Self.encode(point.subdata(in: 1..<33))),
        "y": .string(Self.encode(point.subdata(in: 33..<65))),
      ]),
    ])
    var claims: [String: JSONValue] = [
      "htm": .string(method.uppercased()), "htu": .string(address.absoluteString),
      "iat": .number(floor(now.timeIntervalSince1970)), "jti": .string(nonce),
    ]
    if let accessToken {
      claims["ath"] = .string(Self.encode(Data(SHA256.hash(data: Data(accessToken.utf8)))))
    }
    let payload = JSONValue.object(claims)
    let input =
      try Self.encode(JSONEncoder().encode(header)) + "."
      + Self.encode(JSONEncoder().encode(payload))
    return try input + "." + Self.encode(key.signature(for: Data(input.utf8)).rawRepresentation)
  }
  private static func encode(_ data: Data) -> String {
    data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(
      of: "/", with: "_"
    ).replacingOccurrences(of: "=", with: "")
  }
}

extension CloudClient {
  public func environments() async throws -> [RemoteEnvironment] {
    try await request("/v1/environments")["environments"].decode([RemoteEnvironment].self)
  }
  public func connectEnvironment(_ id: String, proof: DeviceProof, canonicalAccount: Bool = false)
    async throws -> RPCClient
  {
    if canonicalAccount {
      let rpc = RPCClient(
        accountEnvironmentID: id,
        grant: {
          let device = try await request("/v1/client/dpop-token", method: "POST", proof: proof)
          guard let access = device["accessToken"].string else {
            throw ClientError("Missing device grant.")
          }
          let encoder = JSONEncoder()
          encoder.outputFormatting = [.sortedKeys]
          let keyID = Data(SHA256.hash(data: try encoder.encode(proof.publicJWK))).map {
            String(format: "%02x", $0)
          }.joined()
          _ = try await request(
            "/v1/mobile/devices", method: "POST",
            body: .object([
              "deviceId": .string("ios-" + keyID), "platform": .string("ios"),
              "dpopJwk": proof.publicJWK,
            ]), proof: proof, deviceToken: access)
          let encoded = id.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
          let connection = try await request(
            "/v1/environments/\(encoded)/connect", method: "POST",
            body: .object(["requireManaged": .bool(true), "wireProtocolVersion": .number(1)]),
            proof: proof, deviceToken: access)
          guard let endpoint = connection["endpoint"]["wsBaseUrl"].string,
            endpoint.hasPrefix("wss://"),
            let token = connection["connectToken"].string, let expiry = connection["expiresAt"].int
          else { throw ClientError("Missing managed computer endpoint.") }
          return AccountGrant(
            environmentID: id, origin: "https://" + endpoint.dropFirst(6), token: token,
            expiresAt: expiry)
        }, session: session)
      do {
        try await rpc.connect()
        return rpc
      } catch {
        await rpc.close()
        throw error
      }
    }
    let grant = try await request("/v1/client/dpop-token", method: "POST", proof: proof)
    guard let access = grant["accessToken"].string else {
      throw ClientError("Invalid device grant.")
    }
    let encoded = id.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
    let connection = try await request(
      "/v1/environments/\(encoded)/connect", method: "POST", proof: proof, deviceToken: access)
    guard let address = connection["endpoint"]["wsBaseUrl"].string,
      var url = URLComponents(string: address), url.scheme == "wss",
      let token = connection["connectToken"].string
    else { throw ClientError("The computer did not return a secure endpoint.") }
    url.queryItems =
      (url.queryItems ?? []).filter { $0.name != "token" } + [
        URLQueryItem(name: "token", value: token)
      ]
    guard let endpoint = url.url else { throw ClientError("Invalid computer endpoint.") }
    let rpc = try RPCClient(url: endpoint)
    do {
      try await rpc.connect()
      let descriptor = try await rpc.call("connect.describe")
      guard descriptor["environmentId"].string == id else {
        throw ClientError(
          "The connected computer does not match the selected account device.",
          code: "identity-changed")
      }
      return rpc
    } catch {
      await rpc.close()
      throw error
    }
  }
}
