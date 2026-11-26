import SwiftUI
import ZuseCore

/// What the live chat list says about a Zapper conversation.
struct ZapperChatInfo: Equatable {
  var title: String?
  var spaceId: String?
  var harness: String?
  var model: String?
  var archived: Bool
  var lastMessagePreview: String?
  var lastMessageAt: Date?
}

/// A command whose outcome is unknown. Retrying resends this exact envelope.
struct UnconfirmedZapperCommand: Identifiable, Equatable {
  let command: ZapperCommand
  let label: String
  var id: String { command.commandId }
}

/// Live Zapper state for the open computer: `WatchZappers` plus the chat and session rows the
/// Zapper screens need, and the command path that mutates it.
@MainActor @Observable final class ZapperStore {
  /// Whether the open computer hosts Zappers (`zappers.v1` in `ControlInfo`).
  private(set) var available = false
  private(set) var loaded = false
  private(set) var zappers: [Zapper] = []
  private(set) var members: Set<String> = []
  private(set) var chats: [String: ZapperChatInfo] = [:]
  private(set) var sessionRows: [String: (status: ZapperRunStatus, updatedAt: Date)] = [:]
  /// Commands in flight, by Zapper.
  private(set) var busy: [String: String] = [:]
  private(set) var unconfirmed: [String: UnconfirmedZapperCommand] = [:]
  /// The latest failure to show, by Zapper.
  var failure: [String: String] = [:]
  private(set) var preview = false
  private var rpc: RPCClient?
  private var tasks: [Task<Void, Never>] = []
  private var previewFiles: [String: [ZapperFile]] = [:]
  private var previewEvents: [String: [ZapperEvent]] = [:]

  func zapper(_ id: String) -> Zapper? { zappers.first { $0.id == id } }

  /// Whether a chat belongs under a Zapper rather than in its project.
  func keepsChat(_ id: String) -> Bool {
    ZapperMembership.keepsChat(id, members: members)
  }

  func runStatus(of chat: String) -> ZapperRunStatus? {
    guard let row = sessionRows[chat],
      Date().timeIntervalSince(row.updatedAt) < ZapperRunStatus.staleAfter
    else { return nil }
    return row.status
  }
  func rank(of worker: ZapperWorker) -> ZapperThreadRank {
    worker.queued ? .queued : ZapperMembership.rank(runStatus(of: worker.chatId))
  }
  func orderedThreads(_ zapper: Zapper) -> [ZapperWorker] {
    zapper.orderedThreads { rank(of: $0) }
  }
  func workingCount(_ zapper: Zapper) -> Int {
    zapper.workers.filter { runStatus(of: $0.chatId) == .working }.count
  }
  /// A thread's title: its chat title once it has one, else its objective.
  func title(of chat: String, fallback: String) -> String {
    let title = chats[chat]?.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return title.isEmpty ? fallback : title
  }
  /// The ordinary chat row used to open a Zapper conversation in the transcript.
  func session(for chat: String, fallbackTitle: String, known: [SessionRow]) -> SessionRow {
    if let session = known.first(where: { $0.id == chat }) { return session }
    let info = chats[chat]
    return SessionRow(
      id: chat, projectId: info?.spaceId ?? CanonicalWorkspace.projectless,
      title: title(of: chat, fallback: fallbackTitle), providerId: info?.harness ?? "",
      model: info?.model ?? "", worktreeId: nil)
  }

  // MARK: Connection

  func attach(_ client: RPCClient) {
    detach()
    rpc = client
    tasks.append(
      Task { [weak self] in
        guard await client.canonical else { return }
        do {
          let info = try await client.call("ControlInfo")
          guard info["capabilities"].array.contains(.string(ZapperProtocol.capability)) else {
            return
          }
        } catch {
          // Engines without `ControlInfo` predate Zappers.
          return
        }
        guard let self, !Task.isCancelled, self.rpc === client else { return }
        self.available = true
        self.watch("WatchZappers", client) { store, value in store.apply(zappers: value) }
        self.watch("WatchChats", client) { store, value in store.apply(chats: value) }
        self.watch("WatchSessions", client) { store, value in store.apply(sessions: value) }
      })
  }

  func detach() {
    for task in tasks { task.cancel() }
    tasks = []
    rpc = nil
    guard !preview else { return }
    available = false
    loaded = false
    zappers = []
    members = []
    chats = [:]
    sessionRows = [:]
    busy = [:]
    failure = [:]
  }

  /// Keeps one subscription alive for as long as this client is attached.
  private func watch(
    _ method: String, _ client: RPCClient,
    apply: @escaping @MainActor (ZapperStore, JSONValue) -> Void
  ) {
    tasks.append(
      Task { [weak self] in
        var backoff = RetryBackoff()
        while !Task.isCancelled {
          let started = ContinuousClock.now
          do {
            for try await value in try await client.stream(method) {
              guard let self, self.rpc === client else { return }
              apply(self, value)
            }
          } catch {}
          guard !Task.isCancelled else { return }
          let healthy = Double(started.duration(to: .now).components.seconds)
          try? await Task.sleep(for: .seconds(backoff.nextDelay(healthyForSeconds: healthy)))
        }
      })
  }

  private func apply(zappers value: JSONValue) {
    zappers = Zapper.snapshot(value)
    members = ZapperMembership.members(zappers)
    loaded = true
  }
  private func apply(chats value: JSONValue) {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let plain = ISO8601DateFormatter()
    var next: [String: ZapperChatInfo] = [:]
    for chat in value.array {
      guard let id = chat["id"].string else { continue }
      let stamp = chat["lastMessageAt"].string
      next[id] = ZapperChatInfo(
        title: chat["title"].string, spaceId: chat["spaceId"].string,
        harness: chat["config"]["harness"].string, model: chat["config"]["model"].string,
        archived: chat["archived"].bool, lastMessagePreview: chat["lastMessagePreview"].string,
        lastMessageAt: stamp.flatMap { formatter.date(from: $0) ?? plain.date(from: $0) })
    }
    if next != chats { chats = next }
  }
  private func apply(sessions value: JSONValue) {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let plain = ISO8601DateFormatter()
    var next: [String: (status: ZapperRunStatus, updatedAt: Date)] = [:]
    for row in value.array {
      guard let chat = row["chatId"].string,
        let status = row["status"].string.flatMap(ZapperRunStatus.init(rawValue:)),
        let stamp = row["updatedAt"].string,
        let updated = formatter.date(from: stamp) ?? plain.date(from: stamp)
      else { continue }
      if let existing = next[chat], existing.updatedAt > updated { continue }
      next[chat] = (status, updated)
    }
    sessionRows = next
  }

  // MARK: Reads

  func files(_ zapperID: String, history: Bool = false) async throws -> [ZapperFile] {
    if preview {
      let files = previewFiles[zapperID] ?? []
      return history ? files : ZapperDrive.latest(files)
    }
    guard let rpc else { throw ClientError("Connect to this computer to read its Drive.") }
    return try await rpc.call(
      "ZapperFiles", ["zapperId": .string(zapperID), "history": .bool(history)]
    ).decode([ZapperFile].self)
  }
  func events(_ zapperID: String) async throws -> [ZapperEvent] {
    if preview { return previewEvents[zapperID] ?? [] }
    guard let rpc else { throw ClientError("Connect to this computer to read reports.") }
    return try await rpc.call("ZapperEvents", ["zapperId": .string(zapperID)])
      .decode([ZapperEvent].self)
  }
  func acknowledge(_ zapperID: String, events: [String]) async {
    if preview {
      previewEvents[zapperID] = previewEvents[zapperID]?.map {
        var event = $0
        if events.contains(event.id) { event.acknowledged = true }
        return event
      }
      return
    }
    guard let rpc, !events.isEmpty else { return }
    do {
      _ = try await rpc.call(
        "AckZapperEvents",
        ["zapperId": .string(zapperID), "eventIds": .array(events.map(JSONValue.string))])
    } catch { failure[zapperID] = error.localizedDescription }
  }

  // MARK: Commands

  /// Runs `action` against the revision on screen. Returns whether it was applied.
  @discardableResult func perform(_ action: ZapperAction, on zapper: Zapper, label: String) async
    -> Bool
  {
    await send(
      ZapperCommand(zapperId: zapper.id, expectedRevision: zapper.revision, action: action),
      label: label)
  }
  /// Resends an unconfirmed command with its original ID.
  @discardableResult func retry(_ pending: UnconfirmedZapperCommand) async -> Bool {
    await send(pending.command, label: pending.label)
  }
  func dismiss(_ pending: UnconfirmedZapperCommand) {
    if unconfirmed[pending.command.zapperId] == pending {
      unconfirmed.removeValue(forKey: pending.command.zapperId)
    }
  }

  /// Creates a Zapper on the open computer. Returns its ID once the engine applied it, or
  /// throws a message to show in the New Zapper sheet. The draft's ID and command are fixed,
  /// so retrying after an unconfirmed answer resends the same envelope.
  func create(_ draft: ZapperDraft) async throws -> String {
    if preview {
      guard let config = draft.config(homeDeviceID: "preview-mac", repositoryDeviceID: "") else {
        throw ClientError("Add a name, project and model first.")
      }
      let zapper = Zapper(
        id: draft.zapperID, revision: 0, config: config, status: .active,
        coordinatorChatId: "zapper-\(draft.zapperID)-0")
      zappers.insert(zapper, at: 0)
      members = ZapperMembership.members(zappers)
      return zapper.id
    }
    // An earlier attempt whose answer was lost may already have landed.
    if zappers.contains(where: { $0.id == draft.zapperID }) { return draft.zapperID }
    guard let rpc else { throw ClientError("Connect to your computer to create a Zapper.") }
    guard available else {
      throw ClientError("Update Zuse on this computer to use Zappers.")
    }
    let home: String
    let repositoryDevice: String
    do {
      home = try await rpc.call("EngineInfo")["deviceId"].string ?? ""
      let spaces = try await rpc.snapshot("WatchSpaces").array
      repositoryDevice =
        spaces.first { $0["id"].string == draft.projectID }?["deviceId"].string ?? home
    } catch {
      throw ClientError("Couldn’t reach your computer. Try again in a moment.")
    }
    guard let command = draft.command(homeDeviceID: home, repositoryDeviceID: repositoryDevice)
    else { throw ClientError("Add a name, project and model first.") }
    busy[draft.zapperID] = "Create"
    defer { busy.removeValue(forKey: draft.zapperID) }
    do {
      let result = try await ZapperCommands.send(command) { [weak self] method, payload in
        guard let rpc = await self?.rpc else {
          throw ClientError("The connection is unavailable.")
        }
        return try await rpc.call(method, payload)
      }
      if let result, !zappers.contains(where: { $0.id == result.id }) {
        zappers.insert(result, at: 0)
        members = ZapperMembership.members(zappers)
      }
      return draft.zapperID
    } catch {
      throw ClientError(error.localizedDescription)
    }
  }

  private func send(_ command: ZapperCommand, label: String) async -> Bool {
    let id = command.zapperId
    guard busy[id] == nil else { return false }
    busy[id] = label
    defer { busy.removeValue(forKey: id) }
    failure.removeValue(forKey: id)
    if preview {
      applyPreview(command.action, to: id)
      unconfirmed.removeValue(forKey: id)
      return true
    }
    do {
      let result = try await ZapperCommands.send(command) { [weak self] method, payload in
        guard let rpc = await self?.rpc else {
          throw ClientError("The connection is unavailable.")
        }
        return try await rpc.call(method, payload)
      }
      if unconfirmed[id]?.command == command { unconfirmed.removeValue(forKey: id) }
      if let result, let index = zappers.firstIndex(where: { $0.id == id }),
        zappers[index].revision <= result.revision
      {
        zappers[index] = result
      } else if result == nil, case .delete = command.action {
        zappers.removeAll { $0.id == id }
      }
      members = ZapperMembership.members(zappers)
      return true
    } catch {
      switch error {
      case .unconfirmed:
        unconfirmed[id] = UnconfirmedZapperCommand(command: command, label: label)
      case .staleRevision, .rejected:
        if unconfirmed[id]?.command == command { unconfirmed.removeValue(forKey: id) }
        failure[id] = error.localizedDescription
      }
      return false
    }
  }

  // MARK: Preview

  #if DEBUG
    func loadPreview(
      zappers: [Zapper], chats: [String: ZapperChatInfo],
      running: [String: ZapperRunStatus], files: [String: [ZapperFile]],
      events: [String: [ZapperEvent]]
    ) {
      preview = true
      available = true
      loaded = true
      self.zappers = zappers
      members = ZapperMembership.members(zappers)
      self.chats = chats
      sessionRows = running.mapValues { ($0, Date.distantFuture) }
      previewFiles = files
      previewEvents = events
    }
  #endif

  /// Offline previews apply the common actions locally so every screen can be exercised.
  private func applyPreview(_ action: ZapperAction, to id: String) {
    guard let index = zappers.firstIndex(where: { $0.id == id }) else { return }
    var zapper = zappers[index]
    switch action {
    case .setStatus(let status): zapper.status = status
    case .decide(let decision):
      if let at = zapper.decisions.firstIndex(where: { $0.id == decision.id }) {
        zapper.decisions[at] = decision
      }
    case .commentIssue(let issueId, let text, let replyTo):
      if let at = zapper.issues.firstIndex(where: { $0.id == issueId }) {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        zapper.issues[at].activity.append(
          ZapperIssueActivity(
            id: UUID().uuidString, createdAt: now, kind: "comment", text: text, replyTo: replyTo))
        if zapper.issues[at].details.status == .needsYou {
          zapper.issues[at].details.owner = .zapper
          zapper.issues[at].details.status =
            zapper.issues[at].threads.isEmpty ? .todo : .inProgress
        }
      }
    case .editIssue(let issueId, let details):
      if let at = zapper.issues.firstIndex(where: { $0.id == issueId }) {
        zapper.issues[at].details = details
      }
    case .releaseWorker(let chatId), .archiveWorker(let chatId):
      zapper.workers.removeAll { $0.chatId == chatId }
    case .writeFile(let path, let content, let expected):
      previewFiles[id, default: []].append(
        ZapperFile(path: path, version: expected + 1, content: content))
    case .restoreFile(let path, let version, let expected):
      if let old = previewFiles[id]?.first(where: { $0.path == path && $0.version == version }) {
        previewFiles[id, default: []].append(
          ZapperFile(path: path, version: expected + 1, content: old.content))
      }
    case .freshConversation:
      zapper.previousCoordinatorChats.append(zapper.coordinatorChatId)
      zapper.coordinatorChatId = "zapper-\(zapper.id)-\(zapper.revision + 1)"
    case .delete:
      zappers.remove(at: index)
      members = ZapperMembership.members(zappers)
      return
    default: break
    }
    zapper.revision += 1
    zappers[index] = zapper
    members = ZapperMembership.members(zappers)
  }
}
