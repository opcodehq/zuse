import Foundation

/// Where a Zapper thread sits in its lists: working, then waiting on the user, then queued,
/// then idle. Mirrors `ThreadRank` in `crates/protocol/src/presentation/zapper_panel.rs`.
public enum ZapperThreadRank: Int, Sendable, Comparable, CaseIterable {
  case working, needsYou, queued, idle
  public static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
  public var label: String {
    switch self {
    case .working: return "Working"
    case .needsYou: return "Needs you"
    case .queued: return "Queued"
    case .idle: return "Idle"
    }
  }
}

/// A chat's live run state from `WatchSessions`.
public enum ZapperRunStatus: String, Sendable {
  case idle, working, awaitingInput, errored
  /// Rows older than this are left behind by a crashed host and mean nothing.
  public static let staleAfter: TimeInterval = 45
}

/// Something that really waits on the user. Reports are the Zapper's inbox, not this.
public enum ZapperNeedsYouItem: Sendable, Hashable, Identifiable {
  case decision(ZapperDecision)
  case issue(id: String, code: String, title: String, question: String?)
  public var id: String {
    switch self {
    case .decision(let decision): return "decision:" + decision.id
    case .issue(let id, _, _, _): return "issue:" + id
    }
  }
  public var title: String {
    switch self {
    case .decision(let decision): return decision.question
    case .issue(_, let code, let title, let question): return code + " · " + (question ?? title)
    }
  }
}

extension Zapper {
  /// Chats this Zapper owns now: its coordinator, earlier coordinators and its workers.
  public var memberChatIDs: Set<String> {
    Set([coordinatorChatId] + previousCoordinatorChats + workers.map(\.chatId))
  }
  /// Open decisions plus issues handed to the user: the sidebar badge on desktop.
  public var needsYouCount: Int {
    decisions.filter { $0.answer == nil }.count
      + issues.filter { $0.details.status == .needsYou }.count
  }
  /// Issues owned by the user (with their latest question) first, then unanswered decisions.
  public var needsYouItems: [ZapperNeedsYouItem] {
    let asked = issues.filter {
      $0.details.status == .needsYou && $0.details.currentOwner == .user
    }.map { issue -> ZapperNeedsYouItem in
      let question = issue.activity.last {
        $0.kind == "question" && !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      }
      return .issue(
        id: issue.id, code: issue.code, title: issue.details.title,
        question: question?.text.trimmingCharacters(in: .whitespacesAndNewlines))
    }
    return asked + decisions.filter { $0.answer == nil }.map(ZapperNeedsYouItem.decision)
  }
  /// Threads by rank, newest first within a rank. The engine appends new workers, so newest
  /// first is their reverse; a queued thread ranks as queued whatever its chat says.
  public func orderedThreads(rank: (ZapperWorker) -> ZapperThreadRank) -> [ZapperWorker] {
    let newestFirst = Array(workers.reversed())
    return newestFirst.enumerated().sorted { lhs, rhs in
      let left = lhs.element.queued ? .queued : rank(lhs.element)
      let right = rhs.element.queued ? .queued : rank(rhs.element)
      return left == right ? lhs.offset < rhs.offset : left < right
    }.map(\.element)
  }
  /// Threads this Zapper started that left `workers` but are still linked to an issue:
  /// newest issue first, latest thread first within an issue.
  public var finishedThreads: [String] {
    var finished: [String] = []
    for issue in issues.reversed() {
      for id in issue.threads.reversed()
      where ZapperMembership.isThreadID(id) && !workers.contains(where: { $0.chatId == id })
        && !finished.contains(id)
      {
        finished.append(id)
      }
    }
    return finished
  }
  /// The issue a thread is working on: one it owns or is linked to, open issues before closed
  /// ones, then the most recently updated.
  public func issue(forThread chatId: String) -> ZapperIssue? {
    issues.filter { issue in
      if case .thread(let owner) = issue.details.owner, owner == chatId { return true }
      return issue.threads.contains(chatId)
    }.max { lhs, rhs in
      lhs.details.status.terminal == rhs.details.status.terminal
        ? lhs.updatedAt < rhs.updatedAt : lhs.details.status.terminal
    }
  }
  /// The queued worker that owns `issue`, if any; that issue reads as Queued.
  public func queuedWorker(for issue: ZapperIssue) -> ZapperWorker? {
    guard case .thread(let chatId) = issue.details.owner else { return nil }
    return workers.first { $0.chatId == chatId && $0.queued }
  }
  public func issueStatusLabel(_ issue: ZapperIssue) -> String {
    queuedWorker(for: issue) != nil ? "Queued" : issue.details.status.label
  }
  /// Open issues grouped in board order, each group by priority then number.
  public func issueBoard(includeClosed: Bool = true) -> [(
    status: ZapperIssueStatus, issues: [ZapperIssue]
  )] {
    ZapperIssueStatus.board.compactMap { status in
      guard includeClosed || !status.terminal else { return nil }
      let group = issues.filter { $0.details.status == status }.sorted {
        let left = ZapperIssuePriority.allCases.firstIndex(of: $0.details.priority) ?? 0
        let right = ZapperIssuePriority.allCases.firstIndex(of: $1.details.priority) ?? 0
        return left == right ? $0.number > $1.number : left < right
      }
      return group.isEmpty ? nil : (status, group)
    }
  }
  public var defaultAvatar: ZapperAvatar { ZapperMascots.defaultAvatar(seed: id) }
  public var avatar: ZapperAvatar { config.avatar ?? defaultAvatar }
}

public enum ZapperMembership {
  public static func isThreadID(_ id: String) -> Bool {
    ZapperProtocol.workerPrefixes.contains { id.hasPrefix($0) }
  }
  /// Whether a chat stays out of project lists: owned by a Zapper now (as coordinator,
  /// earlier coordinator or worker, or a side chat of one), or a thread a Zapper started.
  /// Threads a Zapper adopted keep their own id and rejoin their project on release.
  public static func keepsChat(
    _ id: String, parentChatId: String? = nil, members: Set<String>
  ) -> Bool {
    [id, parentChatId].compactMap { $0 }.contains { members.contains($0) || isThreadID($0) }
  }
  public static func members(_ zappers: [Zapper]) -> Set<String> {
    zappers.reduce(into: Set<String>()) { $0.formUnion($1.memberChatIDs) }
  }
  /// The rank of a chat from its live session status (`nil` when stale or absent).
  public static func rank(_ status: ZapperRunStatus?) -> ZapperThreadRank {
    switch status {
    case .working: return .working
    case .awaitingInput, .errored: return .needsYou
    case .idle, nil: return .idle
    }
  }
}

public enum ZapperDrive {
  /// Documents worth listing: non-empty, and instructions only once someone edited them.
  public static func isDocument(_ file: ZapperFile) -> Bool {
    (file.path != "INSTRUCTIONS.md" || file.version > 1)
      && !file.content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
  }
  /// The newest version of each path, in a stable reading order.
  public static func latest(_ files: [ZapperFile]) -> [ZapperFile] {
    var newest: [String: ZapperFile] = [:]
    for file in files where (newest[file.path]?.version ?? 0) < file.version {
      newest[file.path] = file
    }
    let order = ["INSTRUCTIONS.md", "DIGEST.md", "NOTES.md", "WORKERS.md"]
    return newest.values.sorted { lhs, rhs in
      let left = order.firstIndex(of: lhs.path) ?? order.count
      let right = order.firstIndex(of: rhs.path) ?? order.count
      return left == right ? lhs.path < rhs.path : left < right
    }
  }
  public static func title(_ path: String) -> String {
    switch path {
    case "INSTRUCTIONS.md": return "Instructions"
    case "WORKERS.md": return "Thread rules"
    case "NOTES.md": return "Notes"
    case "DIGEST.md": return "Digest"
    default:
      return (path as NSString).deletingPathExtension.replacingOccurrences(of: "_", with: " ")
    }
  }
  /// The first line of body text, without Markdown marks; a heading only when there is no body.
  public static func preview(_ content: String) -> String {
    let lines = content.split(whereSeparator: \.isNewline).map {
      $0.trimmingCharacters(in: .whitespaces)
    }.filter { !$0.isEmpty && !$0.hasPrefix("|") && !$0.hasPrefix("```") }
    let line = lines.first { !$0.hasPrefix("#") } ?? lines.first ?? ""
    return line.trimmingCharacters(in: CharacterSet(charactersIn: "#>-* "))
      .replacingOccurrences(of: "**", with: "").replacingOccurrences(of: "`", with: "")
  }
}

extension ZapperEvent {
  /// How the Overview names a report.
  public var title: String {
    switch kind {
    case "settled": return "Thread finished a turn"
    case "needsInput": return "Thread needs input"
    case "failed": return "Thread failed"
    case "interrupted": return "Thread was interrupted"
    case "deliveryPending": return "Message waiting to deliver"
    case "bindingPending": return "Waiting for the thread’s computer"
    case "pullRequest": return "Pull request linked"
    case "issueChanged": return "Issue updated"
    default: return workerChatId.isEmpty ? "Update" : "Thread update"
    }
  }
  /// The issue an issue event is about (`ZAP-3: …` summaries), when it names one.
  public var issueCode: String? {
    guard let code = summary.split(separator: ":", maxSplits: 1).first, code.hasPrefix("ZAP-"),
      Int(code.dropFirst(4)) != nil
    else { return nil }
    return String(code)
  }
}
