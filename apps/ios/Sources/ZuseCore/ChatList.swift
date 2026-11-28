import Foundation

/// Which chats the home list shows.
public enum ArchiveFilter: String, CaseIterable, Codable, Sendable {
  case active, archived, all
  public var label: String {
    switch self {
    case .active: return "Active"
    case .archived: return "Archived"
    case .all: return "All chats"
    }
  }
  /// Spoken description of the filter control.
  public var accessibilityLabel: String {
    switch self {
    case .active: return "Showing active chats"
    case .archived: return "Showing archived chats"
    case .all: return "Showing all chats"
    }
  }
  public func shows(archived: Bool) -> Bool {
    switch self {
    case .active: return !archived
    case .archived: return archived
    case .all: return true
    }
  }
}

/// One chat in the home list, from any place a chat can run (a computer or Zuse Cloud).
public struct ChatListRow: Hashable, Identifiable, Sendable {
  public let id: String
  public let chatID: String
  public let placeKey: String
  public let placeLabel: String
  public let projectKey: String
  /// Nil for chats without a project.
  public let projectName: String?
  public let title: String
  public let archived: Bool
  /// When the chat last had a message, when the host says.
  public let lastActivity: Date?
  public init(
    id: String, chatID: String, placeKey: String, placeLabel: String, projectKey: String,
    projectName: String?, title: String, archived: Bool = false, lastActivity: Date? = nil
  ) {
    self.id = id
    self.chatID = chatID
    self.placeKey = placeKey
    self.placeLabel = placeLabel
    self.projectKey = projectKey
    self.projectName = projectName
    self.title = title
    self.archived = archived
    self.lastActivity = lastActivity
  }
  public var isCloud: Bool { placeKey == ChatList.cloudPlace }
}

public struct ChatListGroup: Hashable, Identifiable, Sendable {
  public let id: String
  public let title: String
  /// The computer this group lives on, only when chats from several computers are listed.
  public let place: String?
  public let rows: [ChatListRow]
  public init(id: String, title: String, place: String?, rows: [ChatListRow]) {
    self.id = id
    self.title = title
    self.place = place
    self.rows = rows
  }
}

public enum ChatList {
  public static let noProject = "No project"
  /// The place key for Zuse Cloud chats.
  public static let cloudPlace = "cloud"
  /// The place key for chats on the open computer.
  public static let computerPlace = "computer"

  /// The flat rows for the Chats tab, combining the open computer's chats with Zuse Cloud's.
  ///
  /// Cloud chats are always included, so a computer that is connecting or unreachable can never
  /// hide them. Computer chats are included only when `includesComputerChats` is set, which the
  /// caller clears while a cloud workspace is the active source (its transcript replaces the
  /// computer's). `excludedChatIDs` drops chats owned by a Zapper, and `lastActivity` dates the
  /// computer rows so `recent` can order the whole list.
  public static func homeRows(
    computerLabel: String, projectNames: [String: String] = [:],
    sessions: [SessionRow] = [], archivedSessions: [SessionRow] = [],
    excludedChatIDs: Set<String> = [], lastActivity: [String: Date] = [:],
    includesComputerChats: Bool, cloudChats: [CloudChat]
  ) -> [ChatListRow] {
    var rows: [ChatListRow] = []
    if includesComputerChats {
      for (list, archived) in [(sessions, false), (archivedSessions, true)] {
        for session in list where !excludedChatIDs.contains(session.id) {
          let name =
            session.projectId == CanonicalWorkspace.projectless
            ? nil : projectNames[session.projectId]
          rows.append(
            ChatListRow(
              id: computerPlace + "|" + session.id, chatID: session.id, placeKey: computerPlace,
              placeLabel: computerLabel, projectKey: session.projectId, projectName: name,
              title: session.title.isEmpty ? "New chat" : session.title, archived: archived,
              lastActivity: lastActivity[session.id]))
        }
      }
    }
    for chat in cloudChats {
      rows.append(
        ChatListRow(
          id: cloudPlace + "|" + chat.id, chatID: chat.chatId, placeKey: cloudPlace,
          placeLabel: "Zuse Cloud", projectKey: chat.projectId,
          projectName: chat.repositoryDisplayName,
          title: chat.title.isEmpty ? "Untitled chat" : chat.title, archived: chat.isArchived))
    }
    return rows
  }

  /// One flat list, most recent first: chats with a known last message by time, then the rest in
  /// the order given. `project` keeps chats whose project has that name (ignoring case), so a
  /// cloud chat on the same repository shows with the computer's.
  public static func recent(
    _ rows: [ChatListRow], query: String = "", archive: ArchiveFilter = .active,
    project: String? = nil
  ) -> [ChatListRow] {
    let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
    let wanted = project?.lowercased()
    var seen = Set<String>()
    let visible = rows.enumerated().filter { _, row in
      archive.shows(archived: row.archived)
        && (wanted == nil || (row.projectName ?? noProject).lowercased() == wanted)
        && (needle.isEmpty || row.title.localizedCaseInsensitiveContains(needle)
          || (row.projectName ?? noProject).localizedCaseInsensitiveContains(needle))
        && seen.insert(row.id).inserted
    }
    return visible.sorted { lhs, rhs in
      switch (lhs.element.lastActivity, rhs.element.lastActivity) {
      case (let left?, let right?) where left != right: return left > right
      case (.some, nil): return true
      case (nil, .some): return false
      default: return lhs.offset < rhs.offset
      }
    }.map(\.element)
  }

  /// The project names a list can be narrowed to, in first-seen order, without duplicates.
  public static func projectNames(_ rows: [ChatListRow]) -> [String] {
    var seen = Set<String>()
    return rows.compactMap { row in
      guard let name = row.projectName?.trimmingCharacters(in: .whitespaces), !name.isEmpty,
        seen.insert(name.lowercased()).inserted
      else { return nil }
      return name
    }
  }

  /// Groups chats by project in the order they are given, filtered by place, archive state and a
  /// search query. Cloud chats are ordinary chats: they join the group of a computer project with
  /// the same name (ignoring case) and never make the list look like it spans several computers.
  public static func groups(
    _ rows: [ChatListRow], query: String = "", place: String? = nil,
    archive: ArchiveFilter = .active
  ) -> [ChatListGroup] {
    let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
    var seen = Set<String>()
    let visible = rows.filter { row in
      (place == nil || row.placeKey == place) && archive.shows(archived: row.archived)
        && (needle.isEmpty || row.title.localizedCaseInsensitiveContains(needle)
          || (row.projectName ?? noProject).localizedCaseInsensitiveContains(needle))
        && seen.insert(row.id).inserted
    }
    let computers = visible.filter { !$0.isCloud }
    let severalPlaces = Set(computers.map(\.placeKey)).count > 1
    var groupOf: [String: String] = [:]
    for row in computers {
      let name = (row.projectName ?? noProject).lowercased()
      if groupOf[name] == nil { groupOf[name] = row.placeKey + "|" + row.projectKey }
    }
    func key(_ row: ChatListRow) -> String {
      if !row.isCloud { return row.placeKey + "|" + row.projectKey }
      let name = (row.projectName ?? noProject).lowercased()
      return groupOf[name] ?? cloudPlace + "|" + name
    }
    var order: [String] = []
    var members: [String: [ChatListRow]] = [:]
    for row in visible {
      let id = key(row)
      if members[id] == nil { order.append(id) }
      members[id, default: []].append(row)
    }
    return order.map { id in
      let chats = members[id] ?? []
      let first = chats.first { !$0.isCloud } ?? chats[0]
      let name = first.projectName?.trimmingCharacters(in: .whitespaces) ?? ""
      return ChatListGroup(
        id: id, title: name.isEmpty ? noProject : name,
        place: severalPlaces && !first.isCloud ? first.placeLabel : nil, rows: chats)
    }
  }
}

/// The dithered cloud that marks Zuse Cloud chats, as cells on a 16x16 grid (from
/// `crates/ui/assets/icons/dither-cloud.svg`).
public enum DitherCloud {
  public struct Cell: Hashable, Sendable {
    public let x: Int
    public let y: Int
    public let width: Int
  }
  public static let gridSize = 16
  /// Opacity of the faint cells.
  public static let faintOpacity = 0.22
  public static let solid: [Cell] = cells([
    (6, 3, 2), (9, 3, 2), (5, 4, 7), (5, 5, 1), (7, 5, 1), (9, 5, 1), (11, 5, 1), (4, 6, 1),
    (6, 6, 3), (10, 6, 3), (3, 7, 1), (5, 7, 1), (7, 7, 1), (9, 7, 1), (11, 7, 1), (2, 8, 1),
    (4, 8, 3), (8, 8, 3), (12, 8, 2), (1, 9, 1), (3, 9, 1), (5, 9, 1), (7, 9, 1), (9, 9, 1),
    (11, 9, 1), (13, 9, 1), (2, 10, 1), (4, 10, 1), (6, 10, 1), (8, 10, 1), (10, 10, 1),
    (12, 10, 1), (14, 10, 1), (3, 11, 1), (5, 11, 1), (7, 11, 1), (9, 11, 1), (11, 11, 1),
    (13, 11, 1), (2, 12, 1), (4, 12, 1), (6, 12, 1), (8, 12, 1), (10, 12, 1), (12, 12, 1),
    (5, 13, 1), (7, 13, 1), (9, 13, 1), (11, 13, 1),
  ])
  public static let faint: [Cell] = cells([
    (8, 3, 1), (4, 5, 1), (6, 5, 1), (8, 5, 1), (10, 5, 1), (12, 5, 1), (5, 6, 1), (9, 6, 1),
    (2, 7, 1), (4, 7, 1), (6, 7, 1), (8, 7, 1), (10, 7, 1), (12, 7, 1), (3, 8, 1), (7, 8, 1),
    (11, 8, 1), (2, 9, 1), (4, 9, 1), (6, 9, 1), (8, 9, 1), (10, 9, 1), (12, 9, 1), (14, 9, 1),
    (1, 10, 1), (3, 10, 1), (5, 10, 1), (7, 10, 1), (9, 10, 1), (11, 10, 1), (13, 10, 1),
    (2, 11, 1), (4, 11, 1), (6, 11, 1), (8, 11, 1), (10, 11, 1), (12, 11, 1), (14, 11, 1),
    (3, 12, 1), (5, 12, 1), (7, 12, 1), (9, 12, 1), (11, 12, 1), (13, 12, 1), (6, 13, 1),
    (8, 13, 1), (10, 13, 1), (12, 13, 1),
  ])
  private static func cells(_ values: [(Int, Int, Int)]) -> [Cell] {
    values.map { Cell(x: $0.0, y: $0.1, width: $0.2) }
  }
}
