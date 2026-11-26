import Foundation
import Testing

@testable import ZuseCore

@Test func draftIsValidOnlyWithANameProjectAndModel() {
  var draft = ZapperDraft(zapperID: "z1")
  #expect(!draft.isValid)
  draft.name = "   "
  draft.projectID = "space-1"
  draft.harness = "codex"
  #expect(!draft.isValid)
  draft.name = " Release lead "
  #expect(draft.isValid)
  draft.projectID = CanonicalWorkspace.projectless
  #expect(!draft.isValid)
  draft.projectID = "space-1"
  draft.harness = ""
  #expect(!draft.isValid)
  draft.harness = "codex"
  draft.name = String(repeating: "a", count: 257)
  #expect(!draft.isValid)
  draft.name = "Ok"
  draft.maxConcurrentWorkers = 0
  #expect(!draft.isValid)
}

@Test func draftBuildsTheCreateCommand() throws {
  var draft = ZapperDraft(
    zapperID: "z1", name: " Release lead ", purpose: " Ship it. \n", projectID: "space-1",
    harness: "claude-code", model: "")
  #expect(draft.avatar == ZapperMascots.defaultAvatar(seed: "z1"))
  let config = try #require(draft.config(homeDeviceID: "mac", repositoryDeviceID: ""))
  #expect(config.name == "Release lead")
  #expect(config.purpose == "Ship it.")
  #expect(config.homeDeviceId == "mac" && config.workerDeviceId == "mac")
  #expect(config.repositories == [ZapperRepository(deviceId: "mac", spaceId: "space-1")])
  #expect(config.coordinatorModel == ZapperModel(harness: "claude-code", model: nil))
  #expect(config.maxConcurrentWorkers == 4 && config.maxFollowups == 3)
  draft.model = "sonnet"
  let command = try #require(draft.command(homeDeviceID: "mac", repositoryDeviceID: "studio"))
  #expect(command.zapperId == "z1" && command.expectedRevision == nil)
  let payload = try command.payload()
  #expect(payload["expectedRevision"] == nil)
  let action = try #require(payload["action"])
  #expect(action["type"].string == "create")
  #expect(action["config"]["repositories"].array.first?["deviceId"].string == "studio")
  #expect(action["config"]["coordinatorModel"]["model"].string == "sonnet")
  #expect(action["config"]["avatar"]["mascot"].string == draft.avatar.mascot)
  #expect(ZapperDraft(zapperID: "z2").config(homeDeviceID: "mac", repositoryDeviceID: "") == nil)
}

@Test func changeLookShowsADifferentFaceEachTime() {
  var draft = ZapperDraft(zapperID: "z1")
  var seen: Set<ZapperAvatar> = [draft.avatar]
  for _ in 0..<5 {
    draft.nextLook()
    seen.insert(draft.avatar)
  }
  #expect(seen.count == 6)
  #expect(ZapperMascots.mascots.contains { $0.id == draft.avatar.mascot })
  #expect(ZapperMascots.colors.contains { $0.id == draft.avatar.color })
}
