import Foundation

/// Zapper contracts mirrored from `crates/protocol/src/zappers.rs`. Field names match the
/// Rust serde output (camelCase); optionals Rust omits are omitted here too.
public enum ZapperProtocol {
  /// Advertised in `ControlInfo.capabilities` when the engine hosts Zappers.
  public static let capability = "zappers.v1"
  /// Coordinator chats are `zapper-<zapper>-<n>`; `zot-` ids predate the rename.
  public static let coordinatorPrefixes = ["zapper-", "zot-"]
  /// Delegated threads are `zapper-worker-<commandId>`; `zot-worker-` predates the rename.
  public static let workerPrefixes = ["zapper-worker-", "zot-worker-"]
}

public enum ZapperStatus: String, Codable, Sendable, CaseIterable {
  case active, paused, stopped, archived
}

public struct ZapperRepository: Codable, Sendable, Hashable {
  public var deviceId: String
  public var spaceId: String
  public init(deviceId: String, spaceId: String) {
    self.deviceId = deviceId
    self.spaceId = spaceId
  }
}

public struct ZapperModel: Codable, Sendable, Hashable {
  public var harness: String
  public var model: String?
  public init(harness: String, model: String? = nil) {
    self.harness = harness
    self.model = model
  }
}

public struct ZapperAvatar: Codable, Sendable, Hashable {
  public var mascot: String
  public var color: String
  public init(mascot: String, color: String) {
    self.mascot = mascot
    self.color = color
  }
}

public struct ZapperConfig: Codable, Sendable, Hashable {
  public var name: String
  public var purpose: String
  public var homeDeviceId: String
  public var repositories: [ZapperRepository]
  public var coordinatorModel: ZapperModel
  public var workerDeviceId: String
  public var maxConcurrentWorkers: Int
  public var maxFollowups: Int
  public var avatar: ZapperAvatar?
  /// A small square `data:` URL picture shown instead of the mascot.
  public var avatarImage: String?
  public init(
    name: String, purpose: String, homeDeviceId: String, repositories: [ZapperRepository],
    coordinatorModel: ZapperModel, workerDeviceId: String, maxConcurrentWorkers: Int,
    maxFollowups: Int, avatar: ZapperAvatar? = nil, avatarImage: String? = nil
  ) {
    self.name = name
    self.purpose = purpose
    self.homeDeviceId = homeDeviceId
    self.repositories = repositories
    self.coordinatorModel = coordinatorModel
    self.workerDeviceId = workerDeviceId
    self.maxConcurrentWorkers = maxConcurrentWorkers
    self.maxFollowups = maxFollowups
    self.avatar = avatar
    self.avatarImage = avatarImage
  }
}

public struct ZapperWorker: Codable, Sendable, Hashable, Identifiable {
  public var chatId: String
  public var deviceId: String
  public var spaceId: String
  public var objective: String
  public var followups: Int
  /// One-based place in the start queue; absent once the thread started.
  public var queuePosition: Int?
  public var id: String { chatId }
  public var queued: Bool { queuePosition != nil }
  public init(
    chatId: String, deviceId: String, spaceId: String, objective: String, followups: Int = 0,
    queuePosition: Int? = nil
  ) {
    self.chatId = chatId
    self.deviceId = deviceId
    self.spaceId = spaceId
    self.objective = objective
    self.followups = followups
    self.queuePosition = queuePosition
  }
}

public struct ZapperDecision: Codable, Sendable, Hashable, Identifiable {
  public var id: String
  public var workerChatId: String?
  public var question: String
  public var answer: String?
  public init(id: String, workerChatId: String? = nil, question: String, answer: String? = nil) {
    self.id = id
    self.workerChatId = workerChatId
    self.question = question
    self.answer = answer
  }
}

/// Unknown values decode as the Rust default, so a newer engine cannot blank the list.
public enum ZapperIssueStatus: String, Codable, Sendable, CaseIterable {
  case backlog, todo, inProgress, needsYou, blocked, inReview, done, canceled
  /// Board order: what needs the user first, then live work, then queued, then finished.
  public static let board: [ZapperIssueStatus] = [
    .needsYou, .inProgress, .inReview, .blocked, .todo, .backlog, .done, .canceled,
  ]
  public var label: String {
    switch self {
    case .backlog: return "Backlog"
    case .todo: return "Todo"
    case .inProgress: return "In progress"
    case .needsYou: return "Needs you"
    case .blocked: return "Blocked"
    case .inReview: return "In review"
    case .done: return "Done"
    case .canceled: return "Canceled"
    }
  }
  public var terminal: Bool { self == .done || self == .canceled }
  public init(from decoder: Decoder) throws {
    self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .backlog
  }
}

public enum ZapperIssuePriority: String, Codable, Sendable, CaseIterable {
  case urgent, high, normal, low
  public var label: String {
    switch self {
    case .urgent: return "Urgent"
    case .high: return "High"
    case .normal: return "Normal"
    case .low: return "Low"
    }
  }
  public init(from decoder: Decoder) throws {
    self = Self(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .normal
  }
}

/// Internally tagged: `{"type":"zapper"}`, `{"type":"user"}`, `{"type":"thread","chatId":…}`.
public enum ZapperIssueOwner: Codable, Sendable, Hashable {
  case zapper
  case user
  case thread(chatId: String)
  private enum Keys: String, CodingKey { case type, chatId }
  public init(from decoder: Decoder) throws {
    let container = try decoder.container(keyedBy: Keys.self)
    switch try container.decode(String.self, forKey: .type) {
    case "zapper": self = .zapper
    case "user": self = .user
    case "thread": self = .thread(chatId: try container.decode(String.self, forKey: .chatId))
    case let other:
      throw DecodingError.dataCorruptedError(
        forKey: .type, in: container, debugDescription: "Unknown issue owner \(other)")
    }
  }
  public func encode(to encoder: Encoder) throws {
    var container = encoder.container(keyedBy: Keys.self)
    switch self {
    case .zapper: try container.encode("zapper", forKey: .type)
    case .user: try container.encode("user", forKey: .type)
    case .thread(let chatId):
      try container.encode("thread", forKey: .type)
      try container.encode(chatId, forKey: .chatId)
    }
  }
  public var label: String {
    switch self {
    case .zapper: return "Zapper"
    case .user: return "You"
    case .thread: return "Thread"
    }
  }
}

public enum ZapperIssueOutcome: String, Codable, Sendable {
  case merged, dropped
}

public struct ZapperIssueDetails: Codable, Sendable, Hashable {
  public var title: String
  public var description: String
  public var status: ZapperIssueStatus
  public var priority: ZapperIssuePriority
  public var assigned: Bool
  /// ISO calendar date (`YYYY-MM-DD`).
  public var dueDate: String?
  public var labels: [String]
  public var relations: [String]
  public var owner: ZapperIssueOwner?
  public var doneCriteria: String
  public var outOfScope: String
  public var parent: String?
  public init(
    title: String, description: String = "", status: ZapperIssueStatus = .backlog,
    priority: ZapperIssuePriority = .normal, assigned: Bool = false, dueDate: String? = nil,
    labels: [String] = [], relations: [String] = [], owner: ZapperIssueOwner? = nil,
    doneCriteria: String = "", outOfScope: String = "", parent: String? = nil
  ) {
    self.title = title
    self.description = description
    self.status = status
    self.priority = priority
    self.assigned = assigned
    self.dueDate = dueDate
    self.labels = labels
    self.relations = relations
    self.owner = owner
    self.doneCriteria = doneCriteria
    self.outOfScope = outOfScope
    self.parent = parent
  }
  private enum Keys: String, CodingKey {
    case title, description, status, priority, assigned, dueDate, labels, relations, owner
    case doneCriteria, outOfScope, parent
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: Keys.self)
    title = try c.decode(String.self, forKey: .title)
    description = try c.decode(String.self, forKey: .description)
    status = try c.decodeIfPresent(ZapperIssueStatus.self, forKey: .status) ?? .backlog
    priority = try c.decodeIfPresent(ZapperIssuePriority.self, forKey: .priority) ?? .normal
    assigned = try c.decodeIfPresent(Bool.self, forKey: .assigned) ?? false
    dueDate = try c.decodeIfPresent(String.self, forKey: .dueDate)
    labels = try c.decodeIfPresent([String].self, forKey: .labels) ?? []
    relations = try c.decodeIfPresent([String].self, forKey: .relations) ?? []
    owner = try c.decodeIfPresent(ZapperIssueOwner.self, forKey: .owner)
    doneCriteria = try c.decodeIfPresent(String.self, forKey: .doneCriteria) ?? ""
    outOfScope = try c.decodeIfPresent(String.self, forKey: .outOfScope) ?? ""
    parent = try c.decodeIfPresent(String.self, forKey: .parent)
  }
  /// The owner, reading issues saved before owners existed (`assigned` meant the Zapper).
  public var currentOwner: ZapperIssueOwner? { owner ?? (assigned ? .zapper : nil) }
}

public struct ZapperIssuePullRequest: Codable, Sendable, Hashable {
  public var url: String
  public var title: String
  public init(url: String, title: String) {
    self.url = url
    self.title = title
  }
}

public struct ZapperIssueActivity: Codable, Sendable, Hashable, Identifiable {
  public var actor: String?
  public var id: String
  /// Unix milliseconds.
  public var createdAt: Int64
  public var kind: String
  public var text: String
  public var replyTo: String?
  public init(
    id: String, actor: String? = nil, createdAt: Int64, kind: String, text: String,
    replyTo: String? = nil
  ) {
    self.id = id
    self.actor = actor
    self.createdAt = createdAt
    self.kind = kind
    self.text = text
    self.replyTo = replyTo
  }
}

public struct ZapperIssue: Codable, Sendable, Hashable, Identifiable {
  public var id: String
  public var number: Int
  public var details: ZapperIssueDetails
  /// Unix milliseconds.
  public var createdAt: Int64
  public var updatedAt: Int64
  public var threads: [String]
  public var pullRequests: [ZapperIssuePullRequest]
  public var activity: [ZapperIssueActivity]
  public var code: String { "ZAP-\(number)" }
  public init(
    id: String, number: Int, details: ZapperIssueDetails, createdAt: Int64, updatedAt: Int64,
    threads: [String] = [], pullRequests: [ZapperIssuePullRequest] = [],
    activity: [ZapperIssueActivity] = []
  ) {
    self.id = id
    self.number = number
    self.details = details
    self.createdAt = createdAt
    self.updatedAt = updatedAt
    self.threads = threads
    self.pullRequests = pullRequests
    self.activity = activity
  }
}

public struct Zapper: Codable, Sendable, Hashable, Identifiable {
  public var id: String
  public var revision: Int
  public var activityRevision: Int
  public var config: ZapperConfig
  public var status: ZapperStatus
  public var coordinatorChatId: String
  public var previousCoordinatorChats: [String]
  public var workers: [ZapperWorker]
  public var decisions: [ZapperDecision]
  public var issues: [ZapperIssue]
  public init(
    id: String, revision: Int, activityRevision: Int = 0, config: ZapperConfig,
    status: ZapperStatus, coordinatorChatId: String, previousCoordinatorChats: [String] = [],
    workers: [ZapperWorker] = [], decisions: [ZapperDecision] = [], issues: [ZapperIssue] = []
  ) {
    self.id = id
    self.revision = revision
    self.activityRevision = activityRevision
    self.config = config
    self.status = status
    self.coordinatorChatId = coordinatorChatId
    self.previousCoordinatorChats = previousCoordinatorChats
    self.workers = workers
    self.decisions = decisions
    self.issues = issues
  }
  private enum Keys: String, CodingKey {
    case id, revision, activityRevision, config, status, coordinatorChatId
    case previousCoordinatorChats, workers, decisions, issues
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: Keys.self)
    id = try c.decode(String.self, forKey: .id)
    revision = try c.decode(Int.self, forKey: .revision)
    activityRevision = try c.decodeIfPresent(Int.self, forKey: .activityRevision) ?? 0
    config = try c.decode(ZapperConfig.self, forKey: .config)
    status = try c.decode(ZapperStatus.self, forKey: .status)
    coordinatorChatId = try c.decode(String.self, forKey: .coordinatorChatId)
    previousCoordinatorChats = try c.decode([String].self, forKey: .previousCoordinatorChats)
    workers = try c.decode([ZapperWorker].self, forKey: .workers)
    decisions = try c.decode([ZapperDecision].self, forKey: .decisions)
    issues = try c.decodeIfPresent([ZapperIssue].self, forKey: .issues) ?? []
  }

  /// Decodes a `WatchZappers` snapshot. A Zapper this client cannot read is skipped instead
  /// of hiding every other one.
  public static func snapshot(_ value: JSONValue) -> [Zapper] {
    value.array.compactMap { try? $0.decode(Zapper.self) }
  }
}

public struct ZapperFile: Codable, Sendable, Hashable, Identifiable {
  public var path: String
  public var version: Int
  public var content: String
  public var id: String { path + "@" + String(version) }
  public init(path: String, version: Int, content: String) {
    self.path = path
    self.version = version
    self.content = content
  }
}

public struct ZapperEvent: Codable, Sendable, Hashable, Identifiable {
  public var id: String
  public var zapperId: String
  public var workerChatId: String
  /// `settled`, `needsInput`, `failed`, `interrupted`, `deliveryPending`, …
  public var kind: String
  public var summary: String
  public var acknowledged: Bool
  public init(
    id: String, zapperId: String, workerChatId: String, kind: String, summary: String,
    acknowledged: Bool
  ) {
    self.id = id
    self.zapperId = zapperId
    self.workerChatId = workerChatId
    self.kind = kind
    self.summary = summary
    self.acknowledged = acknowledged
  }
  private enum Keys: String, CodingKey {
    case id, zapperId, zotId, workerChatId, kind, summary, acknowledged
  }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: Keys.self)
    id = try c.decode(String.self, forKey: .id)
    // Engines from before the rename send `zotId`.
    zapperId =
      try c.decodeIfPresent(String.self, forKey: .zapperId)
      ?? c.decode(String.self, forKey: .zotId)
    workerChatId = try c.decode(String.self, forKey: .workerChatId)
    kind = try c.decode(String.self, forKey: .kind)
    summary = try c.decode(String.self, forKey: .summary)
    acknowledged = try c.decode(Bool.self, forKey: .acknowledged)
  }
  public func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Keys.self)
    try c.encode(id, forKey: .id)
    try c.encode(zapperId, forKey: .zapperId)
    try c.encode(workerChatId, forKey: .workerChatId)
    try c.encode(kind, forKey: .kind)
    try c.encode(summary, forKey: .summary)
    try c.encode(acknowledged, forKey: .acknowledged)
  }
}

/// `ZapperAction`: internally tagged by `type`, fields camelCase, no content wrapper.
public enum ZapperAction: Sendable, Hashable {
  case create(config: ZapperConfig)
  case configure(config: ZapperConfig)
  case setStatus(ZapperStatus)
  case adoptWorker(ZapperWorker)
  case delegate(issueId: String?, worker: ZapperWorker, model: ZapperModel, base: String)
  case messageWorker(chatId: String, text: String, automatic: Bool)
  case stopWorker(chatId: String)
  case archiveWorker(chatId: String)
  case releaseWorker(chatId: String)
  case decide(ZapperDecision)
  case writeFile(path: String, content: String, expectedVersion: Int)
  case restoreFile(path: String, version: Int, expectedVersion: Int)
  case createIssue(issueId: String, details: ZapperIssueDetails)
  case editIssue(issueId: String, details: ZapperIssueDetails)
  case linkIssueThread(issueId: String, chatId: String)
  case unlinkIssueThread(issueId: String, chatId: String)
  case linkIssuePullRequest(issueId: String, url: String, title: String)
  case unlinkIssuePullRequest(issueId: String, url: String)
  case commentIssue(issueId: String, text: String, replyTo: String?)
  case askUser(issueId: String, question: String, recommendation: String?)
  case closeIssue(issueId: String, outcome: ZapperIssueOutcome, note: String)
  case freshConversation
  case delete

  /// The wire `type`.
  public var type: String {
    switch self {
    case .create: return "create"
    case .configure: return "configure"
    case .setStatus: return "setStatus"
    case .adoptWorker: return "adoptWorker"
    case .delegate: return "delegate"
    case .messageWorker: return "messageWorker"
    case .stopWorker: return "stopWorker"
    case .archiveWorker: return "archiveWorker"
    case .releaseWorker: return "releaseWorker"
    case .decide: return "decide"
    case .writeFile: return "writeFile"
    case .restoreFile: return "restoreFile"
    case .createIssue: return "createIssue"
    case .editIssue: return "editIssue"
    case .linkIssueThread: return "linkIssueThread"
    case .unlinkIssueThread: return "unlinkIssueThread"
    case .linkIssuePullRequest: return "linkIssuePullRequest"
    case .unlinkIssuePullRequest: return "unlinkIssuePullRequest"
    case .commentIssue: return "commentIssue"
    case .askUser: return "askUser"
    case .closeIssue: return "closeIssue"
    case .freshConversation: return "freshConversation"
    case .delete: return "delete"
    }
  }

  public func json() throws -> JSONValue {
    var fields: [String: JSONValue] = [:]
    switch self {
    case .create(let config), .configure(let config): fields["config"] = try .encoding(config)
    case .setStatus(let status): fields["status"] = .string(status.rawValue)
    case .adoptWorker(let worker): fields["worker"] = try .encoding(worker)
    case .delegate(let issueId, let worker, let model, let base):
      if let issueId { fields["issueId"] = .string(issueId) }
      fields["worker"] = try .encoding(worker)
      fields["model"] = try .encoding(model)
      fields["base"] = .string(base)
    case .messageWorker(let chatId, let text, let automatic):
      fields = ["chatId": .string(chatId), "text": .string(text), "automatic": .bool(automatic)]
    case .stopWorker(let chatId), .archiveWorker(let chatId), .releaseWorker(let chatId):
      fields["chatId"] = .string(chatId)
    case .decide(let decision): fields["decision"] = try .encoding(decision)
    case .writeFile(let path, let content, let expected):
      fields = [
        "path": .string(path), "content": .string(content),
        "expectedVersion": .number(Double(expected)),
      ]
    case .restoreFile(let path, let version, let expected):
      fields = [
        "path": .string(path), "version": .number(Double(version)),
        "expectedVersion": .number(Double(expected)),
      ]
    case .createIssue(let issueId, let details), .editIssue(let issueId, let details):
      fields = ["issueId": .string(issueId), "details": try .encoding(details)]
    case .linkIssueThread(let issueId, let chatId), .unlinkIssueThread(let issueId, let chatId):
      fields = ["issueId": .string(issueId), "chatId": .string(chatId)]
    case .linkIssuePullRequest(let issueId, let url, let title):
      fields = ["issueId": .string(issueId), "url": .string(url), "title": .string(title)]
    case .unlinkIssuePullRequest(let issueId, let url):
      fields = ["issueId": .string(issueId), "url": .string(url)]
    case .commentIssue(let issueId, let text, let replyTo):
      fields = ["issueId": .string(issueId), "text": .string(text)]
      if let replyTo { fields["replyTo"] = .string(replyTo) }
    case .askUser(let issueId, let question, let recommendation):
      fields = ["issueId": .string(issueId), "question": .string(question)]
      if let recommendation { fields["recommendation"] = .string(recommendation) }
    case .closeIssue(let issueId, let outcome, let note):
      fields = [
        "issueId": .string(issueId), "outcome": .string(outcome.rawValue), "note": .string(note),
      ]
    case .freshConversation, .delete: break
    }
    fields["type"] = .string(type)
    return .object(fields)
  }
}

extension JSONValue {
  static func encoding<T: Encodable>(_ value: T) throws -> JSONValue {
    try JSONDecoder().decode(JSONValue.self, from: JSONEncoder().encode(value))
  }
}
