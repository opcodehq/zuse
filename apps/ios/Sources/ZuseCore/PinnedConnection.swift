import CryptoKit
import Foundation
import Security

/// A private self-signed server is trusted only when its exact certificate matches the paired pin.
public final class PinnedConnection: NSObject, URLSessionDelegate, URLSessionTaskDelegate, Sendable
{
  private let pin: String
  public init(pin: String) throws {
    guard pin.count == 43,
      pin.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_") })
    else { throw ClientError("Invalid computer certificate pin.") }
    self.pin = pin
  }
  public static func session(pin: String) throws -> URLSession {
    URLSession(
      configuration: .ephemeral, delegate: try PinnedConnection(pin: pin), delegateQueue: nil)
  }
  public func urlSession(
    _ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
    completionHandler:
      @escaping @Sendable (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
  ) {
    guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
      let trust = challenge.protectionSpace.serverTrust,
      let certificates = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
      let certificate = certificates.first
    else {
      completionHandler(.cancelAuthenticationChallenge, nil)
      return
    }
    let digest = Data(SHA256.hash(data: SecCertificateCopyData(certificate) as Data))
      .base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(
        of: "/", with: "_"
      ).replacingOccurrences(of: "=", with: "")
    guard digest == pin else {
      completionHandler(.cancelAuthenticationChallenge, nil)
      return
    }
    completionHandler(.useCredential, URLCredential(trust: trust))
  }
  public func urlSession(
    _ session: URLSession, task: URLSessionTask,
    willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
    completionHandler: @escaping @Sendable (URLRequest?) -> Void
  ) { completionHandler(nil) }
}
