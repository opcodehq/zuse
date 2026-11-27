import Foundation
import Testing

@testable import ZuseCore

private func zapper(
  workers: [ZapperWorker] = [], decisions: [ZapperDecision] = [], issues: [ZapperIssue] = []
) -> Zapper {
  Zapper(
    id: "z", revision: 3,
    config: ZapperConfig(
      name: "Ops", purpose: "", homeDeviceId: "d", repositories: [],
      coordinatorModel: ZapperModel(harness: "codex"), workerDeviceId: "d",
      maxConcurrentWorkers: 2, maxFollowups: 3),
    status: .active, coordinatorChatId: "zapper-z-2", previousCoordinatorChats: ["zapper-z-0"],
    workers: workers, decisions: decisions, issues: issues)
}

private func worker(_ id: String, queue: Int? = nil) -> ZapperWorker {
  ZapperWorker(
    chatId: id, deviceId: "d", spaceId: "s", objective: id, queuePosition: queue)
}

private func issue(
  _ id: String, number: Int, status: ZapperIssueStatus, priority: ZapperIssuePriority = .normal,
  owner: ZapperIssueOwner? = nil, threads: [String] = [], activity: [ZapperIssueActivity] = []
) -> ZapperIssue {
  ZapperIssue(
    id: id, number: number,
    details: ZapperIssueDetails(title: id, status: status, priority: priority, owner: owner),
    createdAt: 0, updatedAt: 0, threads: threads, activity: activity)
}

@Test func threadsOrderByRankThenNewestFirst() {
  let z = zapper(workers: [
    worker("old-idle"), worker("working"), worker("queued-2", queue: 2), worker("asks"),
    worker("queued-1", queue: 1), worker("new-idle"),
  ])
  let ranks: [String: ZapperThreadRank] = [
    "working": .working, "asks": .needsYou, "queued-1": .working,
  ]
  let ordered = z.orderedThreads { ranks[$0.chatId] ?? .idle }.map(\.chatId)
  // Queued wins over whatever the chat says; newest first within a rank.
  #expect(ordered == ["working", "asks", "queued-1", "queued-2", "new-idle", "old-idle"])
  #expect(ZapperMembership.rank(.errored) == .needsYou)
  #expect(ZapperMembership.rank(nil) == .idle)
}

@Test func membershipKeepsZapperChatsOutOfProjects() {
  let z = zapper(workers: [worker("adopted-chat")])
  let members = ZapperMembership.members([z])
  #expect(members == ["zapper-z-2", "zapper-z-0", "adopted-chat"])
  #expect(ZapperMembership.keepsChat("zapper-z-0", members: members))
  #expect(ZapperMembership.keepsChat("adopted-chat", members: members))
  // Released delegated threads keep their id and stay out; side chats follow their parent.
  #expect(ZapperMembership.keepsChat("zapper-worker-gone", members: members))
  #expect(ZapperMembership.keepsChat("zot-worker-old", members: members))
  #expect(ZapperMembership.keepsChat("side", parentChatId: "adopted-chat", members: members))
  #expect(!ZapperMembership.keepsChat("ordinary", members: members))
}

@Test func needsYouCountsDecisionsAndHandedOverIssues() {
  let question = ZapperIssueActivity(
    id: "q", createdAt: 2, kind: "question", text: "  Which branch?  ")
  let z = zapper(
    decisions: [
      ZapperDecision(id: "open", question: "Ship it?"),
      ZapperDecision(id: "done", question: "Old", answer: "Yes"),
    ],
    issues: [
      issue("asked", number: 4, status: .needsYou, owner: .user, activity: [question]),
      issue("legacy", number: 5, status: .needsYou),
      issue("busy", number: 6, status: .inProgress, owner: .zapper),
    ])
  #expect(z.needsYouCount == 3)
  #expect(z.needsYouItems.map(\.id) == ["issue:asked", "decision:open"])
  #expect(z.needsYouItems.first?.title == "ZAP-4 · Which branch?")
}

@Test func issueBoardGroupsInBoardOrderByPriority() {
  let z = zapper(
    workers: [worker("zapper-worker-q", queue: 1)],
    issues: [
      issue("low", number: 1, status: .todo, priority: .low),
      issue("urgent", number: 2, status: .todo, priority: .urgent),
      issue("shipped", number: 3, status: .done),
      issue("asks", number: 4, status: .needsYou, owner: .user),
      issue(
        "waiting", number: 5, status: .inProgress, owner: .thread(chatId: "zapper-worker-q")),
    ])
  #expect(z.issueBoard().map(\.status) == [.needsYou, .inProgress, .todo, .done])
  #expect(z.issueBoard()[2].issues.map(\.id) == ["urgent", "low"])
  #expect(z.issueBoard(includeClosed: false).map(\.status) == [.needsYou, .inProgress, .todo])
  #expect(z.issueStatusLabel(z.issues[4]) == "Queued")
  #expect(z.issueStatusLabel(z.issues[0]) == "Todo")
}

@Test func finishedThreadsComeFromIssuesNewestFirst() {
  let z = zapper(
    workers: [worker("zapper-worker-live")],
    issues: [
      issue("a", number: 1, status: .done, threads: ["zapper-worker-1", "zapper-worker-2"]),
      issue("b", number: 2, status: .inProgress, threads: ["zapper-worker-live", "adopted"]),
      issue("c", number: 3, status: .done, threads: ["zapper-worker-3", "zapper-worker-1"]),
    ])
  #expect(z.finishedThreads == ["zapper-worker-1", "zapper-worker-3", "zapper-worker-2"])
}

@Test func driveListsEditedDocumentsNewestVersionFirst() {
  let files = [
    ZapperFile(path: "INSTRUCTIONS.md", version: 1, content: "Ship it"),
    ZapperFile(path: "NOTES.md", version: 1, content: "## Notes\nfirst"),
    ZapperFile(path: "NOTES.md", version: 3, content: "# Release notes\n\nLatest"),
    ZapperFile(path: "DIGEST.md", version: 1, content: "   "),
    ZapperFile(path: "WORKERS.md", version: 2, content: "- Keep PRs small"),
  ]
  let latest = ZapperDrive.latest(files)
  #expect(latest.map(\.id) == ["INSTRUCTIONS.md@1", "DIGEST.md@1", "NOTES.md@3", "WORKERS.md@2"])
  #expect(latest.filter(ZapperDrive.isDocument).map(\.path) == ["NOTES.md", "WORKERS.md"])
  #expect(ZapperDrive.preview(latest[2].content) == "Latest")
  #expect(ZapperDrive.preview("# Digest") == "Digest")
  #expect(ZapperDrive.preview("## Blockers\n- **Windows** `ci` fails") == "Windows ci fails")
  #expect(ZapperDrive.title("WORKERS.md") == "Thread rules")
}
