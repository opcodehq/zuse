import Foundation
import Testing

@testable import ZuseCore

@Test func chatPagesKeepTheChatFirstAndThePickerLast() {
  var deck = ChatPaneDeck()
  #expect(deck.pageIDs == [ChatPaneDeck.mainID, ChatPaneDeck.selectionID])
  #expect(deck.focus == ChatPaneDeck.mainID)
  let files = deck.open(.files, title: "Files", id: "files")
  let terminal = deck.open(.terminal, title: "Terminal", id: "terminal")
  #expect(deck.pageIDs == [ChatPaneDeck.mainID, files.id, terminal.id, ChatPaneDeck.selectionID])
  #expect(deck.focus == terminal.id)
  #expect(deck.focusIndex == 2)
  #expect(deck.pages.first == .main)
  #expect(deck.pages.last == .selection)
  #expect(!ChatPanePage.main.isClosable && !ChatPanePage.selection.isClosable)
}

@Test func openingAnOpenListFocusesItInsteadOfAddingAnother() {
  var deck = ChatPaneDeck()
  let first = deck.open(.threads, title: "Threads", id: "a")
  deck.open(.files, title: "Files", id: "b")
  let again = deck.open(.threads, title: "Threads", id: "c")
  #expect(again.id == first.id)
  #expect(deck.panes.count == 2)
  #expect(deck.focus == "a")
  // Terminals are always new; one-item panes are unique per target.
  deck.open(.terminal, title: "Terminal", id: "t1")
  deck.open(.terminal, title: "Terminal", id: "t2")
  deck.open(.file, target: "src/main.rs", title: "main.rs", id: "f1")
  let same = deck.open(.file, target: "src/main.rs", title: "main.rs", id: "f2")
  deck.open(.file, target: "README.md", title: "README.md", id: "f3")
  #expect(same.id == "f1")
  #expect(deck.panes.map(\.id) == ["a", "b", "t1", "t2", "f1", "f3"])
}

@Test func closingAPaneFocusesTheOneBeforeIt() {
  var deck = ChatPaneDeck()
  deck.open(.files, title: "Files", id: "a")
  deck.open(.threads, title: "Threads", id: "b")
  deck.open(.issues, title: "Issues", id: "c")
  deck.focus("b")
  deck.close("b")
  #expect(deck.focus == "a")
  deck.close("a")
  #expect(deck.focus == ChatPaneDeck.mainID)
  // Closing a page that isn't in front leaves the front alone; the chat and picker stay.
  deck.focus("c")
  deck.close(ChatPaneDeck.mainID)
  deck.close(ChatPaneDeck.selectionID)
  #expect(deck.pageIDs == [ChatPaneDeck.mainID, "c", ChatPaneDeck.selectionID])
  deck.focus(ChatPaneDeck.selectionID)
  deck.close("c")
  #expect(deck.focus == ChatPaneDeck.selectionID)
  deck.focus("missing")
  #expect(deck.focus == ChatPaneDeck.selectionID)
}

@Test func terminalPanesLearnTheirShell() {
  var deck = ChatPaneDeck()
  deck.open(.terminal, title: "Terminal", id: "t")
  deck.setTarget("term-1", title: "zsh", for: "t")
  #expect(deck.pane("t")?.target == "term-1")
  #expect(deck.pane("t")?.title == "zsh")
  deck.retain { $0.kind != .terminal }
  #expect(deck.panes.isEmpty)
  #expect(deck.focus == ChatPaneDeck.mainID)
}

@Test func paneArchiveRoundTripsAndForgetsTheOldestChats() throws {
  var archive = ChatPaneArchive(limit: 2)
  archive.update("one") { $0.open(.files, title: "Files", id: "f") }
  archive.update("two") { $0.open(.threads, title: "Threads", id: "t") }
  archive.update("one") { $0.focus(ChatPaneDeck.selectionID) }
  archive.update("three") { $0.open(.issues, title: "Issues", id: "i") }
  #expect(archive.decks["two"] == nil)
  #expect(archive.deck("one").focus == ChatPaneDeck.selectionID)
  let decoded = ChatPaneArchive.decode(try JSONEncoder().encode(archive))
  #expect(decoded == archive)
  #expect(ChatPaneArchive.decode(Data("nonsense".utf8)).decks.isEmpty)
  // A saved focus on a page that is gone falls back to the chat.
  let stale = ChatPaneDeck(panes: [], focus: "gone")
  #expect(stale.focus == ChatPaneDeck.mainID)
}

@Test func newThreadInheritsItsParentsFolderBranchAndConfig() {
  let parent: JSONValue = .object([
    "id": .string("parent"), "spaceId": .string("space"), "cwd": .string("/repo/work"),
    "branch": .string("feature"),
    "config": .object([
      "harness": .string("codex"), "model": .string("gpt"),
      "sandbox": .string("workspace-write"),
    ]),
  ])
  let payload = ChatSideThread.createChat(
    parent: parent, chatID: "child", deviceID: .string("device"))
  #expect(payload["op"]?.string == "createChat")
  #expect(payload["chatId"]?.string == "child")
  // The new thread is parented to the chat, in its project, folder and branch.
  #expect(payload["parentChatId"]?.string == "parent")
  #expect(payload["spaceId"]?.string == "space")
  #expect(payload["cwd"]?.string == "/repo/work")
  #expect(payload["branch"]?.string == "feature")
  #expect(payload["deviceId"]?.string == "device")
  // It inherits the parent's agent configuration.
  #expect(payload["config"]?["harness"].string == "codex")
  #expect(payload["config"]?["model"].string == "gpt")
  #expect(payload["config"]?["sandbox"].string == "workspace-write")
  // A stored chat omits `plan`; the engine wants a bool, not null, so it defaults to false.
  #expect(payload["config"]?["plan"] == .bool(false))

  let row = ChatSideThread.row(parent: parent, chatID: "child")
  #expect(row.id == "child")
  #expect(row.projectId == "space")
  #expect(row.providerId == "codex")
  #expect(row.model == "gpt")
}
