import Foundation

// Wire models for the engine's pull request methods. They mirror the serde contracts in
// `crates/protocol/src/entities.rs` (`ChangeRequestListItem`, `ChangeRequestPage`,
// `ChangeRequestDetail` and friends): camelCase keys, every field optional on the way in,
// because older engines omit newer fields and GitHub reports some values as empty strings.

/// Lifecycle state. Listing items send `open`; detail sends GitHub's `OPEN`.
public enum PullRequestState: String, Sendable, Hashable {
  case open, closed, merged
  init(wire: String?) {
    self = PullRequestState(rawValue: (wire ?? "").lowercased()) ?? .open
  }
}

/// Whether the branch merges cleanly. Detail sends `MERGEABLE`, listing sends `mergeable`.
public enum PullRequestMergeability: String, Sendable, Hashable {
  case mergeable, conflicting, unknown
  init(wire: String?) {
    self = PullRequestMergeability(rawValue: (wire ?? "").lowercased()) ?? .unknown
  }
}

/// The aggregate review decision. Detail sends `CHANGES_REQUESTED`, listing `changesRequested`.
public enum PullRequestReviewDecision: String, Sendable, Hashable {
  case approved, changesRequested, reviewRequired, unknown
  init(wire: String?) {
    switch (wire ?? "").replacingOccurrences(of: "_", with: "").lowercased() {
    case "approved": self = .approved
    case "changesrequested": self = .changesRequested
    case "reviewrequired": self = .reviewRequired
    default: self = .unknown
    }
  }
}

/// `CiState` in `change_request_assessment.rs`; unknown values read as `unknown`.
public enum PullRequestCIState: String, Sendable, Hashable {
  case noChecks, pending, passed, skipped, failed, unknown
}

public struct PullRequestCI: Sendable, Hashable, Decodable {
  public var state: PullRequestCIState
  public var totalCount: Int
  public init(state: PullRequestCIState = .unknown, totalCount: Int = 0) {
    self.state = state
    self.totalCount = totalCount
  }
  enum CodingKeys: String, CodingKey { case state, totalCount }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    state = PullRequestCIState(rawValue: c.lenient(String.self, .state) ?? "") ?? .unknown
    totalCount = c.lenient(Int.self, .totalCount) ?? 0
  }
}

public struct PullRequestActor: Sendable, Hashable, Decodable {
  public var login: String
  public var avatarURL: URL?
  public init(login: String, avatarURL: URL? = nil) {
    self.login = login
    self.avatarURL = avatarURL
  }
  enum CodingKeys: String, CodingKey { case login, avatarUrl }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    login = c.lenient(String.self, .login) ?? ""
    avatarURL = c.lenient(String.self, .avatarUrl).flatMap(PullRequestActor.safeURL)
  }
  /// Apps post as `name[bot]` (REST) or `app/name` (GraphQL).
  public var isBot: Bool { login.hasSuffix("[bot]") || login.hasPrefix("app/") }
  /// The login as people read it: no `app/` prefix.
  public var displayName: String {
    if login.isEmpty { return "Unknown author" }
    return login.hasPrefix("app/") ? String(login.dropFirst(4)) + "[bot]" : login
  }
  static func safeURL(_ raw: String) -> URL? {
    guard let url = URL(string: raw), url.scheme == "https", url.host != nil, url.user == nil
    else { return nil }
    return url
  }
}

/// One row of `ListChangeRequestPage`.
public struct PullRequestListItem: Sendable, Hashable, Identifiable, Decodable {
  public var provider: String
  public var author: PullRequestActor
  public var repository: String
  public var headRefOid: String
  public var ci: PullRequestCI
  public var viewerDidAuthor: Bool?
  public var viewerReviewRequested: Bool?
  public var number: Int
  public var title: String
  public var url: String
  public var state: PullRequestState
  public var isDraft: Bool
  public var reviewDecision: PullRequestReviewDecision
  public var additions: Int
  public var deletions: Int
  public var mergeability: PullRequestMergeability
  public var createdAt: Date?
  public var updatedAt: Date?
  public var id: String { url }

  enum CodingKeys: String, CodingKey {
    case provider, author, repository, headRefOid, ci, viewerDidAuthor, viewerReviewRequested
    case number, title, url, state, isDraft, reviewDecision, additions, deletions, mergeability
    case createdAt, updatedAt
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    provider = c.lenient(String.self, .provider) ?? "github"
    author = c.lenient(PullRequestActor.self, .author) ?? PullRequestActor(login: "")
    repository = c.lenient(String.self, .repository) ?? ""
    headRefOid = c.lenient(String.self, .headRefOid) ?? ""
    ci = c.lenient(PullRequestCI.self, .ci) ?? PullRequestCI()
    viewerDidAuthor = c.lenient(Bool.self, .viewerDidAuthor)
    viewerReviewRequested = c.lenient(Bool.self, .viewerReviewRequested)
    number = c.lenient(Int.self, .number) ?? 0
    title = c.lenient(String.self, .title) ?? ""
    url = c.lenient(String.self, .url) ?? ""
    state = PullRequestState(wire: c.lenient(String.self, .state))
    isDraft = c.lenient(Bool.self, .isDraft) ?? false
    reviewDecision = PullRequestReviewDecision(wire: c.lenient(String.self, .reviewDecision))
    additions = c.lenient(Int.self, .additions) ?? 0
    deletions = c.lenient(Int.self, .deletions) ?? 0
    mergeability = PullRequestMergeability(wire: c.lenient(String.self, .mergeability))
    createdAt = c.lenient(String.self, .createdAt).flatMap(PullRequestDate.parse)
    updatedAt = c.lenient(String.self, .updatedAt).flatMap(PullRequestDate.parse)
  }
}

/// `ChangeRequestPage`: one page of a repository's listing, newest updates first.
public struct PullRequestPage: Sendable, Hashable, Decodable {
  public var items: [PullRequestListItem]
  public var nextCursor: String?
  public var totalCount: Int?
  public var fetchedAt: Date?
  public init(
    items: [PullRequestListItem], nextCursor: String? = nil, totalCount: Int? = nil,
    fetchedAt: Date? = nil
  ) {
    self.items = items
    self.nextCursor = nextCursor
    self.totalCount = totalCount
    self.fetchedAt = fetchedAt
  }
  enum CodingKeys: String, CodingKey { case items, nextCursor, totalCount, fetchedAt }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    items = c.lenient([PullRequestListItem].self, .items) ?? []
    nextCursor = c.lenient(String.self, .nextCursor)
    totalCount = c.lenient(Int.self, .totalCount)
    fetchedAt = c.lenient(String.self, .fetchedAt).flatMap(PullRequestDate.parse)
  }
}

/// `ChangeRequestFilter`. `all` means every open pull request.
public enum PullRequestFilter: String, Sendable, Hashable, CaseIterable, Identifiable {
  case all, authored, reviewing, merged, allStates
  public var id: String { rawValue }
  public var label: String {
    switch self {
    case .all: return "Open"
    case .authored: return "Created by you"
    case .reviewing: return "Review requested"
    case .merged: return "Merged"
    case .allStates: return "All"
    }
  }
}

public struct PullRequestComment: Sendable, Hashable, Identifiable, Decodable {
  public var id: String
  public var url: String
  public var replyTo: String?
  public var viewerDidAuthor: Bool
  public var author: PullRequestActor
  public var body: String
  /// Review state (`APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, …); empty for comments.
  public var state: String
  public var createdAt: String
  public var submittedAt: String
  /// The diff an inline review comment is anchored to, ending at its line.
  public var diffHunk: String
  public init(
    id: String, url: String = "", replyTo: String? = nil, viewerDidAuthor: Bool = false,
    author: PullRequestActor, body: String, state: String = "", createdAt: String = "",
    submittedAt: String = "", diffHunk: String = ""
  ) {
    self.id = id
    self.url = url
    self.replyTo = replyTo
    self.viewerDidAuthor = viewerDidAuthor
    self.author = author
    self.body = body
    self.state = state
    self.createdAt = createdAt
    self.submittedAt = submittedAt
    self.diffHunk = diffHunk
  }
  enum CodingKeys: String, CodingKey {
    case id, url, replyTo, viewerDidAuthor, author, body, state, createdAt, submittedAt, diffHunk
  }
  enum ReferenceKeys: String, CodingKey { case id }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    id = c.lenient(String.self, .id) ?? ""
    url = c.lenient(String.self, .url) ?? ""
    replyTo = (try? c.nestedContainer(keyedBy: ReferenceKeys.self, forKey: .replyTo))
      .flatMap { $0.lenient(String.self, .id) }
    viewerDidAuthor = c.lenient(Bool.self, .viewerDidAuthor) ?? false
    author = c.lenient(PullRequestActor.self, .author) ?? PullRequestActor(login: "")
    body = c.lenient(String.self, .body) ?? ""
    state = c.lenient(String.self, .state) ?? ""
    createdAt = c.lenient(String.self, .createdAt) ?? ""
    submittedAt = c.lenient(String.self, .submittedAt) ?? ""
    diffHunk = c.lenient(String.self, .diffHunk) ?? ""
  }
  /// When it was written: `createdAt`, else a review's `submittedAt`.
  public var date: Date? { PullRequestDate.parse(createdAt) ?? PullRequestDate.parse(submittedAt) }
}

public struct PullRequestReviewThread: Sendable, Hashable, Identifiable, Decodable {
  public var id: String
  public var isResolved: Bool
  public var isOutdated: Bool
  public var path: String
  public var line: Int?
  public var startLine: Int?
  public var originalLine: Int?
  public var originalStartLine: Int?
  public var diffSide: String
  public var comments: [PullRequestComment]
  enum CodingKeys: String, CodingKey {
    case id, isResolved, isOutdated, path, line, startLine, originalLine, originalStartLine
    case diffSide, comments
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    id = c.lenient(String.self, .id) ?? ""
    isResolved = c.lenient(Bool.self, .isResolved) ?? false
    isOutdated = c.lenient(Bool.self, .isOutdated) ?? false
    path = c.lenient(String.self, .path) ?? ""
    line = c.lenient(Int.self, .line)
    startLine = c.lenient(Int.self, .startLine)
    originalLine = c.lenient(Int.self, .originalLine)
    originalStartLine = c.lenient(Int.self, .originalStartLine)
    diffSide = c.lenient(String.self, .diffSide) ?? ""
    comments = c.lenient([PullRequestComment].self, .comments) ?? []
  }
  /// The line people see: the current one, else where it was when written.
  public var displayLine: Int? { line ?? originalLine }
  public var displayStartLine: Int? { startLine ?? originalStartLine }
}

public struct PullRequestFile: Sendable, Hashable, Decodable {
  public var path: String
  public var additions: Int
  public var deletions: Int
  enum CodingKeys: String, CodingKey { case path, additions, deletions }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    path = c.lenient(String.self, .path) ?? ""
    additions = c.lenient(Int.self, .additions) ?? 0
    deletions = c.lenient(Int.self, .deletions) ?? 0
  }
}

/// One entry of `statusCheckRollup`: a check run (`name`, `status`, `conclusion`) or a commit
/// status (`context`, `state`).
public struct PullRequestCheck: Sendable, Hashable, Identifiable, Decodable {
  public var databaseId: Int?
  public var startedAt: String
  public var completedAt: String
  public var name: String
  public var context: String
  public var status: String
  public var conclusion: String
  public var state: String
  public var detailsUrl: String
  public var targetUrl: String
  public var id: String { databaseId.map(String.init) ?? (title + startedAt) }
  enum CodingKeys: String, CodingKey {
    case databaseId, startedAt, completedAt, name, context, status, conclusion, state
    case detailsUrl, targetUrl
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    databaseId = c.lenient(Int.self, .databaseId)
    startedAt = c.lenient(String.self, .startedAt) ?? ""
    completedAt = c.lenient(String.self, .completedAt) ?? ""
    name = c.lenient(String.self, .name) ?? ""
    context = c.lenient(String.self, .context) ?? ""
    status = c.lenient(String.self, .status) ?? ""
    conclusion = c.lenient(String.self, .conclusion) ?? ""
    state = c.lenient(String.self, .state) ?? ""
    detailsUrl = c.lenient(String.self, .detailsUrl) ?? ""
    targetUrl = c.lenient(String.self, .targetUrl) ?? ""
  }
  public var title: String { name.isEmpty ? (context.isEmpty ? "Check" : context) : name }
  /// `check_status`: conclusion, then state, then status, upper-cased.
  public var rawStatus: String {
    [conclusion, state, status].first { !$0.isEmpty }?.uppercased() ?? ""
  }
  public var outcome: PullRequestCheckOutcome { PullRequestCheckOutcome(rawStatus) }
  /// `detailsUrl`, else `targetUrl`; HTTPS only, never with credentials.
  public var link: URL? {
    [detailsUrl, targetUrl].lazy.compactMap(PullRequestActor.safeURL).first
  }
}

public enum PullRequestCheckOutcome: Sendable, Hashable {
  case passed, failed, pending, skipped
  init(_ status: String) {
    switch status {
    case "FAILURE", "ERROR", "TIMED_OUT", "ACTION_REQUIRED", "CANCELLED", "STARTUP_FAILURE":
      self = .failed
    case "SUCCESS": self = .passed
    case "NEUTRAL", "SKIPPED": self = .skipped
    default: self = .pending
    }
  }
}

public struct PullRequestCommit: Sendable, Hashable, Identifiable, Decodable {
  public var oid: String
  public var messageHeadline: String
  public var authoredDate: String
  public var committedDate: String
  public var authors: [Author]
  public var additions: Int?
  public var deletions: Int?
  public var id: String { oid }
  public struct Author: Sendable, Hashable, Decodable {
    public var login: String?
    public var name: String?
  }
  enum CodingKeys: String, CodingKey {
    case oid, messageHeadline, authoredDate, committedDate, authors, additions, deletions
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    oid = c.lenient(String.self, .oid) ?? ""
    messageHeadline = c.lenient(String.self, .messageHeadline) ?? ""
    authoredDate = c.lenient(String.self, .authoredDate) ?? ""
    committedDate = c.lenient(String.self, .committedDate) ?? ""
    authors = c.lenient([Author].self, .authors) ?? []
    additions = c.lenient(Int.self, .additions)
    deletions = c.lenient(Int.self, .deletions)
  }
  public var shortOid: String { String(oid.prefix(7)) }
  public var date: Date? {
    PullRequestDate.parse(committedDate) ?? PullRequestDate.parse(authoredDate)
  }
  public var authorName: String {
    authors.first.flatMap { $0.login ?? $0.name } ?? "Someone"
  }
}

/// `ChangeRequestDetail`, from `GetChangeRequest` or a `WatchChangeRequest` snapshot.
public struct PullRequestDetail: Sendable, Hashable, Decodable {
  public var title: String
  public var body: String
  public var url: String
  public var number: Int
  public var author: PullRequestActor
  public var baseRefName: String
  public var baseRefOid: String
  public var headRefName: String
  public var headRefOid: String
  public var headRepository: String?
  public var ci: PullRequestCI
  public var viewerDidAuthor: Bool?
  public var viewerReviewRequested: Bool?
  public var state: PullRequestState
  public var isDraft: Bool
  public var reviewDecision: PullRequestReviewDecision
  public var mergeability: PullRequestMergeability
  public var additions: Int
  public var deletions: Int
  public var comments: [PullRequestComment]
  public var reviews: [PullRequestComment]
  /// `nil` means unavailable (a summary read or an older engine), not "no feedback".
  public var reviewThreads: [PullRequestReviewThread]?
  public var files: [PullRequestFile]
  public var checks: [PullRequestCheck]
  public var commits: [PullRequestCommit]?
  /// The armed auto-merge method (`SQUASH`, `MERGE`, `REBASE`), if any.
  public var autoMergeMethod: String?
  public var viewerLogin: String?

  enum CodingKeys: String, CodingKey {
    case title, body, url, number, author, baseRefName, baseRefOid, headRefName, headRefOid
    case headRepository, ci, viewerDidAuthor, viewerReviewRequested, state, isDraft
    case reviewDecision, mergeable, additions, deletions, comments, reviews, reviewThreads, files
    case statusCheckRollup, commits, autoMergeRequest, viewerLogin
  }
  enum RepositoryKeys: String, CodingKey { case nameWithOwner }
  enum AutoMergeKeys: String, CodingKey { case mergeMethod }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    title = c.lenient(String.self, .title) ?? ""
    body = c.lenient(String.self, .body) ?? ""
    url = c.lenient(String.self, .url) ?? ""
    number = c.lenient(Int.self, .number) ?? 0
    author = c.lenient(PullRequestActor.self, .author) ?? PullRequestActor(login: "")
    baseRefName = c.lenient(String.self, .baseRefName) ?? ""
    baseRefOid = c.lenient(String.self, .baseRefOid) ?? ""
    headRefName = c.lenient(String.self, .headRefName) ?? ""
    headRefOid = c.lenient(String.self, .headRefOid) ?? ""
    headRepository = (try? c.nestedContainer(keyedBy: RepositoryKeys.self, forKey: .headRepository))
      .flatMap { $0.lenient(String.self, .nameWithOwner) }.flatMap { $0.isEmpty ? nil : $0 }
    ci = c.lenient(PullRequestCI.self, .ci) ?? PullRequestCI()
    viewerDidAuthor = c.lenient(Bool.self, .viewerDidAuthor)
    viewerReviewRequested = c.lenient(Bool.self, .viewerReviewRequested)
    state = PullRequestState(wire: c.lenient(String.self, .state))
    isDraft = c.lenient(Bool.self, .isDraft) ?? false
    reviewDecision = PullRequestReviewDecision(wire: c.lenient(String.self, .reviewDecision))
    mergeability = PullRequestMergeability(wire: c.lenient(String.self, .mergeable))
    additions = c.lenient(Int.self, .additions) ?? 0
    deletions = c.lenient(Int.self, .deletions) ?? 0
    comments = c.lenient([PullRequestComment].self, .comments) ?? []
    reviews = c.lenient([PullRequestComment].self, .reviews) ?? []
    reviewThreads = c.lenient([PullRequestReviewThread].self, .reviewThreads)
    files = c.lenient([PullRequestFile].self, .files) ?? []
    checks = c.lenient([PullRequestCheck].self, .statusCheckRollup) ?? []
    commits = c.lenient([PullRequestCommit].self, .commits)
    autoMergeMethod =
      (try? c.nestedContainer(keyedBy: AutoMergeKeys.self, forKey: .autoMergeRequest))
      .map { $0.lenient(String.self, .mergeMethod) ?? "" }
    viewerLogin = c.lenient(String.self, .viewerLogin)
  }

  /// `owner/repo`, from the canonical URL.
  public var repository: String { PullRequestLink(url)?.repository ?? "" }
  public var isOpen: Bool { state == .open }
  /// CI as the engine reported it, derived from the checks when it could not say.
  public var effectiveCI: PullRequestCI {
    if ci.state == .unknown && !checks.isEmpty { return PullRequestChecks.rollup(checks) }
    return ci
  }
}

/// `CheckoutChangeRequestStatus.changeRequest` from `WatchCheckoutChangeRequest`: the pull
/// request for a chat's checkout and branch, if there is one.
public struct PullRequestCheckoutSummary: Sendable, Hashable, Decodable {
  public var number: Int
  public var title: String
  public var url: String
  public var state: PullRequestState
  public var isDraft: Bool
  public var baseRef: String
  public var headRef: String
  enum CodingKeys: String, CodingKey { case number, title, url, state, isDraft, baseRef, headRef }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    number = c.lenient(Int.self, .number) ?? 0
    title = c.lenient(String.self, .title) ?? ""
    url = c.lenient(String.self, .url) ?? ""
    state = PullRequestState(wire: c.lenient(String.self, .state))
    isDraft = c.lenient(Bool.self, .isDraft) ?? false
    baseRef = c.lenient(String.self, .baseRef) ?? ""
    headRef = c.lenient(String.self, .headRef) ?? ""
  }
  /// The summary inside one status frame; `nil` for "no pull request" and failed lookups.
  public static func from(status: JSONValue) -> PullRequestCheckoutSummary? {
    guard status["changeRequest"] != .null,
      let summary = try? status["changeRequest"].decode(PullRequestCheckoutSummary.self),
      PullRequestLink(summary.url) != nil
    else { return nil }
    return summary
  }
}

/// One `SourceControlFrame` from `WatchChangeRequest`.
public struct PullRequestFrame: Sendable {
  public var reset: Bool
  public var refreshing: Bool
  public var error: String?
  public var snapshot: PullRequestDetail?
  public init(_ value: JSONValue) {
    reset = value["reset"].bool
    refreshing = value["refreshing"].bool
    error = value["error"].string
    snapshot =
      value["snapshot"] == .null ? nil : try? value["snapshot"].decode(PullRequestDetail.self)
  }
}

/// A canonical `https://github.com/<owner>/<repo>/pull/<n>` link, the only form the engine
/// accepts. Links to a pull request's tabs (`/files`, `/commits`, anchors) resolve to it too.
public struct PullRequestLink: Sendable, Hashable, Identifiable {
  public let owner: String
  public let name: String
  public let number: Int
  public var repository: String { owner + "/" + name }
  public var url: String { "https://github.com/\(owner)/\(name)/pull/\(number)" }
  public var id: String { url }
  /// `name#123`, as titles show it.
  public var shortLabel: String { "\(name)#\(number)" }

  public init?(_ raw: String) {
    guard let url = URL(string: raw.trimmingCharacters(in: .whitespacesAndNewlines)) else {
      return nil
    }
    self.init(url: url)
  }
  public init?(url: URL) {
    guard url.scheme?.lowercased() == "https", url.host?.lowercased() == "github.com",
      url.user == nil, url.password == nil, url.port == nil
    else { return nil }
    let parts = url.path.split(separator: "/").map(String.init)
    guard parts.count >= 4, parts[2] == "pull", let number = Int(parts[3]), number > 0,
      parts[0...1].allSatisfy(PullRequestLink.validSegment)
    else { return nil }
    owner = parts[0]
    name = parts[1]
    self.number = number
  }
  public init(owner: String, name: String, number: Int) {
    self.owner = owner
    self.name = name
    self.number = number
  }
  static func validSegment(_ part: String) -> Bool {
    !part.isEmpty && part != "." && part != ".."
      && part.unicodeScalars.allSatisfy {
        ($0.isASCII && CharacterSet.alphanumerics.contains($0)) || "-_.".unicodeScalars.contains($0)
      }
  }
  /// `owner/repo`, validated like the engine's `valid_pr_repository`.
  public static func validRepository(_ value: String) -> Bool {
    let parts = value.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
    return parts.count == 2 && parts.allSatisfy(validSegment)
  }
}

/// RFC 3339 timestamps from chrono and GitHub, with or without fractional seconds.
public enum PullRequestDate {
  nonisolated(unsafe) private static let plain: ISO8601DateFormatter = {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime]
    return formatter
  }()
  public static func parse(_ raw: String) -> Date? {
    guard !raw.isEmpty else { return nil }
    // Drop fractional seconds of any precision; the list needs minutes, not nanoseconds.
    var text = raw
    if let dot = text.firstIndex(of: "."),
      let end = text[dot...].firstIndex(where: { $0 == "Z" || $0 == "+" || $0 == "-" })
    {
      text.removeSubrange(dot..<end)
    }
    return plain.date(from: text)
  }
}

extension KeyedDecodingContainer {
  /// A missing, null or mistyped field reads as absent instead of failing the whole payload.
  func lenient<T: Decodable>(_ type: T.Type, _ key: Key) -> T? {
    (try? decodeIfPresent(type, forKey: key)) ?? nil
  }
}
