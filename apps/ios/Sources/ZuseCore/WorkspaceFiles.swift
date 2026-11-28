import Foundation

/// One file or folder in a chat's checkout (`ListWorkspaceDirectory`).
public struct WorkspaceFileEntry: Decodable, Hashable, Sendable, Identifiable {
  public enum Kind: String, Decodable, Sendable {
    case file, directory, symlink
  }
  /// Workspace-relative, `/`-separated.
  public var path: String
  public var name: String
  public var kind: Kind
  public var size: UInt64?
  public var ignored: Bool
  public var id: String { path }
  public var isDirectory: Bool { kind == .directory }

  public init(
    path: String, name: String? = nil, kind: Kind, size: UInt64? = nil, ignored: Bool = false
  ) {
    self.path = path
    self.name = name ?? String(path.split(separator: "/").last ?? Substring(path))
    self.kind = kind
    self.size = size
    self.ignored = ignored
  }

  enum CodingKeys: String, CodingKey { case path, name, kind, size, ignored }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    path = try c.decode(String.self, forKey: .path)
    name =
      try c.decodeIfPresent(String.self, forKey: .name)
      ?? String(path.split(separator: "/").last ?? Substring(path))
    kind = (try? c.decode(Kind.self, forKey: .kind)) ?? .file
    size = try? c.decodeIfPresent(UInt64.self, forKey: .size)
    ignored = (try? c.decodeIfPresent(Bool.self, forKey: .ignored)) ?? false
  }
}

/// One page of a folder listing.
public struct WorkspaceDirectoryPage: Decodable, Sendable {
  public var directory: String
  public var entries: [WorkspaceFileEntry]
  public var nextCursor: String?
  public var truncated: Bool
  enum CodingKeys: String, CodingKey { case directory, entries, nextCursor, truncated }
  public init(from decoder: Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    directory = (try? c.decode(String.self, forKey: .directory)) ?? ""
    entries = try c.decode([WorkspaceFileEntry].self, forKey: .entries)
    nextCursor = try? c.decodeIfPresent(String.self, forKey: .nextCursor)
    truncated = (try? c.decode(Bool.self, forKey: .truncated)) ?? false
  }
}

/// Reads a chat's checkout: folder listings and file-name search.
public struct WorkspaceFiles: Sendable {
  let rpc: RPCClient
  let chatID: String
  public init(rpc: RPCClient, chatID: String) {
    self.rpc = rpc
    self.chatID = chatID
  }

  /// A folder's entries, folders first. `""` is the checkout's root.
  public func list(_ directory: String) async throws -> [WorkspaceFileEntry] {
    var entries: [WorkspaceFileEntry] = []
    var cursor: String?
    // A huge folder comes in pages; stop after a few thousand entries.
    for _ in 0..<8 {
      var payload: [String: JSONValue] = [
        "chatId": .string(chatID), "directory": .string(directory),
      ]
      if let cursor { payload["cursor"] = .string(cursor) }
      let page = try await rpc.call("ListWorkspaceDirectory", payload)
        .decode(WorkspaceDirectoryPage.self)
      entries += page.entries
      guard let next = page.nextCursor, !next.isEmpty else { break }
      cursor = next
    }
    return FileTree.sorted(entries)
  }

  /// Files and folders whose path matches `query`, best first.
  public func search(_ query: String, limit: Int = 200) async throws -> [WorkspaceFileEntry] {
    try await rpc.call(
      "SearchWorkspaceFiles",
      ["chatId": .string(chatID), "query": .string(query), "limit": .number(Double(limit))]
    ).decode([WorkspaceFileEntry].self)
  }
}

/// A folder tree flattened into the rows on screen.
public enum FileTree {
  public struct Row: Hashable, Sendable, Identifiable {
    public var entry: WorkspaceFileEntry
    public var depth: Int
    public var id: String { entry.path }
  }

  /// Folders first, then files, each by name ignoring case.
  public static func sorted(_ entries: [WorkspaceFileEntry]) -> [WorkspaceFileEntry] {
    entries.sorted { left, right in
      if left.isDirectory != right.isDirectory { return left.isDirectory }
      return left.name.localizedStandardCompare(right.name) == .orderedAscending
    }
  }

  /// The rows shown for `children` (listings by folder path) with `expanded` folders open.
  public static func rows(
    children: [String: [WorkspaceFileEntry]], expanded: Set<String>, root: String = ""
  ) -> [Row] {
    var rows: [Row] = []
    func add(_ folder: String, depth: Int) {
      for entry in children[folder] ?? [] {
        rows.append(Row(entry: entry, depth: depth))
        if entry.isDirectory && expanded.contains(entry.path) { add(entry.path, depth: depth + 1) }
      }
    }
    add(root, depth: 0)
    return rows
  }
}

/// An open shell on the chat's computer (`OpenTerminal` and `ListTerminals`).
public struct TerminalSessionInfo: Decodable, Hashable, Sendable, Identifiable {
  public var id: String
  public var cwd: String
  public var shell: String
  public init(id: String, cwd: String, shell: String) {
    self.id = id
    self.cwd = cwd
    self.shell = shell
  }
}

/// One `SubscribeTerminal` item.
public enum TerminalStreamEvent: Equatable, Sendable {
  case data(seq: UInt64, bytes: [UInt8])
  case exit(seq: UInt64, code: Int, signal: String?)

  public init?(_ value: JSONValue) {
    guard let seq = value["seq"].int.flatMap({ UInt64(exactly: $0) }) else { return nil }
    switch value["type"].string {
    case "data":
      guard let encoded = value["data"].string, let data = Data(base64Encoded: encoded) else {
        return nil
      }
      self = .data(seq: seq, bytes: Array(data))
    case "exit":
      self = .exit(seq: seq, code: value["exitCode"].int ?? 0, signal: value["signal"].string)
    default:
      return nil
    }
  }

  public var seq: UInt64 {
    switch self {
    case .data(let seq, _), .exit(let seq, _, _): return seq
    }
  }
}

/// The engine's terminal methods for one chat.
public struct TerminalClient: Sendable {
  let rpc: RPCClient
  public init(rpc: RPCClient) { self.rpc = rpc }

  public func list(chatID: String) async throws -> [TerminalSessionInfo] {
    try await rpc.call("ListTerminals", ["chatId": .string(chatID)])
      .decode([TerminalSessionInfo].self)
  }

  /// Opens a shell in the chat's folder.
  public func open(chatID: String, cols: Int, rows: Int) async throws -> TerminalSessionInfo {
    try await rpc.call(
      "OpenTerminal",
      [
        "chatId": .string(chatID), "cols": .number(Double(cols)), "rows": .number(Double(rows)),
      ]
    ).decode(TerminalSessionInfo.self)
  }

  /// Output after `afterSeq`: what was missed first, then live. A quiet shell sends nothing,
  /// so the stream has no first-item deadline.
  public func subscribe(_ terminalID: String, afterSeq: UInt64) async throws
    -> AsyncThrowingStream<JSONValue, Error>
  {
    try await rpc.subscribe(
      "SubscribeTerminal",
      ["terminalId": .string(terminalID), "afterSeq": .number(Double(afterSeq))])
  }

  public func write(_ terminalID: String, bytes: [UInt8]) async throws {
    _ = try await rpc.call(
      "WriteTerminal",
      ["terminalId": .string(terminalID), "data": .string(Data(bytes).base64EncodedString())])
  }

  public func resize(_ terminalID: String, cols: Int, rows: Int) async throws {
    _ = try await rpc.call(
      "ResizeTerminal",
      [
        "terminalId": .string(terminalID), "cols": .number(Double(cols)),
        "rows": .number(Double(rows)),
      ])
  }

  public func close(_ terminalID: String) async throws {
    _ = try await rpc.call("CloseTerminal", ["terminalId": .string(terminalID)])
  }
}
