import Foundation

/// Which issues the Issues list starts from, before any filter.
public enum IssueScope: String, CaseIterable, Sendable {
  /// Waiting on you, or yours to do.
  case forYou
  /// Owned by you.
  case assignedToMe
  case all
  public var label: String {
    switch self {
    case .forYou: return "For you"
    case .assignedToMe: return "Assigned to me"
    case .all: return "All issues"
    }
  }
  public func includes(_ issue: ZapperIssue) -> Bool {
    switch self {
    case .forYou:
      return issue.details.status == .needsYou || issue.details.currentOwner == .user
    case .assignedToMe: return issue.details.currentOwner == .user
    case .all: return true
    }
  }
}

/// Narrowing on top of a scope. An empty set means any value.
public struct IssueFilter: Hashable, Sendable {
  public var statuses: Set<ZapperIssueStatus>
  public var priorities: Set<ZapperIssuePriority>
  public var zappers: Set<String>
  public init(
    statuses: Set<ZapperIssueStatus> = [], priorities: Set<ZapperIssuePriority> = [],
    zappers: Set<String> = []
  ) {
    self.statuses = statuses
    self.priorities = priorities
    self.zappers = zappers
  }
  /// How many kinds of filter are on.
  public var activeCount: Int {
    [!statuses.isEmpty, !priorities.isEmpty, !zappers.isEmpty].filter { $0 }.count
  }
  public var isEmpty: Bool { activeCount == 0 }
  public func includes(_ entry: IssueEntry) -> Bool {
    (statuses.isEmpty || statuses.contains(entry.issue.details.status))
      && (priorities.isEmpty || priorities.contains(entry.issue.details.priority))
      && (zappers.isEmpty || zappers.contains(entry.zapperID))
  }
}

/// One issue and the Zapper it belongs to.
public struct IssueEntry: Hashable, Sendable, Identifiable {
  public let zapperID: String
  public let issue: ZapperIssue
  public init(zapperID: String, issue: ZapperIssue) {
    self.zapperID = zapperID
    self.issue = issue
  }
  public var id: String { zapperID + "/" + issue.id }
}

/// Issues sharing a status, in board order.
public struct IssueSection: Hashable, Sendable, Identifiable {
  public let status: ZapperIssueStatus
  public let entries: [IssueEntry]
  public init(status: ZapperIssueStatus, entries: [IssueEntry]) {
    self.status = status
    self.entries = entries
  }
  public var id: String { status.rawValue }
  /// Finished work starts folded away.
  public var collapsedByDefault: Bool { status.terminal }
}

public enum IssueBoard {
  /// Every issue across `zappers`, keeping each Zapper's own order.
  public static func entries(_ zappers: [Zapper]) -> [IssueEntry] {
    zappers.flatMap { zapper in
      zapper.issues.map { IssueEntry(zapperID: zapper.id, issue: $0) }
    }
  }

  /// Issues in `scope` that pass `filter`, grouped in board order (what needs you first, then
  /// live work, then queued, then finished). Within a status: higher priority first, then the
  /// most recently updated. Empty statuses are left out.
  public static func sections(
    _ entries: [IssueEntry], scope: IssueScope = .all, filter: IssueFilter = IssueFilter()
  ) -> [IssueSection] {
    let visible = entries.filter { scope.includes($0.issue) && filter.includes($0) }
    return ZapperIssueStatus.board.compactMap { status in
      let group = visible.filter { $0.issue.details.status == status }.sorted { lhs, rhs in
        let left = rank(lhs.issue.details.priority)
        let right = rank(rhs.issue.details.priority)
        if left != right { return left < right }
        if lhs.issue.updatedAt != rhs.issue.updatedAt {
          return lhs.issue.updatedAt > rhs.issue.updatedAt
        }
        return lhs.issue.number > rhs.issue.number
      }
      return group.isEmpty ? nil : IssueSection(status: status, entries: group)
    }
  }

  /// How many issues each scope would show, for the scope picker.
  public static func counts(_ entries: [IssueEntry]) -> [IssueScope: Int] {
    Dictionary(
      uniqueKeysWithValues: IssueScope.allCases.map { scope in
        (
          scope,
          entries.filter { scope.includes($0.issue) && !$0.issue.details.status.terminal }.count
        )
      })
  }

  /// The statuses someone can move an issue to by hand, plus its current one when that is a
  /// state only the Zapper sets (waiting on you, blocked).
  public static func choosableStatuses(current: ZapperIssueStatus) -> [ZapperIssueStatus] {
    let manual: [ZapperIssueStatus] = [.backlog, .todo, .inProgress, .inReview, .done, .canceled]
    return manual.contains(current) ? manual : [current] + manual
  }

  /// Details after moving an issue to `status`. Moving on from a question hands the issue
  /// back to the Zapper.
  public static func details(
    _ details: ZapperIssueDetails, movingTo status: ZapperIssueStatus
  ) -> ZapperIssueDetails {
    var next = details
    next.status = status
    if details.status == .needsYou, status != .needsYou, details.currentOwner == .user {
      next.owner = .zapper
    }
    return next
  }

  /// Details after handing an issue to `owner`; `nil` clears it.
  public static func details(
    _ details: ZapperIssueDetails, owner: ZapperIssueOwner?
  ) -> ZapperIssueDetails {
    var next = details
    next.owner = owner
    // `assigned` predates owners and meant the Zapper; keep it in step for older readers.
    next.assigned = owner == .zapper
    return next
  }

  private static func rank(_ priority: ZapperIssuePriority) -> Int {
    ZapperIssuePriority.allCases.firstIndex(of: priority) ?? 0
  }
}

/// Short times for lists, as in a message list: now, 9m, 3h, 2d, 1w, 4mo, 1y.
public enum CompactTime {
  public static func label(_ date: Date, now: Date = Date()) -> String {
    let seconds = max(0, now.timeIntervalSince(date))
    switch seconds {
    case ..<60: return "now"
    case ..<3600: return "\(Int(seconds / 60))m"
    case ..<86_400: return "\(Int(seconds / 3600))h"
    case ..<604_800: return "\(Int(seconds / 86_400))d"
    case ..<(86_400 * 60): return "\(Int(seconds / 604_800))w"
    case ..<(86_400 * 365): return "\(Int(seconds / (86_400 * 30)))mo"
    default: return "\(Int(seconds / (86_400 * 365)))y"
    }
  }
  /// The same label from Unix milliseconds.
  public static func label(milliseconds: Int64, now: Date = Date()) -> String {
    label(Date(timeIntervalSince1970: TimeInterval(milliseconds) / 1000), now: now)
  }
}
