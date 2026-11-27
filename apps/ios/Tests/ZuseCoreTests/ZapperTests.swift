import Foundation
import Testing

@testable import ZuseCore

/// A `WatchZappers` item exactly as the Rust engine serializes it: explicit nulls for plain
/// `Option` fields, omitted `avatar`/`avatarImage`/`queuePosition`/`owner`/`parent`/`replyTo`.
private let snapshotJSON = #"""
  [{
    "id": "release",
    "revision": 12,
    "activityRevision": 4,
    "config": {
      "name": "Release lead",
      "purpose": "Ship the 2.4 release and keep main green.",
      "homeDeviceId": "mac-studio",
      "repositories": [{"deviceId": "mac-studio", "spaceId": "space-zuse"}],
      "coordinatorModel": {"harness": "claude-code", "model": null},
      "workerDeviceId": "mac-studio",
      "maxConcurrentWorkers": 2,
      "maxFollowups": 3,
      "avatar": {"mascot": "hex", "color": "pink"}
    },
    "status": "active",
    "coordinatorChatId": "zapper-release-7",
    "previousCoordinatorChats": ["zapper-release-0"],
    "workers": [
      {"chatId": "zapper-worker-a1", "deviceId": "mac-studio", "spaceId": "space-zuse",
       "objective": "Fix flaky sync test", "followups": 1},
      {"chatId": "zapper-worker-b2", "deviceId": "mac-studio", "spaceId": "space-zuse",
       "objective": "Draft release notes", "followups": 0, "queuePosition": 1}
    ],
    "decisions": [
      {"id": "d1", "workerChatId": null, "question": "Cut the release today?", "answer": null},
      {"id": "d2", "workerChatId": "zapper-worker-a1", "question": "Skip the slow suite?",
       "answer": "No"}
    ],
    "issues": [{
      "id": "issue-1", "number": 3,
      "details": {
        "title": "Sync test flakes on CI", "description": "Fails 1 in 5 runs.",
        "status": "needsYou", "priority": "urgent", "assigned": false, "dueDate": "2026-12-01",
        "labels": ["ci"], "relations": [], "owner": {"type": "user"},
        "doneCriteria": "Ten green runs in a row.", "outOfScope": ""
      },
      "createdAt": 1764100000000, "updatedAt": 1764103600000,
      "threads": ["zapper-worker-a1"],
      "pullRequests": [{"url": "https://github.com/acme/zuse/pull/41", "title": "Retry sync"}],
      "activity": [
        {"actor": null, "id": "act-1", "createdAt": 1764100000000, "kind": "created",
         "text": "Created the issue"},
        {"actor": "zapper-release-7", "id": "act-2", "createdAt": 1764103600000,
         "kind": "question", "text": "Quarantine the test or keep fixing?"}
      ]
    }]
  }, {
    "id": "legacy",
    "revision": 1,
    "config": {
      "name": "Older", "purpose": "", "homeDeviceId": "d", "repositories": [],
      "coordinatorModel": {"harness": "codex", "model": "gpt"}, "workerDeviceId": "d",
      "maxConcurrentWorkers": 1, "maxFollowups": 0
    },
    "status": "paused",
    "coordinatorChatId": "zot-legacy-0",
    "previousCoordinatorChats": [],
    "workers": [],
    "decisions": []
  }, {
    "id": "broken", "revision": "not a number"
  }]
  """#

private func decodeSnapshot() throws -> [Zapper] {
  Zapper.snapshot(try JSONDecoder().decode(JSONValue.self, from: Data(snapshotJSON.utf8)))
}

@Test func watchSnapshotDecodesEveryRustField() throws {
  let zappers = try decodeSnapshot()
  #expect(zappers.map(\.id) == ["release", "legacy"])
  let release = zappers[0]
  #expect(release.revision == 12 && release.activityRevision == 4)
  #expect(release.status == .active)
  #expect(release.config.coordinatorModel == ZapperModel(harness: "claude-code", model: nil))
  #expect(release.config.avatar == ZapperAvatar(mascot: "hex", color: "pink"))
  #expect(release.config.avatarImage == nil)
  #expect(release.workers[0].queuePosition == nil && !release.workers[0].queued)
  #expect(release.workers[1].queuePosition == 1 && release.workers[1].queued)
  #expect(release.decisions[0].answer == nil && release.decisions[1].answer == "No")
  let issue = try #require(release.issues.first)
  #expect(issue.code == "ZAP-3")
  #expect(issue.details.status == .needsYou && issue.details.priority == .urgent)
  #expect(issue.details.owner == .user && issue.details.currentOwner == .user)
  #expect(issue.details.parent == nil && issue.details.dueDate == "2026-12-01")
  #expect(issue.activity[1].replyTo == nil && issue.activity[0].actor == nil)
  #expect(issue.pullRequests.first?.title == "Retry sync")
  // `activityRevision` and `issues` default when an older engine omits them.
  #expect(zappers[1].activityRevision == 0 && zappers[1].issues.isEmpty)
}

@Test func issueDetailsFillRustDefaultsAndTolerateNewValues() throws {
  let details = try JSONDecoder().decode(
    ZapperIssueDetails.self,
    from: Data(
      #"{"title":"T","description":"","status":"someday","priority":"critical","dueDate":null,"owner":{"type":"thread","chatId":"zapper-worker-x"}}"#
        .utf8))
  #expect(details.status == .backlog && details.priority == .normal)
  #expect(details.labels.isEmpty && details.relations.isEmpty && details.doneCriteria.isEmpty)
  #expect(details.owner == .thread(chatId: "zapper-worker-x"))
  let legacy = try JSONDecoder().decode(
    ZapperIssueDetails.self,
    from: Data(#"{"title":"T","description":"","assigned":true,"dueDate":null}"#.utf8))
  #expect(legacy.owner == nil && legacy.currentOwner == .zapper)
}

@Test func encodedRecordsOmitWhatRustOmits() throws {
  let worker = ZapperWorker(
    chatId: "zapper-worker-c", deviceId: "d", spaceId: "s", objective: "Ship")
  let json = try JSONValue.encoding(worker)
  #expect(json["queuePosition"] == .null)
  guard case .object(let fields) = json else {
    Issue.record("not an object")
    return
  }
  #expect(!fields.keys.contains("queuePosition"))
  let owner = try JSONValue.encoding(ZapperIssueOwner.thread(chatId: "c"))
  #expect(owner == .object(["type": .string("thread"), "chatId": .string("c")]))
  #expect(try JSONValue.encoding(ZapperIssueOwner.user) == .object(["type": .string("user")]))
}

@Test func eventsAcceptTheirLegacyIdField() throws {
  let events = try JSONDecoder().decode(
    [ZapperEvent].self,
    from: Data(
      #"[{"id":"e1","zapperId":"z","workerChatId":"w","kind":"settled","summary":"Tests pass","acknowledged":false},{"id":"e2","zotId":"z","workerChatId":"w","kind":"failed","summary":"x","acknowledged":true}]"#
        .utf8))
  #expect(events.map(\.zapperId) == ["z", "z"])
  #expect(events.map(\.title) == ["Thread finished a turn", "Thread failed"])
}

@Test func commandEnvelopesMatchTheRustCommandShape() throws {
  let decision = ZapperDecision(id: "d1", question: "Cut the release today?", answer: "Yes")
  let command = ZapperCommand(
    zapperId: "release", expectedRevision: 12, action: .decide(decision), commandId: "cmd-1")
  let payload = try command.payload()
  #expect(payload["commandId"] == .string("cmd-1"))
  #expect(payload["zapperId"] == .string("release"))
  #expect(payload["expectedRevision"] == .number(12))
  #expect(payload["action"]?["type"] == .string("decide"))
  #expect(payload["action"]?["decision"]["answer"] == .string("Yes"))

  let status = try ZapperAction.setStatus(.paused).json()
  #expect(status == .object(["type": .string("setStatus"), "status": .string("paused")]))
  let comment = try ZapperAction.commentIssue(issueId: "i", text: "Keep fixing", replyTo: nil)
    .json()
  guard case .object(let fields) = comment else {
    Issue.record("not an object")
    return
  }
  #expect(Set(fields.keys) == ["type", "issueId", "text"])
  #expect(
    try ZapperAction.freshConversation.json() == .object(["type": .string("freshConversation")]))
  #expect(
    try ZapperAction.writeFile(path: "NOTES.md", content: "x", expectedVersion: 2).json()[
      "expectedVersion"] == .number(2))
  let unpinned = try ZapperCommand(zapperId: "z", expectedRevision: nil, action: .delete).payload()
  #expect(unpinned["expectedRevision"] == nil)
  #expect(ZapperCommand(zapperId: "z", expectedRevision: 1, action: .delete).commandId.count == 36)
}

@Test func mascotsMatchTheSharedArt() {
  #expect(
    ZapperMascots.defaultAvatar(seed: "zapper") == ZapperMascots.defaultAvatar(seed: "zapper"))
  // FNV-1a of "a" is 0xe40c292c, as in the Rust renderer.
  #expect(ZapperMascots.hash("a") == 0xe40c_292c)
  let unknown = ZapperMascots.resolve(ZapperAvatar(mascot: "gone", color: "gone"))
  #expect(unknown.0.id == "bolt" && unknown.1.id == "coral")
  #expect(ZapperMascots.resolve(ZapperAvatar(mascot: "hex", color: "teal")).0.name == "Orbit")
  #expect(ZapperMascots.grid(for: 16) == 16 && ZapperMascots.grid(for: 40) == 26)
  for grid in [16, 26, 42] {
    var seen = Set<[ZapperMascots.Cell?]>()
    for mascot in ZapperMascots.mascots {
      let cells = ZapperMascots.raster(mascot, grid: grid)
      #expect(seen.insert(cells).inserted, "\(mascot.id) repeats at \(grid)")
      #expect(cells.compactMap { $0 }.count > grid * grid * 35 / 100)
      #expect(cells.filter { $0 == .eye }.count >= 2, "\(mascot.id) has no eyes at \(grid)")
      if mascot.id != "rogue" {
        for y in 0..<grid {
          for x in 0..<grid where cells[y * grid + x] != cells[y * grid + grid - 1 - x] {
            Issue.record("\(mascot.id) is lopsided at \(grid)")
          }
        }
      }
    }
  }
  let resting = ZapperMascots.raster(ZapperMascots.mascots[0], grid: 26, blink: 1)
  let awake = ZapperMascots.raster(ZapperMascots.mascots[0], grid: 26)
  #expect(resting.filter { $0 == .eye }.count < awake.filter { $0 == .eye }.count)
}
