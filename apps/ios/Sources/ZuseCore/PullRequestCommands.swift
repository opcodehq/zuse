import Foundation

/// How a pull request's commits land on its base (`ChangeRequestMergeMethod`).
public enum PullRequestMergeMethod: String, Sendable, Hashable, CaseIterable, Identifiable {
  case squash, merge, rebase
  public var id: String { rawValue }
  public var title: String {
    switch self {
    case .squash: return "Squash and merge"
    case .merge: return "Create a merge commit"
    case .rebase: return "Rebase and merge"
    }
  }
  public var subtitle: String {
    switch self {
    case .squash: return "Combine every commit into one"
    case .merge: return "Keep all commits and their history"
    case .rebase: return "Replay each commit onto the base"
    }
  }
  /// GitHub's `autoMergeRequest.mergeMethod` spelling.
  public init?(github: String) {
    self.init(rawValue: github.lowercased())
  }
}

/// A change the user asked for explicitly (`ChangeRequestUpdate`, tagged by `kind`).
public enum PullRequestUpdate: Sendable, Hashable {
  case merge(PullRequestMergeMethod)
  case enableAutoMerge(PullRequestMergeMethod)
  case disableAutoMerge
  case markReady

  public var json: JSONValue {
    switch self {
    case .merge(let method):
      return .object(["kind": .string("merge"), "method": .string(method.rawValue)])
    case .enableAutoMerge(let method):
      return .object(["kind": .string("enableAutoMerge"), "method": .string(method.rawValue)])
    case .disableAutoMerge: return .object(["kind": .string("disableAutoMerge")])
    case .markReady: return .object(["kind": .string("markReady")])
    }
  }
  /// Merges land on whatever the head becomes, so they're pinned to the head on screen.
  var pinsHead: Bool {
    switch self {
    case .merge, .enableAutoMerge: return true
    case .disableAutoMerge, .markReady: return false
    }
  }
}

/// Request builders for the pull request methods. Writes are explicit user actions: they're
/// sent once and never retried, because a lost reply may still mean GitHub applied them.
public enum PullRequestCommands {
  public static let maximumCommentBytes = 60_000

  public struct Request: Sendable, Equatable {
    public let method: String
    public let payload: [String: JSONValue]
  }

  public static func validHead(_ oid: String) -> Bool {
    oid.utf8.count == 40 && oid.allSatisfy(\.isHexDigit)
  }

  // MARK: Reads

  public static func repository(cwd: String) -> Request {
    Request(method: "GetChangeRequestRepository", payload: ["cwd": .string(cwd)])
  }

  /// `cached` serves the engine's stored first page at once; `refresh` always asks GitHub.
  public static func listPage(
    repository: String, filter: PullRequestFilter, after: String? = nil, refresh: Bool = false,
    cached: Bool = false
  ) throws -> Request {
    guard PullRequestLink.validRepository(repository) else {
      throw ClientError("Repository must be owner/name.")
    }
    var payload: [String: JSONValue] = [
      "repository": .string(repository), "filter": .string(filter.rawValue),
      "refresh": .bool(refresh), "cached": .bool(cached),
    ]
    if let after { payload["after"] = .string(after) }
    return Request(method: "ListChangeRequestPage", payload: payload)
  }

  /// The full read, including review threads. `summary` reads skip the threads.
  public static func detail(_ link: PullRequestLink, refresh: Bool = false) -> Request {
    Request(
      method: "GetChangeRequest", payload: ["url": .string(link.url), "refresh": .bool(refresh)])
  }

  public static func watch(_ link: PullRequestLink) -> Request {
    Request(method: "WatchChangeRequest", payload: ["url": .string(link.url)])
  }

  /// The unified diff. With both revisions it's pinned to the commits on screen.
  public static func diff(
    _ link: PullRequestLink, head: String? = nil, base: String? = nil, refresh: Bool = false
  ) -> Request {
    var payload: [String: JSONValue] = ["url": .string(link.url), "refresh": .bool(refresh)]
    if let head, let base, validHead(head), validHead(base) {
      payload["headRefOid"] = .string(head)
      payload["baseRefOid"] = .string(base)
    }
    return Request(method: "GetChangeRequestDiff", payload: payload)
  }

  // MARK: Writes

  public static func comment(_ link: PullRequestLink, body: String) throws -> Request {
    guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
      throw ClientError("Write a comment first.")
    }
    guard body.utf8.count <= maximumCommentBytes else {
      throw ClientError("Comment is too long (maximum 60,000 bytes).")
    }
    return Request(
      method: "PostChangeRequestComment",
      payload: ["url": .string(link.url), "body": .string(body)])
  }

  /// Merges go through `UpdateChangeRequestAtHead` so GitHub refuses them if the branch moved
  /// after the user looked.
  public static func update(_ link: PullRequestLink, _ update: PullRequestUpdate, head: String)
    throws -> Request
  {
    var payload: [String: JSONValue] = ["url": .string(link.url), "update": update.json]
    if validHead(head) { payload["headRefOid"] = .string(head) }
    if update.pinsHead {
      guard validHead(head) else { throw ClientError("Refresh the pull request before merging.") }
      return Request(method: "UpdateChangeRequestAtHead", payload: payload)
    }
    return Request(method: "UpdateChangeRequest", payload: payload)
  }

  /// Sends one update. Returns GitHub's reason when it declined (`{refusal}`), `nil` when done.
  public static func send(
    _ request: Request,
    call: @Sendable (String, [String: JSONValue]) async throws -> JSONValue
  ) async throws -> String? {
    let reply = try await call(request.method, request.payload)
    guard let refusal = reply["refusal"].string?.trimmingCharacters(in: .whitespacesAndNewlines),
      !refusal.isEmpty
    else { return nil }
    return refusal
  }

  /// Posts one comment and returns the comment GitHub stored.
  public static func post(
    _ request: Request,
    call: @Sendable (String, [String: JSONValue]) async throws -> JSONValue
  ) async throws -> PullRequestComment {
    try await call(request.method, request.payload).decode(PullRequestComment.self)
  }
}
