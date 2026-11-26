import Foundation

/// What the New Zapper sheet collects, and the `create` config it becomes.
public struct ZapperDraft: Sendable, Hashable {
  /// Client-minted, like the desktop form; also seeds the default look.
  public let zapperID: String
  public var name: String
  public var purpose: String
  public var projectID: String?
  public var harness: String?
  public var model: String?
  public var avatar: ZapperAvatar
  public var maxConcurrentWorkers: Int
  public var maxFollowups: Int

  public init(
    zapperID: String = UUID().uuidString.lowercased(), name: String = "", purpose: String = "",
    projectID: String? = nil, harness: String? = nil, model: String? = nil,
    avatar: ZapperAvatar? = nil, maxConcurrentWorkers: Int = 4, maxFollowups: Int = 3
  ) {
    self.zapperID = zapperID
    self.name = name
    self.purpose = purpose
    self.projectID = projectID
    self.harness = harness
    self.model = model
    self.avatar = avatar ?? ZapperMascots.defaultAvatar(seed: zapperID)
    self.maxConcurrentWorkers = maxConcurrentWorkers
    self.maxFollowups = maxFollowups
  }

  public var trimmedName: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

  /// Mirrors the engine's checks so the confirm button is only on when `create` can pass.
  public var isValid: Bool {
    let name = trimmedName
    guard !name.isEmpty, name.utf8.count <= 256, purpose.utf8.count <= 65_536 else {
      return false
    }
    guard let projectID, !projectID.isEmpty, projectID != CanonicalWorkspace.projectless else {
      return false
    }
    guard let harness, !harness.isEmpty else { return false }
    return (1...32).contains(maxConcurrentWorkers) && (0...32).contains(maxFollowups)
  }

  /// The next mascot and colour, so each tap of "Change look" shows a different face.
  public mutating func nextLook() {
    let mascots = ZapperMascots.mascots
    let colors = ZapperMascots.colors
    let (mascot, color) = ZapperMascots.resolve(avatar)
    let m = mascots.firstIndex(of: mascot) ?? 0
    let c = colors.firstIndex(of: color) ?? 0
    avatar = ZapperAvatar(
      mascot: mascots[(m + 1) % mascots.count].id, color: colors[(c + 1) % colors.count].id)
  }

  /// The config to send with `create`. The Zapper lives on `homeDeviceID` (the engine that
  /// receives the command) and its threads run there too; the project's repository lives on
  /// `repositoryDeviceID`. Nil until the draft is valid.
  public func config(homeDeviceID: String, repositoryDeviceID: String) -> ZapperConfig? {
    guard isValid, let projectID, let harness, !homeDeviceID.isEmpty else { return nil }
    let model = model?.trimmingCharacters(in: .whitespacesAndNewlines)
    return ZapperConfig(
      name: trimmedName, purpose: purpose.trimmingCharacters(in: .whitespacesAndNewlines),
      homeDeviceId: homeDeviceID,
      repositories: [
        ZapperRepository(
          deviceId: repositoryDeviceID.isEmpty ? homeDeviceID : repositoryDeviceID,
          spaceId: projectID)
      ],
      coordinatorModel: ZapperModel(harness: harness, model: model?.isEmpty == false ? model : nil),
      workerDeviceId: homeDeviceID, maxConcurrentWorkers: maxConcurrentWorkers,
      maxFollowups: maxFollowups, avatar: avatar)
  }

  /// The `create` command for this draft. `create` carries no expected revision.
  public func command(homeDeviceID: String, repositoryDeviceID: String) -> ZapperCommand? {
    config(homeDeviceID: homeDeviceID, repositoryDeviceID: repositoryDeviceID).map {
      ZapperCommand(zapperId: zapperID, expectedRevision: nil, action: .create(config: $0))
    }
  }
}
