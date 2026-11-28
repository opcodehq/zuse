import Foundation

/// What an opened pane beside a chat shows.
public enum ChatPaneKind: String, Codable, Sendable, CaseIterable {
  /// A Zapper's worker threads.
  case threads
  /// Pull requests linked from a Zapper's issues.
  case pullRequests
  /// A Zapper's issues.
  case issues
  /// Side chats started from this chat.
  case sideThreads
  /// One live shell on the chat's computer, in the chat's folder.
  case terminal
  /// The chat checkout's file tree.
  case files
  /// A Zapper's Drive documents.
  case docs
  /// A Zapper's overview page.
  case overview
  /// The chat checkout's uncommitted changes.
  case changes
  /// One pull request, by URL.
  case pullRequest
  /// One file from the checkout, read-only, by path.
  case file
  /// One Drive document, by path.
  case document

  /// Lists and whole-chat views open once; asking again focuses the open one. Terminals are
  /// always new, and one-item panes are unique per target.
  public var isSingleton: Bool {
    switch self {
    case .terminal, .pullRequest, .file, .document: return false
    default: return true
    }
  }
}

/// One opened pane. `target` names the item a one-item pane shows (a path, a URL or a
/// terminal ID); terminals learn theirs once the computer opens the shell.
public struct ChatPane: Codable, Sendable, Hashable, Identifiable {
  public var id: String
  public var kind: ChatPaneKind
  public var target: String?
  public var title: String
  public init(
    id: String = UUID().uuidString, kind: ChatPaneKind, target: String? = nil, title: String
  ) {
    self.id = id
    self.kind = kind
    self.target = target
    self.title = title
  }
}

/// One page of a chat's pager: the chat itself first, opened panes after it, and the pane
/// picker last. The chat and the picker are always there.
public enum ChatPanePage: Hashable, Sendable, Identifiable {
  case main
  case pane(ChatPane)
  case selection
  public var id: String {
    switch self {
    case .main: return ChatPaneDeck.mainID
    case .pane(let pane): return pane.id
    case .selection: return ChatPaneDeck.selectionID
    }
  }
  public var isClosable: Bool {
    if case .pane = self { return true }
    return false
  }
}

/// The panes opened beside one chat and which page is in front.
public struct ChatPaneDeck: Codable, Sendable, Equatable {
  public static let mainID = "main"
  public static let selectionID = "selection"
  public private(set) var panes: [ChatPane]
  public private(set) var focus: String

  public init(panes: [ChatPane] = [], focus: String = ChatPaneDeck.mainID) {
    self.panes = panes
    self.focus = focus
    if !pageIDs.contains(focus) { self.focus = Self.mainID }
  }

  public var pages: [ChatPanePage] { [.main] + panes.map { .pane($0) } + [.selection] }
  public var pageIDs: [String] { [Self.mainID] + panes.map(\.id) + [Self.selectionID] }
  public var focusIndex: Int { pageIDs.firstIndex(of: focus) ?? 0 }
  public var focusedPane: ChatPane? { panes.first { $0.id == focus } }

  public func pane(_ id: String) -> ChatPane? { panes.first { $0.id == id } }

  /// Brings a page to the front. Unknown pages are ignored.
  public mutating func focus(_ id: String) {
    if pageIDs.contains(id) { focus = id }
  }

  /// Opens a pane after the others and brings it to the front. A list that is already open, or
  /// the same file, document or pull request, comes to the front instead of opening twice.
  @discardableResult public mutating func open(
    _ kind: ChatPaneKind, target: String? = nil, title: String, id: String = UUID().uuidString
  ) -> ChatPane {
    if let existing = panes.first(where: { pane in
      pane.kind == kind && (kind.isSingleton || (target != nil && pane.target == target))
    }) {
      focus = existing.id
      return existing
    }
    let pane = ChatPane(id: id, kind: kind, target: target, title: title)
    panes.append(pane)
    focus = pane.id
    return pane
  }

  /// Closes a pane. The chat and the picker stay. Closing the page in front brings the one
  /// before it forward, so the view doesn't jump past what you were looking at.
  public mutating func close(_ id: String) {
    guard let index = panes.firstIndex(where: { $0.id == id }) else { return }
    panes.remove(at: index)
    if focus == id { focus = index == 0 ? Self.mainID : panes[index - 1].id }
  }

  /// Records what a pane shows once it is known, like the ID of a terminal the computer opened.
  public mutating func setTarget(_ target: String?, title: String? = nil, for id: String) {
    guard let index = panes.firstIndex(where: { $0.id == id }) else { return }
    panes[index].target = target
    if let title { panes[index].title = title }
  }

  /// Closes every pane that no longer passes `keep`, such as panes of a kind this chat can't
  /// show any more.
  public mutating func retain(where keep: (ChatPane) -> Bool) {
    for pane in panes where !keep(pane) { close(pane.id) }
  }
}

/// Every chat's panes, saved between launches. Chats untouched the longest are forgotten past
/// `limit`.
public struct ChatPaneArchive: Codable, Sendable, Equatable {
  public private(set) var decks: [String: ChatPaneDeck]
  /// Chat IDs, least recently changed first.
  public private(set) var order: [String]
  public let limit: Int

  public init(limit: Int = 40) {
    decks = [:]
    order = []
    self.limit = limit
  }

  public func deck(_ chat: String) -> ChatPaneDeck { decks[chat] ?? ChatPaneDeck() }

  public mutating func update(_ chat: String, _ change: (inout ChatPaneDeck) -> Void) {
    var deck = self.deck(chat)
    change(&deck)
    guard deck != decks[chat] else { return }
    decks[chat] = deck
    order.removeAll { $0 == chat }
    order.append(chat)
    while order.count > limit { decks.removeValue(forKey: order.removeFirst()) }
  }

  public static func decode(_ data: Data?) -> ChatPaneArchive {
    guard let data, let archive = try? JSONDecoder().decode(ChatPaneArchive.self, from: data)
    else { return ChatPaneArchive() }
    return archive
  }
}

/// Side chats started from a chat, read from a `WatchChats` snapshot.
public enum ChatSideThreads {
  public static func rows(_ chats: [JSONValue], parent: String) -> [SessionRow] {
    chats.compactMap { chat in
      guard chat["parentChatId"].string == parent, !chat["archived"].bool,
        let id = chat["id"].string
      else { return nil }
      let config = chat["config"]
      return SessionRow(
        id: id, projectId: chat["spaceId"].string ?? CanonicalWorkspace.projectless,
        title: chat["title"].string ?? "Side thread", providerId: config["harness"].string ?? "",
        model: config["model"].string ?? "", worktreeId: nil)
    }
  }
}
