import Foundation
import Testing

@testable import ZuseCore

@Test func nativeAuthorizationUsesApprovedMobileRedirectAndPKCE() throws {
  let url = try AccountAuthorization.url(clientID: "client", challenge: "challenge", state: "state")
  let items = try #require(URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems)
  #expect(url.host == "api.workos.com")
  #expect(items.first { $0.name == "redirect_uri" }?.value == "zuse://auth")
  #expect(items.first { $0.name == "code_challenge_method" }?.value == "S256")
  #expect(items.first { $0.name == "code_challenge" }?.value == "challenge")
  #expect(items.first { $0.name == "state" }?.value == "state")
  for callback in [
    "zuse://auth?code=one&state=expected", "zuse://auth/?code=one&state=expected",
    "zuse://auth?code=one&state=expected#",
  ] {
    #expect(
      try AccountAuthorization.code(from: URL(string: callback)!, expectedState: "expected")
        == "one")
  }
}

@Test func nativeAuthorizationRejectsUntrustedAndAmbiguousCallbacks() {
  for callback in [
    "zuse://auth?code=one&state=wrong", "zuse://auth?code=one",
    "zuse://auth?code=one&state=expected&state=wrong",
    "zuse://auth?code=one&code=two&state=expected", "zuse://auth?code=&state=expected",
    "zuse://other?code=one&state=expected", "zuse://auth/other?code=one&state=expected",
    "https://auth?code=one&state=expected", "zuse://user@auth?code=one&state=expected",
    "zuse://auth?code=one&state=expected#fragment",
    "zuse://auth?code=one&state=expected&error=access_denied",
  ] {
    #expect(throws: ClientError.self) {
      try AccountAuthorization.code(from: URL(string: callback)!, expectedState: "expected")
    }
  }
}
