import Foundation
import Testing

@testable import ZuseCore

private func issue(
  _ id: String, number: Int, status: ZapperIssueStatus, priority: ZapperIssuePriority = .normal,
  owner: ZapperIssueOwner? = nil, updated: Int64 = 0
) -> ZapperIssue {
  ZapperIssue(
    id: id, number: number,
    details: ZapperIssueDetails(title: id, status: status, priority: priority, owner: owner),
    createdAt: 0, updatedAt: updated)
}

private func zapper(_ id: String, _ issues: [ZapperIssue]) -> Zapper {
  Zapper(
    id: id, revision: 1,
    config: ZapperConfig(
      name: id, purpose: "", homeDeviceId: "d", repositories: [],
      coordinatorModel: ZapperModel(harness: "codex"), workerDeviceId: "d",
      maxConcurrentWorkers: 2, maxFollowups: 3),
    status: .active, coordinatorChatId: "zapper-\(id)-0", issues: issues)
}

private let sample = [
  zapper(
    "release",
    [
      issue("ask", number: 1, status: .needsYou, owner: .user),
      issue("mine", number: 2, status: .todo, owner: .user),
      issue("urgent", number: 3, status: .inProgress, priority: .urgent, owner: .zapper),
      issue("fresh", number: 4, status: .inProgress, owner: .zapper, updated: 50),
      issue("stale", number: 5, status: .inProgress, owner: .zapper, updated: 10),
      issue("shipped", number: 6, status: .done, owner: .user),
    ]),
  zapper(
    "docs",
    [
      issue("someday", number: 1, status: .backlog, priority: .low),
      issue("dropped", number: 2, status: .canceled),
    ]),
]

@Test func sectionsFollowBoardOrderThenPriorityThenRecency() {
  let sections = IssueBoard.sections(IssueBoard.entries(sample))
  #expect(sections.map(\.status) == [.needsYou, .inProgress, .todo, .backlog, .done, .canceled])
  #expect(sections[1].entries.map(\.issue.id) == ["urgent", "fresh", "stale"])
  #expect(sections.filter(\.collapsedByDefault).map(\.status) == [.done, .canceled])
  #expect(sections.last?.entries.first?.zapperID == "docs")
}

@Test func scopesPickWhatIsYours() {
  let entries = IssueBoard.entries(sample)
  func ids(_ scope: IssueScope) -> [String] {
    IssueBoard.sections(entries, scope: scope).flatMap(\.entries).map(\.issue.id)
  }
  #expect(ids(.forYou) == ["ask", "mine", "shipped"])
  #expect(ids(.assignedToMe) == ["ask", "mine", "shipped"])
  #expect(ids(.all).count == 8)
  // A question waiting on you counts for you even before anyone owns it.
  let unowned = [IssueEntry(zapperID: "z", issue: issue("q", number: 9, status: .needsYou))]
  #expect(IssueBoard.sections(unowned, scope: .forYou).count == 1)
  #expect(IssueBoard.sections(unowned, scope: .assignedToMe).isEmpty)
  // Counts leave finished work out.
  #expect(IssueBoard.counts(entries)[.forYou] == 2)
  #expect(IssueBoard.counts(entries)[.all] == 6)
}

@Test func filtersNarrowByStatusPriorityAndZapper() {
  let entries = IssueBoard.entries(sample)
  var filter = IssueFilter(statuses: [.inProgress, .backlog])
  #expect(filter.activeCount == 1)
  #expect(
    IssueBoard.sections(entries, filter: filter).flatMap(\.entries).map(\.issue.id) == [
      "urgent", "fresh", "stale", "someday",
    ])
  filter.priorities = [.low]
  #expect(filter.activeCount == 2)
  #expect(
    IssueBoard.sections(entries, filter: filter).flatMap(\.entries).map(\.issue.id) == ["someday"])
  filter = IssueFilter(zappers: ["docs"])
  #expect(
    IssueBoard.sections(entries, filter: filter).flatMap(\.entries).map(\.issue.id) == [
      "someday", "dropped",
    ])
  #expect(IssueFilter().isEmpty)
}

@Test func statusChoicesAndHandBack() {
  #expect(IssueBoard.choosableStatuses(current: .todo).count == 6)
  #expect(IssueBoard.choosableStatuses(current: .needsYou).first == .needsYou)
  #expect(IssueBoard.choosableStatuses(current: .blocked).count == 7)
  let asked = issue("ask", number: 1, status: .needsYou, owner: .user).details
  // Moving a question on hands it back to the Zapper.
  let moved = IssueBoard.details(asked, movingTo: .inProgress)
  #expect(moved.status == .inProgress && moved.owner == .zapper)
  // Your own todo stays yours when you start it.
  let mine = issue("mine", number: 2, status: .todo, owner: .user).details
  #expect(IssueBoard.details(mine, movingTo: .inProgress).owner == .user)
  let handed = IssueBoard.details(mine, owner: .zapper)
  #expect(handed.owner == .zapper && handed.assigned)
  let cleared = IssueBoard.details(handed, owner: nil)
  #expect(cleared.owner == nil && !cleared.assigned && cleared.currentOwner == nil)
}

@Test func compactTimesReadLikeAMessageList() {
  let now = Date(timeIntervalSince1970: 1_000_000_000)
  func ago(_ seconds: Double) -> String {
    CompactTime.label(now.addingTimeInterval(-seconds), now: now)
  }
  #expect(ago(10) == "now")
  #expect(ago(-30) == "now")
  #expect(ago(9 * 60) == "9m")
  #expect(ago(3 * 3600) == "3h")
  #expect(ago(2 * 86_400) == "2d")
  #expect(ago(15 * 86_400) == "2w")
  #expect(ago(100 * 86_400) == "3mo")
  #expect(ago(800 * 86_400) == "2y")
  #expect(CompactTime.label(milliseconds: (1_000_000_000 - 7200) * 1000, now: now) == "2h")
}
