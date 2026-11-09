import Foundation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// Redirects are rejected so a one-time code cannot be forwarded to another server.
public final class PairingHTTPDelegate: NSObject, URLSessionTaskDelegate, Sendable {
  public func urlSession(
    _ session: URLSession, task: URLSessionTask,
    willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
    completionHandler: @escaping @Sendable (URLRequest?) -> Void
  ) { completionHandler(nil) }
}
