import SwiftUI
import ZuseCore

/// Every chat's opened panes, saved between launches, plus the live pieces behind them
/// (terminal sessions and file trees) for as long as the app runs.
@MainActor @Observable final class ChatPaneStore {
  private(set) var archive: ChatPaneArchive
  /// Chats whose panes are zoomed out to the overview.
  private(set) var overview: Set<String> = []
  /// Bumped when a card is swiped away, for its haptic.
  private(set) var closedCount = 0
  @ObservationIgnored private var terminals: [String: TerminalSessionModel] = [:]
  @ObservationIgnored private var fileTrees: [String: FileTreeModel] = [:]
  @ObservationIgnored private let defaults: UserDefaults
  @ObservationIgnored private let key = "chat-panes.v1"
  @ObservationIgnored private var seeded = Set<String>()

  init(defaults: UserDefaults = .standard) {
    self.defaults = defaults
    archive = ChatPaneArchive.decode(defaults.data(forKey: key))
    #if DEBUG
      // Screenshots start from the same panes every launch.
      if ProcessInfo.processInfo.arguments.contains("--panes-preview") {
        archive = ChatPaneArchive()
      }
    #endif
  }

  func deck(_ chat: String) -> ChatPaneDeck { archive.deck(chat) }

  func isOverview(_ chat: String) -> Bool { overview.contains(chat) }

  func setOverview(_ chat: String, _ on: Bool) {
    if on { overview.insert(chat) } else { overview.remove(chat) }
  }

  func focus(_ chat: String, _ page: String) {
    update(chat) { $0.focus(page) }
  }

  @discardableResult func open(
    _ chat: String, _ kind: ChatPaneKind, target: String? = nil, title: String
  ) -> ChatPane {
    var opened: ChatPane?
    update(chat) { opened = $0.open(kind, target: target, title: title) }
    return opened ?? ChatPane(kind: kind, target: target, title: title)
  }

  func close(_ chat: String, _ pane: String, swiped: Bool = false) {
    terminals.removeValue(forKey: pane)?.close()
    fileTrees.removeValue(forKey: pane)
    update(chat) { $0.close(pane) }
    if swiped { closedCount += 1 }
  }

  func setTarget(_ chat: String, _ pane: String, target: String?, title: String? = nil) {
    update(chat) { $0.setTarget(target, title: title, for: pane) }
  }

  private func update(_ chat: String, _ change: (inout ChatPaneDeck) -> Void) {
    let before = archive
    archive.update(chat, change)
    guard archive != before else { return }
    #if DEBUG
      if ProcessInfo.processInfo.arguments.contains("--panes-preview") { return }
    #endif
    if let data = try? JSONEncoder().encode(archive) { defaults.set(data, forKey: key) }
  }

  // MARK: Live pane state

  /// The shell behind a terminal pane. It keeps running while you look at other pages and chats.
  func terminal(_ pane: ChatPane, chat: String) -> TerminalSessionModel {
    if let existing = terminals[pane.id] { return existing }
    let model = TerminalSessionModel(chatID: chat, terminalID: pane.target)
    model.onOpened = { [weak self] session in
      self?.setTarget(chat, pane.id, target: session.id, title: TerminalSessionModel.label(session))
    }
    terminals[pane.id] = model
    return model
  }

  func fileTree(_ pane: ChatPane, chat: String) -> FileTreeModel {
    if let existing = fileTrees[pane.id] { return existing }
    let model = FileTreeModel(chatID: chat)
    fileTrees[pane.id] = model
    return model
  }

  /// Terminal IDs already shown in one of this chat's panes.
  func attachedTerminals(_ chat: String) -> Set<String> {
    Set(deck(chat).panes.filter { $0.kind == .terminal }.compactMap(\.target))
  }

  #if DEBUG
    /// `--panes-preview` opens sample panes once per chat, `--pane=<kind>` brings one to the front
    /// (`--pane=selection` the picker) and `--pane-overview` zooms out to the overview.
    func seedPreview(_ chat: String, kinds: [(ChatPaneKind, String?, String)]) {
      let arguments = ProcessInfo.processInfo.arguments
      guard arguments.contains("--panes-preview"), seeded.insert(chat).inserted else { return }
      for (kind, target, title) in kinds { open(chat, kind, target: target, title: title) }
      if let value = arguments.first(where: { $0.hasPrefix("--pane=") })?.dropFirst(7) {
        if value == "selection" {
          focus(chat, ChatPaneDeck.selectionID)
        } else if value == "main" {
          focus(chat, ChatPaneDeck.mainID)
        } else if let kind = ChatPaneKind(rawValue: String(value)),
          let pane = deck(chat).panes.first(where: { $0.kind == kind })
        {
          focus(chat, pane.id)
        }
      } else {
        focus(chat, ChatPaneDeck.mainID)
      }
      if arguments.contains("--pane-overview") { setOverview(chat, true) }
    }
  #endif
}
