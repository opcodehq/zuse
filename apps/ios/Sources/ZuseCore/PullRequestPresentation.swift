import Foundation

// Presentation rules shared by the pull request screens. They follow the desktop app and
// `change_request_assessment.rs`: missing metadata never reads as ready or passing.

/// Why an open pull request can't merge as it stands, in the order people should fix them.
public enum PullRequestBlocker: String, Sendable, Hashable {
  case conflicts, failingChecks, changesRequested, pendingChecks, unknownStatus
  public var label: String {
    switch self {
    case .conflicts: return "Merge conflicts"
    case .failingChecks: return "CI failing"
    case .changesRequested: return "Changes requested"
    case .pendingChecks: return "Waiting on checks"
    case .unknownStatus: return "Merge status unknown"
    }
  }
  public func explanation(base: String) -> String {
    let target = base.isEmpty ? "the base branch" : base
    switch self {
    case .conflicts:
      return "This branch has conflicts with \(target). Resolve them, then merge."
    case .failingChecks: return "Some checks are failing. Fix them before merging."
    case .changesRequested: return "A reviewer requested changes. Address them before merging."
    case .pendingChecks: return "Checks are still running. You can merge once they pass."
    case .unknownStatus:
      return "GitHub hasn’t worked out whether this can merge yet. Pull to refresh in a moment."
    }
  }
}

/// What the Merge button can do right now.
public enum PullRequestMergeGate: Sendable, Hashable {
  case ready
  case blocked(PullRequestBlocker)
  case draft
  case merged
  case closed

  public static func of(_ detail: PullRequestDetail) -> PullRequestMergeGate {
    switch detail.state {
    case .merged: return .merged
    case .closed: return .closed
    case .open: break
    }
    if detail.isDraft { return .draft }
    if detail.mergeability == .conflicting { return .blocked(.conflicts) }
    let ci = detail.effectiveCI.state
    if ci == .failed { return .blocked(.failingChecks) }
    if detail.reviewDecision == .changesRequested { return .blocked(.changesRequested) }
    if ci == .pending { return .blocked(.pendingChecks) }
    guard detail.mergeability == .mergeable, ci != .unknown,
      PullRequestCommands.validHead(detail.headRefOid)
    else { return .blocked(.unknownStatus) }
    return .ready
  }
  public var canMerge: Bool { self == .ready }
  /// Arming auto-merge only makes sense while GitHub is waiting on something it can resolve.
  public static func offersAutoMerge(_ detail: PullRequestDetail) -> Bool {
    guard detail.isOpen, !detail.isDraft, detail.autoMergeMethod == nil,
      PullRequestCommands.validHead(detail.headRefOid), detail.mergeability != .conflicting
    else { return false }
    if case .blocked(let blocker) = of(detail) {
      return [.pendingChecks, .failingChecks, .changesRequested].contains(blocker)
    }
    return false
  }
}

/// Board sections, in display order (`PullRequestGroup` on desktop), plus "Ready to merge" for
/// open pull requests with nothing left to do.
public enum PullRequestSection: String, Sendable, Hashable, CaseIterable, Identifiable {
  case attention, ready, review, approved, drafts, merged, closed
  public var id: String { rawValue }
  public var label: String {
    switch self {
    case .attention: return "Needs attention"
    case .ready: return "Ready to merge"
    case .review: return "Awaiting review"
    case .approved: return "Approved"
    case .drafts: return "Drafts"
    case .merged: return "Merged"
    case .closed: return "Closed"
    }
  }

  public static func of(_ item: PullRequestListItem) -> PullRequestSection {
    switch item.state {
    case .merged: return .merged
    case .closed: return .closed
    case .open: break
    }
    if item.isDraft { return .drafts }
    let blocked =
      item.ci.state == .failed || item.reviewDecision == .changesRequested
      || item.mergeability == .conflicting
    if blocked || item.viewerReviewRequested == true { return .attention }
    let checksClear = [.passed, .noChecks, .skipped].contains(item.ci.state)
    if item.mergeability == .mergeable && checksClear && item.reviewDecision != .reviewRequired {
      return .ready
    }
    return item.reviewDecision == .approved ? .approved : .review
  }

  /// Non-empty sections in display order; rows keep the listing's newest-first order.
  public static func group(_ items: [PullRequestListItem]) -> [(
    section: PullRequestSection, items: [PullRequestListItem]
  )] {
    let grouped = Dictionary(grouping: items, by: of)
    return allCases.compactMap { section in
      grouped[section].map { (section, $0) }
    }
  }
}

/// A one-line reason a row sits where it does, shown under "Needs attention".
public enum PullRequestRowStatus {
  public static func attention(_ item: PullRequestListItem) -> String? {
    guard item.state == .open, !item.isDraft else { return nil }
    if item.mergeability == .conflicting { return "Conflicts" }
    if item.ci.state == .failed { return "CI failing" }
    if item.reviewDecision == .changesRequested { return "Changes requested" }
    if item.viewerReviewRequested == true { return "Review requested" }
    return nil
  }
}

/// Check counts and the summary line ("All 8 passed", "1 failing, 2 running, 5 passed").
public struct PullRequestChecks: Sendable, Hashable {
  public var failed: Int
  public var pending: Int
  public var passed: Int
  public var skipped: Int
  public var total: Int { failed + pending + passed + skipped }

  public init(_ checks: [PullRequestCheck]) {
    var counts = (failed: 0, pending: 0, passed: 0, skipped: 0)
    for check in checks {
      switch check.outcome {
      case .failed: counts.failed += 1
      case .pending: counts.pending += 1
      case .passed: counts.passed += 1
      case .skipped: counts.skipped += 1
      }
    }
    (failed, pending, passed, skipped) = counts
  }

  /// `ChangeRequestCi::from_checks`.
  public static func rollup(_ checks: [PullRequestCheck]) -> PullRequestCI {
    let counts = PullRequestChecks(checks)
    let state: PullRequestCIState
    if checks.isEmpty {
      state = .noChecks
    } else if counts.failed > 0 {
      state = .failed
    } else if counts.pending > 0 {
      state = .pending
    } else if counts.passed == 0 {
      state = .skipped
    } else {
      state = .passed
    }
    return PullRequestCI(state: state, totalCount: checks.count)
  }

  public static func summary(_ detail: PullRequestDetail) -> String {
    let counts = PullRequestChecks(detail.checks)
    if counts.total == 0 {
      switch detail.effectiveCI.state {
      case .noChecks: return "No checks"
      case .unknown: return "Unavailable"
      case .passed: return "Passed"
      case .pending: return "Running"
      case .failed: return "Failing"
      case .skipped: return "Skipped"
      }
    }
    if counts.failed == 0 && counts.pending == 0 {
      return counts.passed == 0
        ? "\(counts.skipped) skipped" : "\(counts.passed + counts.skipped) passed"
    }
    var parts: [String] = []
    if counts.failed > 0 { parts.append("\(counts.failed) failing") }
    if counts.pending > 0 { parts.append("\(counts.pending) running") }
    if counts.passed > 0 { parts.append("\(counts.passed) passed") }
    return parts.joined(separator: ", ")
  }

  /// Failing first, then running, then the rest by name.
  public static func ordered(_ checks: [PullRequestCheck]) -> [PullRequestCheck] {
    func rank(_ outcome: PullRequestCheckOutcome) -> Int {
      switch outcome {
      case .failed: return 0
      case .pending: return 1
      case .passed: return 2
      case .skipped: return 3
      }
    }
    return checks.sorted {
      let (left, right) = (rank($0.outcome), rank($1.outcome))
      return left == right
        ? $0.title.localizedStandardCompare($1.title) == .orderedAscending : left < right
    }
  }

  public static func label(_ outcome: PullRequestCheckOutcome, raw: String) -> String {
    switch outcome {
    case .passed: return "Passed"
    case .failed:
      switch raw {
      case "CANCELLED": return "Cancelled"
      case "TIMED_OUT": return "Timed out"
      case "ACTION_REQUIRED": return "Action required"
      default: return "Failed"
      }
    case .skipped: return raw == "NEUTRAL" ? "Neutral" : "Skipped"
    case .pending:
      switch raw {
      case "QUEUED", "WAITING", "REQUESTED": return "Queued"
      case "EXPECTED": return "Expected"
      default: return "Running"
      }
    }
  }
}

/// A reviewer's latest standing on the pull request.
public struct PullRequestReviewer: Sendable, Hashable, Identifiable {
  public enum Verdict: Sendable, Hashable { case approved, changesRequested, commented }
  public let actor: PullRequestActor
  public let verdict: Verdict
  public var id: String { actor.login }

  /// Review authors other than the pull request's own, first-seen order, latest verdict wins.
  /// A later comment-only review doesn't erase an approval or a change request.
  public static func from(_ detail: PullRequestDetail) -> [PullRequestReviewer] {
    var order: [String] = []
    var latest: [String: PullRequestReviewer] = [:]
    for review in detail.reviews
    where !review.author.login.isEmpty && review.author.login != detail.author.login {
      let login = review.author.login
      if latest[login] == nil { order.append(login) }
      let verdict: Verdict
      switch review.state.uppercased() {
      case "APPROVED": verdict = .approved
      case "CHANGES_REQUESTED": verdict = .changesRequested
      default:
        verdict = latest[login]?.verdict ?? .commented
      }
      latest[login] = PullRequestReviewer(actor: review.author, verdict: verdict)
    }
    return order.compactMap { latest[$0] }
  }
}

/// Status chip on the detail card.
public enum PullRequestStatusLabel {
  public static func of(_ detail: PullRequestDetail) -> String {
    switch detail.state {
    case .merged: return "Merged"
    case .closed: return "Closed"
    case .open: break
    }
    if detail.isDraft { return "Draft" }
    if PullRequestMergeGate.of(detail) == .ready { return "Ready to merge" }
    switch detail.reviewDecision {
    case .approved: return "Approved"
    case .changesRequested: return "Changes requested"
    case .reviewRequired, .unknown: return "Ready for review"
    }
  }
}

/// One entry in the Activity timeline.
public enum PullRequestActivity: Sendable, Hashable, Identifiable {
  case comment(PullRequestComment)
  /// A review: its verdict, and its summary body if it has one.
  case review(PullRequestComment)
  /// An inline conversation anchored to a file and line.
  case thread(PullRequestReviewThread)
  case commit(PullRequestCommit)

  public var id: String {
    switch self {
    case .comment(let comment): return "comment:" + comment.id
    case .review(let review): return "review:" + review.id
    case .thread(let thread): return "thread:" + thread.id
    case .commit(let commit): return "commit:" + commit.oid
    }
  }
  public var date: Date? {
    switch self {
    case .comment(let comment), .review(let comment): return comment.date
    case .thread(let thread): return thread.comments.first?.date
    case .commit(let commit): return commit.date
    }
  }

  /// Comments, reviews, inline threads and commits, oldest first. Threads sit at their first
  /// comment; items without a date keep their place at the start.
  public static func timeline(_ detail: PullRequestDetail, commits: Bool = true)
    -> [PullRequestActivity]
  {
    var items: [PullRequestActivity] = detail.comments.map(PullRequestActivity.comment)
    items += detail.reviews.map(PullRequestActivity.review)
    items += (detail.reviewThreads ?? []).filter { !$0.comments.isEmpty }.map(
      PullRequestActivity.thread)
    if commits { items += (detail.commits ?? []).map(PullRequestActivity.commit) }
    return items.enumerated().sorted { left, right in
      switch (left.element.date, right.element.date) {
      case (let l?, let r?) where l != r: return l < r
      case (nil, .some): return true
      case (.some, nil): return false
      default: return left.offset < right.offset
      }
    }.map(\.element)
  }

  /// What a review did, as a sentence fragment after the reviewer's name.
  public static func reviewVerb(_ state: String) -> String {
    switch state.uppercased() {
    case "APPROVED": return "approved"
    case "CHANGES_REQUESTED": return "requested changes"
    case "DISMISSED": return "had a review dismissed"
    default: return "reviewed"
    }
  }
}

/// The few lines of diff a review thread points at, with their numbers.
public enum PullRequestHunkSnippet {
  /// The last `limit` lines of a comment's `diffHunk`, ending at the commented line.
  public static func lines(_ hunk: String, limit: Int = 4) -> [DiffLine] {
    let lines = DiffLine.parse(hunk).filter { $0.kind != .header }
    return Array(lines.suffix(limit))
  }
}

/// Compact ages as the list shows them (`now`, `5m`, `3h`, `2d`, `8w`, `9mo`, `1y`).
public enum PullRequestAge {
  public static func short(_ date: Date?, now: Date = Date()) -> String {
    guard let date else { return "" }
    let seconds = max(0, now.timeIntervalSince(date))
    switch seconds {
    case ..<60: return "now"
    case ..<3600: return "\(Int(seconds / 60))m"
    case ..<86_400: return "\(Int(seconds / 3600))h"
    case ..<604_800: return "\(Int(seconds / 86_400))d"
    case ..<2_592_000: return "\(Int(seconds / 604_800))w"
    case ..<31_536_000: return "\(Int(seconds / 2_592_000))mo"
    default: return "\(Int(seconds / 31_536_000))y"
    }
  }
  /// "26m ago", "just now".
  public static func ago(_ date: Date?, now: Date = Date()) -> String {
    let value = short(date, now: now)
    if value.isEmpty { return "" }
    return value == "now" ? "just now" : value + " ago"
  }
}

/// Readable sentences for the engine's capability errors (`pull_requests.*`).
public enum PullRequestErrorText {
  public static func readable(_ error: Error) -> String {
    let message = (error as? ClientError)?.message ?? error.localizedDescription
    let codes: [(String, String)] = [
      ("pull_requests.cli_unavailable", "Install GitHub CLI (gh) on your computer and sign in."),
      ("pull_requests.cli_outdated", "Update GitHub CLI (gh) on your computer, then try again."),
      ("pull_requests.authentication", "Run gh auth login on your computer, then refresh."),
      ("pull_requests.rate_limited", "GitHub’s rate limit was reached. Try again in 15 minutes."),
      ("pull_requests.timeout", "GitHub didn’t respond in time. Try again."),
      (
        "pull_requests.decode",
        "GitHub’s response was too large or unreadable. Open it on GitHub instead."
      ),
      ("pull_requests.command_failed", "GitHub couldn’t complete the request. Try again."),
    ]
    if let match = codes.first(where: { message.contains($0.0) }) { return match.1 }
    if message.lowercased().contains("unknown method") {
      return "Update Zuse on your computer to see pull requests here."
    }
    return message
  }
}
