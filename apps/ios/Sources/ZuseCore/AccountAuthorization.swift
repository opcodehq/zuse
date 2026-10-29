import Foundation

/// Uses the same approved redirect as the existing mobile client. AuthenticationServices
/// routes the response to its initiating session without requiring a global URL handler.
public enum AccountAuthorization {
  public static let callbackScheme = "zuse"
  public static let redirectURI = "zuse://auth"

  public static func url(clientID: String, challenge: String, state: String) throws -> URL {
    guard !clientID.isEmpty, !challenge.isEmpty, !state.isEmpty else {
      throw ClientError("Sign-in is not configured in this build.")
    }
    var url = URLComponents(string: "https://api.workos.com/user_management/authorize")!
    url.queryItems = [
      "client_id": clientID, "redirect_uri": redirectURI, "response_type": "code",
      "provider": "authkit", "scope": "openid profile email offline_access",
      "code_challenge": challenge, "code_challenge_method": "S256", "state": state,
    ].map { URLQueryItem(name: $0.key, value: $0.value) }
    guard let result = url.url else { throw ClientError("Could not prepare sign-in.") }
    return result
  }

  public static func code(from callback: URL, expectedState: String) throws -> String {
    guard let url = URLComponents(url: callback, resolvingAgainstBaseURL: false),
      url.scheme == callbackScheme, url.host == "auth", url.path.isEmpty || url.path == "/",
      url.user == nil, url.password == nil, url.port == nil, url.fragment?.isEmpty ?? true
    else { throw ClientError("The sign-in response could not be verified.") }
    let items = url.queryItems ?? []
    let states = items.filter { $0.name == "state" }
    guard !expectedState.isEmpty, states.count == 1, states[0].value == expectedState else {
      throw ClientError("The sign-in response could not be verified.")
    }
    guard !items.contains(where: { $0.name == "error" }) else {
      throw ClientError("Sign-in was declined or cancelled. Please try again.")
    }
    let codes = items.filter { $0.name == "code" }
    guard codes.count == 1, let code = codes[0].value, !code.isEmpty else {
      throw ClientError("The sign-in response could not be verified.")
    }
    return code
  }
}
