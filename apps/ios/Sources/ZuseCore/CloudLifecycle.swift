import Foundation

public enum CloudChatScope: String, Sendable { case active, archived, all }
public enum CloudWorkspaceAction: String, Codable, Sendable, CaseIterable {
  case pause, resume, archive, unarchive
  public var title: String {
    switch self {
    case .pause: return "Pause"
    case .resume: return "Wake"
    case .archive: return "Archive"
    case .unarchive: return "Restore"
    }
  }
  public var symbol: String {
    switch self {
    case .pause: return "pause"
    case .resume: return "power"
    case .archive: return "archivebox"
    case .unarchive: return "arrow.uturn.backward"
    }
  }
  public var stopsWork: Bool { self == .pause || self == .archive }
}

extension CloudChat {
  public var isArchived: Bool { state == "archived" || desiredState == "archived" }
  public var isTransitioning: Bool {
    ["queued", "provisioning", "setup", "pausing", "resuming", "archiving", "deleting"].contains(
      state)
      || (state == "paused" && desiredState == "ready")
      || (state == "ready" && ["paused", "archived"].contains(desiredState ?? ""))
      || (state == "paused" && desiredState == "archived")
      || (state == "archived" && ["paused", "ready"].contains(desiredState ?? ""))
  }
  public var statusLabel: String {
    if state == "paused" && desiredState == "ready" { return "Waking" }
    if state == "ready" && desiredState == "paused" { return "Pausing" }
    if state == "ready" && desiredState == "archived" { return "Archiving" }
    if state == "paused" && desiredState == "archived" { return "Archiving" }
    if state == "archived" && ["paused", "ready"].contains(desiredState ?? "") {
      return "Restoring"
    }
    return state.replacingOccurrences(of: "_", with: " ").capitalized
  }
  public var availableActions: [CloudWorkspaceAction] {
    if isTransitioning { return [] }
    if isArchived { return [.unarchive] }
    switch state {
    case "paused", "failed": return [.resume, .archive]
    case "ready": return [.pause, .archive]
    default: return []
    }
  }
}

extension CloudClient {
  public func control(workspaceID: String, action: CloudWorkspaceAction, commandID: String)
    async throws -> JSONValue
  {
    let id = workspaceID.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
    let response = try await request(
      "/v1/cloud/workspaces/\(id)/\(action.rawValue)", method: "POST",
      body: .object([
        "workspaceId": .string(workspaceID), "commandId": .string(commandID),
      ]))
    guard response["workspaceId"].string == workspaceID else {
      throw ClientError(
        "The cloud action response did not identify the selected workspace. Retry the saved action to confirm its outcome."
      )
    }
    return response
  }
}
