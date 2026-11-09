import Foundation
import Testing

@testable import ZuseCore

@Test func chatRetriesKeepDurableIdentitiesAndWorkspacePolicy() throws {
  let request = ChatCreation(
    projectID: "project", providerID: "claude", model: "model",
    permissionMode: "plan", freshWorktree: true)
  let restored = try JSONDecoder().decode(ChatCreation.self, from: JSONEncoder().encode(request))
  #expect(restored.payload == request.payload)
  #expect(restored.payload["operationId"]?.string == request.operationID)
  #expect(restored.payload["workspacePolicy"]?["_tag"].string == "fresh")
  #expect(restored.payload["permissionMode"]?.string == "plan")
  let existing = ChatCreation(
    projectID: "project", providerID: "codex", model: "model", worktreeID: "tree")
  #expect(existing.payload["workspacePolicy"]?["worktreeId"].string == "tree")
  #expect(existing.payload["workspacePolicy"]?["_tag"].string == "existing")
}

@Test func branchSourcesPreserveLocalAndRemoteSemantics() throws {
  let local = try JSONDecoder().decode(
    GitBranch.self,
    from: Data(#"{"name":"feature","kind":"local","remote":null,"current":false}"#.utf8))
  let remote = try JSONDecoder().decode(
    GitBranch.self,
    from: Data(#"{"name":"feature","kind":"remote","remote":"origin","current":false}"#.utf8))
  #expect(local.id != remote.id)
  #expect(remote.worktreeSource["remote"].string == "origin")
  #expect(local.worktreeSource["remote"] == .null)
}

@Test func chatCreationKeepsInitialPromptAcrossRetry() throws {
  let request = ChatCreation(
    projectID: "project", providerID: "codex", model: "model", initialPrompt: "Review this change")
  let restored = try JSONDecoder().decode(ChatCreation.self, from: JSONEncoder().encode(request))
  #expect(restored.payload == request.payload)
  #expect(restored.payload["initialPrompt"]?.string == "Review this change")
  let empty = ChatCreation(projectID: "project", providerID: "codex", model: "model")
  #expect(empty.payload["initialPrompt"] == nil)
}
