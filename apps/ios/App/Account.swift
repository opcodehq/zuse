import AuthenticationServices
import CryptoKit
import Security
import SwiftUI
import ZuseCore

#if os(macOS)
  import AppKit
#endif

struct Credentials: Codable, Sendable {
  let accessToken: String
  let refreshToken: String
  enum CodingKeys: String, CodingKey {
    case accessToken = "access_token"
    case refreshToken = "refresh_token"
  }
  var claims: JSONValue? {
    let parts = accessToken.split(separator: ".")
    guard parts.count > 1 else { return nil }
    var encoded = String(parts[1]).replacingOccurrences(of: "-", with: "+").replacingOccurrences(
      of: "_", with: "/")
    encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
    guard let data = Data(base64Encoded: encoded) else { return nil }
    return try? JSONDecoder().decode(JSONValue.self, from: data)
  }
  var expiresAt: Date {
    guard let expiry = claims?["exp"].int else { return .distantPast }
    return Date(timeIntervalSince1970: Double(expiry))
  }

}

enum Vault {
  static func read(_ key: String) throws -> Data? {
    var query = base(key)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var value: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &value)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess else {
      throw ClientError("Could not read credentials from Keychain.")
    }
    return value as? Data
  }
  static func write(_ data: Data?, key: String) throws {
    let query = base(key)
    if let data {
      let status = SecItemUpdate(
        query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
      if status == errSecItemNotFound {
        var entry = query
        entry[kSecValueData as String] = data
        entry[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        guard SecItemAdd(entry as CFDictionary, nil) == errSecSuccess else {
          throw ClientError("Could not save credentials to Keychain.")
        }
        return
      }
      guard status == errSecSuccess else { throw ClientError("Could not update credentials.") }
    } else {
      let status = SecItemDelete(query as CFDictionary)
      guard status == errSecSuccess || status == errSecItemNotFound else {
        throw ClientError("Could not remove credentials.")
      }
    }
  }
  private static func base(_ key: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "sh.zuse.native",
      kSecAttrAccount as String: key,
    ]
  }
}

@MainActor @Observable
final class Account: NSObject, ASWebAuthenticationPresentationContextProviding {
  private(set) var signedIn = false
  private(set) var reauthenticationRequired = false
  private var credentials: Credentials?
  private var webSession: ASWebAuthenticationSession?
  private var refresh: Task<Credentials, Error>?
  private(set) var generation = UUID()
  private let key = "account"
  var archiveIdentity: String? {
    guard let claims = credentials?.claims, let subject = claims["sub"].string else { return nil }
    return "\(apiURL.absoluteString)|\(subject)|\(claims["org_id"].string ?? "personal")"
  }
  // Public configuration is copied from the shared deployment contract at project generation.
  private var clientID: String {
    Bundle.main.object(forInfoDictionaryKey: "WorkOSClientID") as? String ?? ""
  }
  var apiURL: URL {
    URL(
      string: Bundle.main.object(forInfoDictionaryKey: "ZuseAPIURL") as? String
        ?? "https://api.zuse.sh")!
  }
  override init() {
    super.init()
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--native-preview")
        || ProcessInfo.processInfo.arguments.contains("--rpc-fixture")
      {
        return
      }
    #endif
    if let data = try? Vault.read(key),
      let stored = try? JSONDecoder().decode(Credentials.self, from: data)
    {
      credentials = stored
      signedIn = true
    }
  }
  func token(expectedGeneration: UUID? = nil) async throws -> String {
    guard expectedGeneration == nil || expectedGeneration == generation else {
      throw CancellationError()
    }
    guard let credentials else { throw ClientError("Sign in to connect to Zuse Cloud.") }
    if credentials.expiresAt.timeIntervalSinceNow > 60 { return credentials.accessToken }
    let epoch = generation
    let task: Task<Credentials, Error>
    if let refresh {
      task = refresh
    } else {
      task = Task {
        try await self.authenticate([
          "grant_type": "refresh_token", "refresh_token": credentials.refreshToken,
        ])
      }
      refresh = task
    }
    defer { if generation == epoch { refresh = nil } }
    let updated: Credentials
    do { updated = try await task.value } catch let failure as ClientError
      where failure.requiresUserAction
    {
      guard generation == epoch else { throw CancellationError() }
      reauthenticationRequired = true
      throw failure
    }
    guard generation == epoch else { throw CancellationError() }
    try save(updated)
    return updated.accessToken
  }
  func signIn() async throws {
    guard webSession == nil else { return }
    let epoch = generation
    let verifier = try random()
    let state = try random()
    let challenge = Data(SHA256.hash(data: Data(verifier.utf8))).base64URL
    let url = try AccountAuthorization.url(clientID: clientID, challenge: challenge, state: state)
    defer { if generation == epoch { webSession = nil } }
    let callback: URL = try await withCheckedThrowingContinuation { continuation in
      let session = ASWebAuthenticationSession(
        url: url, callbackURLScheme: AccountAuthorization.callbackScheme
      ) {
        url, error in
        if let url {
          continuation.resume(returning: url)
        } else {
          continuation.resume(throwing: error ?? ClientError("Sign-in was cancelled."))
        }
      }
      session.presentationContextProvider = self
      webSession = session
      if !session.start() { continuation.resume(throwing: ClientError("Could not open sign-in.")) }
    }
    guard epoch == generation else { throw CancellationError() }
    try Task.checkCancellation()
    let code = try AccountAuthorization.code(from: callback, expectedState: state)
    let authenticated = try await authenticate([
      "grant_type": "authorization_code", "code": code, "code_verifier": verifier,
    ])
    guard epoch == generation else { throw CancellationError() }
    try save(authenticated)
    webSession = nil
    refresh?.cancel()
    refresh = nil
    generation = UUID()
  }
  func recordAuthenticationFailure(_ failure: Error) {
    if (failure as? ClientError)?.code == "authentication-required" {
      reauthenticationRequired = true
    }
  }
  func signOut() throws {
    generation = UUID()
    refresh?.cancel()
    refresh = nil
    webSession?.cancel()
    webSession = nil
    try Vault.write(nil, key: key)
    credentials = nil
    signedIn = false
    reauthenticationRequired = false
  }
  private func save(_ value: Credentials) throws {
    try Vault.write(JSONEncoder().encode(value), key: key)
    credentials = value
    signedIn = true
    reauthenticationRequired = false
  }
  private func authenticate(_ body: [String: String]) async throws -> Credentials {
    var body = body
    body["client_id"] = clientID
    var request = URLRequest(
      url: URL(string: "https://api.workos.com/user_management/authenticate")!)
    request.httpMethod = "POST"
    request.timeoutInterval = 30
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = try JSONEncoder().encode(body)
    let (data, response) = try await URLSession.shared.data(for: request)
    guard let response = response as? HTTPURLResponse else {
      throw ClientError("Invalid sign-in response.")
    }
    guard (200..<300).contains(response.statusCode) else {
      throw ClientError(
        "Sign-in expired or was rejected. Please sign in again.",
        code: [400, 401, 403].contains(response.statusCode) ? "authentication-required" : nil)
    }
    return try JSONDecoder().decode(Credentials.self, from: data)
  }
  func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
    #if os(macOS)
      NSApplication.shared.keyWindow ?? NSApplication.shared.windows.first ?? ASPresentationAnchor()
    #else
      UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
        .first(where: \.isKeyWindow) ?? ASPresentationAnchor()
    #endif
  }
  private func random() throws -> String {
    var bytes = [UInt8](repeating: 0, count: 32)
    guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
      throw ClientError("Could not start secure sign-in.")
    }
    return Data(bytes).base64URL
  }
}
extension Data {
  fileprivate var base64URL: String {
    base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(
      of: "/", with: "_"
    ).replacingOccurrences(of: "=", with: "")
  }
}
