import Foundation

/// Canonical Rust workspace projections for the native Apple client.
public struct CanonicalWorkspace: Sendable {
  public static let projectless = "__projectless__"
  private let rpc: RPCClient
  public init(_ rpc: RPCClient) { self.rpc = rpc }

  public func projects() async throws -> [ProjectRow] {
    let spaces = try await rpc.snapshot("WatchSpaces").array
    return spaces.compactMap { space in
      guard let id = space["id"].string else { return nil }
      let path = space["path"].string ?? ""
      return ProjectRow(
        id: id, name: space["name"].string ?? URL(fileURLWithPath: path).lastPathComponent)
    } + [ProjectRow(id: Self.projectless, name: "Chats without a project")]
  }

  public func sessions(projectID: String) async throws -> [SessionRow] {
    let chats = try await rpc.snapshot("WatchChats").array
    return chats.compactMap { chat in
      guard !chat["archived"].bool, chat["parentChatId"].string == nil,
        (chat["spaceId"].string ?? Self.projectless) == projectID,
        let id = chat["id"].string
      else { return nil }
      let config = chat["config"]
      return SessionRow(
        id: id, projectId: projectID, title: chat["title"].string ?? "New chat",
        providerId: config["harness"].string ?? "", model: config["model"].string ?? "",
        worktreeId: nil)
    }
  }
  public func session(id: String) async throws -> SessionRow {
    let chats = try await rpc.snapshot("WatchChats").array
    guard let chat = chats.first(where: { $0["id"].string == id }) else {
      throw ClientError("This chat no longer exists.", code: "SessionNotFoundError")
    }
    return SessionRow(
      id: id, projectId: chat["spaceId"].string ?? Self.projectless,
      title: chat["title"].string ?? "New chat", providerId: chat["config"]["harness"].string ?? "",
      model: chat["config"]["model"].string ?? "", worktreeId: nil)
  }
  public func sources(projectID: String) async throws -> ([GitBranch], [JSONValue]) {
    guard projectID != Self.projectless else { return ([], []) }
    let spaces = try await rpc.snapshot("WatchSpaces").array
    guard let space = spaces.first(where: { $0["id"].string == projectID }) else {
      throw ClientError("Project no longer exists")
    }
    let refs = try await rpc.call("ListRefs", ["repoPath": space["path"]]).array
    let branches = try refs.map { ref in
      try JSONValue.object([
        "name": ref["name"], "current": ref["current"], "kind": .string("local"),
      ]).decode(GitBranch.self)
    }
    let worktrees: [JSONValue] = refs.compactMap { ref in
      guard let path = ref["worktreePath"].string else { return nil }
      return .object(["id": .string(path), "path": .string(path), "branch": ref["name"]])
    }
    return (branches, worktrees)
  }

  public func branchCheckout(projectID: String, branch: String, requestID: String) async throws
    -> JSONValue
  {
    let spaces = try await rpc.snapshot("WatchSpaces").array
    guard let space = spaces.first(where: { $0["id"].string == projectID }) else {
      throw ClientError("Project no longer exists")
    }
    let result = try await rpc.call(
      "ResolveCreationSource",
      [
        "repoPath": space["path"], "spaceId": .string(projectID), "requestId": .string(requestID),
        "source": .object(["kind": .string("branch"), "reference": .string(branch)]),
      ])
    let tree = result["outcome"]["worktree"]
    guard let path = tree["path"].string else {
      throw ClientError("The host did not return a checkout path")
    }
    return .object(["id": .string(path), "path": .string(path), "branch": tree["branch"]])
  }

  public func create(_ creation: ChatCreation) async throws -> SessionRow {
    let existing = try await rpc.snapshot("WatchChats").array.first {
      $0["id"].string == creation.chatID
    }
    if let existing {
      guard (existing["spaceId"].string ?? Self.projectless) == creation.projectID else {
        throw ClientError("The saved creation belongs to a different project")
      }
      return SessionRow(
        id: creation.chatID, projectId: creation.projectID,
        title: existing["title"].string ?? "New chat",
        providerId: existing["config"]["harness"].string ?? creation.providerID,
        model: existing["config"]["model"].string ?? creation.model, worktreeId: nil)
    }
    let spaces = try await rpc.snapshot("WatchSpaces").array
    let space = spaces.first { $0["id"].string == creation.projectID }
    guard space != nil || creation.projectID == Self.projectless else {
      throw ClientError("Project no longer exists")
    }
    let host = try await rpc.call("EngineInfo")["deviceId"]
    var cwd = space?["path"] ?? .null
    var branch: JSONValue = .null
    if let existing = creation.worktreeID {
      let (_, trees) = try await sources(projectID: creation.projectID)
      guard let tree = trees.first(where: { $0["id"].string == existing }) else {
        throw ClientError("This worktree is no longer available")
      }
      cwd = tree["path"]
      branch = tree["branch"]
    } else if creation.freshWorktree {
      guard let space else { throw ClientError("Choose a project for an isolated checkout") }
      let result = try await rpc.call(
        "CreateWorktree",
        [
          "repoPath": space["path"], "spaceId": .string(creation.projectID),
          "branch": .string("mobile-" + creation.operationID),
        ])
      guard result["worktree"]["path"].string != nil else {
        throw ClientError("The host did not return a checkout path")
      }
      cwd = result["worktree"]["path"]
      branch = result["worktree"]["branch"]
    }
    _ = try await rpc.call(
      "Mutate",
      [
        "op": .string("createChat"), "chatId": .string(creation.chatID),
        "spaceId": space?["id"] ?? .null, "deviceId": host, "cwd": cwd, "branch": branch,
        "config": .object([
          "harness": .string(creation.providerID), "model": .string(creation.model),
          "reasoning": .null,
          "modelOptions": .object((creation.modelOptions ?? [:]).mapValues(JSONValue.string)),
          "sandbox": .string("workspace-write"),
        ]),
      ])
    let rows = try await sessions(projectID: creation.projectID)
    guard let session = rows.first(where: { $0.id == creation.chatID }) else {
      throw ClientError("The created chat is not visible yet. Refresh before retrying.")
    }
    return session
  }

  public func diff(chatID: String, scope: String) async throws -> JSONValue {
    guard ["staged", "unstaged", "branch"].contains(scope) else {
      throw ClientError("Unknown diff scope")
    }
    let chats = try await rpc.snapshot("WatchChats").array
    guard let chat = chats.first(where: { $0["id"].string == chatID }), let cwd = chat["cwd"].string
    else { throw ClientError("This chat has no checkout.") }
    return try await rpc.call(
      "GetCheckoutDiff", ["chatId": .string(chatID), "cwd": .string(cwd), "mode": .string(scope)])
  }

  public func catalog() async throws -> JSONValue {
    let harnesses = try await rpc.call("ListHarnesses").array.filter {
      $0["installed"] != .bool(false) && $0["enabled"] != .bool(false)
    }
    let providers = try await withThrowingTaskGroup(of: (String, JSONValue).self) { group in
      for harness in harnesses {
        guard let id = harness["id"].string else { continue }
        group.addTask {
          let models = try await rpc.call("ListModels", ["harness": .string(id)])
          return (id, .object(["displayName": harness["name"], "models": models]))
        }
      }
      var values: [String: JSONValue] = [:]
      for try await (id, provider) in group { values[id] = provider }
      return values
    }
    return .object(["providers": .object(providers)])
  }

  public func setModel(chatID: String, model: String) async throws -> SessionRow {
    let chats = try await rpc.snapshot("WatchChats").array
    guard let chat = chats.first(where: { $0["id"].string == chatID }),
      case .object(var config) = chat["config"]
    else { throw ClientError("This chat no longer exists.") }
    config["model"] = .string(model)
    _ = try await rpc.call(
      "Mutate",
      [
        "op": .string("setChatConfig"), "chatId": .string(chatID),
        "expectedConfig": .object(["deviceId": chat["deviceId"], "config": chat["config"]]),
        "config": .object(config),
      ])
    return SessionRow(
      id: chatID, projectId: chat["spaceId"].string ?? Self.projectless,
      title: chat["title"].string ?? "New chat", providerId: config["harness"]?.string ?? "",
      model: model, worktreeId: nil)
  }

  public func addProject(path: String) async throws -> ProjectRow {
    let identity = try await rpc.call("EngineInfo")
    let id = UUID().uuidString
    _ = try await rpc.call(
      "Mutate",
      [
        "op": .string("createSpace"), "spaceId": .string(id), "deviceId": identity["deviceId"],
        "path": .string(path),
      ])
    return ProjectRow(id: id, name: URL(fileURLWithPath: path).lastPathComponent)
  }

  public func send(_ message: PendingMessage) async throws {
    let chats = try await rpc.snapshot("WatchChats").array
    guard let chat = chats.first(where: { $0["id"].string == message.sessionID }),
      let cwd = chat["cwd"].string, let harness = chat["config"]["harness"].string
    else { throw ClientError("The host has not resolved this chat's agent and folder.") }
    let input = message.input ?? .null
    let paths = try input["attachments"].array.map { ref -> String in
      guard let path = ref["path"].string ?? ref["id"].string, path.hasPrefix("/") else {
        throw ClientError("Upload this attachment to the connected host again.")
      }
      return path
    }
    let text = input["text"].string ?? message.text
    let prompt =
      paths.isEmpty
      ? text
      : (text.isEmpty ? "Please review the attached files." : text)
        + "\n\nAttached images (local files — open them to view):\n"
        + paths.map { "- " + $0 }.joined(separator: "\n")
    let config = chat["config"]
    let options: JSONValue =
      if case .object = config["modelOptions"] { config["modelOptions"] } else { .object([:]) }
    _ = try await rpc.call(
      "QueueCommand",
      [
        "chatId": .string(message.sessionID),
        "command": .object([
          "kind": .string("run"), "messageId": .string(message.messageID),
          "request": .object([
            "prompt": .string(prompt), "attachments": .array(paths.map(JSONValue.string)),
            "harness": .string(harness),
            "cwd": .string(cwd), "model": config["model"], "reasoning": config["reasoning"],
            "modelOptions": options,
            "sandbox": .string(config["sandbox"].string ?? "workspace-write"),
            "autoApprove": .bool(false), "resume": chat["harnessSessionId"],
          ]),
        ]),
      ])
  }

  public func answer(
    chatID: String, requestID: String, questions: [JSONValue], answers: [JSONValue]
  ) async throws {
    let encoded = try answers.map { answer -> JSONValue in
      guard let index = answer["questionIndex"].int, questions.indices.contains(index),
        let id = questions[index]["id"].string
      else { throw ClientError("The question changed. Refresh before answering.") }
      let options = questions[index]["options"].array
      var labels = try answer["selected"].array.map { selected -> JSONValue in
        guard let index = selected.int, options.indices.contains(index) else {
          throw ClientError("The answer option changed.")
        }
        return options[index]
      }
      if let other = answer["other"].string, !other.isEmpty { labels.append(.string(other)) }
      return .object(["questionId": .string(id), "labels": .array(labels)])
    }
    _ = try await rpc.call(
      "QueueCommand",
      [
        "chatId": .string(chatID),
        "command": .object([
          "kind": .string("respondInput"), "requestId": .string(requestID),
          "answers": .array(encoded),
        ]),
      ])
  }

  public func stop(chatID: String) async throws {
    _ = try await rpc.call(
      "QueueCommand",
      ["chatId": .string(chatID), "command": .object(["kind": .string("interrupt")])])
  }

}
