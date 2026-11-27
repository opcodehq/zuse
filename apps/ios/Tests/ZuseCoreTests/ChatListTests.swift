import Foundation
import Testing

@testable import ZuseCore

private func row(
  _ id: String, place: String = "computer", project: String = "p", name: String? = "Zuse",
  title: String? = nil, archived: Bool = false
) -> ChatListRow {
  ChatListRow(
    id: place + "|" + id, chatID: id, placeKey: place,
    placeLabel: place == ChatList.cloudPlace ? "Zuse Cloud" : place.capitalized,
    projectKey: project, projectName: name, title: title ?? id, archived: archived)
}

@Test func archiveFilterShowsMatchingChats() {
  let rows = [row("live"), row("old", archived: true)]
  #expect(ChatList.groups(rows).flatMap(\.rows).map(\.chatID) == ["live"])
  #expect(ChatList.groups(rows, archive: .archived).flatMap(\.rows).map(\.chatID) == ["old"])
  #expect(ChatList.groups(rows, archive: .all).flatMap(\.rows).map(\.chatID) == ["live", "old"])
  #expect(ArchiveFilter.allCases.map(\.label) == ["Active", "Archived", "All chats"])
  #expect(ArchiveFilter.archived.accessibilityLabel == "Showing archived chats")
}

@Test func cloudChatsJoinTheirRepositoryGroupIgnoringCase() {
  let rows = [
    row("local", project: "space-1", name: "Zuse"),
    row("other", project: "space-2", name: "Docs"),
    row("cloud", place: ChatList.cloudPlace, project: "repo-9", name: "zuse"),
    row("lonely", place: ChatList.cloudPlace, project: "repo-7", name: "Website"),
  ]
  let groups = ChatList.groups(rows)
  #expect(groups.map(\.title) == ["Zuse", "Docs", "Website"])
  #expect(groups[0].id == "computer|space-1")
  #expect(groups[0].rows.map(\.chatID) == ["local", "cloud"])
  #expect(groups[2].id == "cloud|website")
  #expect(groups.allSatisfy { $0.place == nil })
}

@Test func cloudNeverCountsAsAnotherComputer() {
  let single = ChatList.groups([
    row("a", place: "laptop"), row("b", place: ChatList.cloudPlace, name: "Elsewhere"),
  ])
  #expect(single.allSatisfy { $0.place == nil })
  let several = ChatList.groups([
    row("a", place: "laptop"), row("b", place: "server", project: "q"),
    row("c", place: ChatList.cloudPlace, name: "Cloud only"),
  ])
  #expect(several.map(\.place) == ["Laptop", "Server", nil])
}

@Test func chatsWithoutProjectShareOneGroupAndSearchMatchesProjects() {
  let rows = [
    row("loose", project: CanonicalWorkspace.projectless, name: nil),
    row("cloud-loose", place: ChatList.cloudPlace, project: "x", name: nil),
    row("fix", title: "Fix login", archived: true),
  ]
  let groups = ChatList.groups(rows, archive: .all)
  #expect(groups.first?.title == ChatList.noProject)
  #expect(groups.first?.rows.map(\.chatID) == ["loose", "cloud-loose"])
  #expect(
    ChatList.groups(rows, query: " LOGIN ", archive: .all).flatMap(\.rows).map(\.chatID) == ["fix"])
  #expect(
    ChatList.groups(rows, query: "zuse", archive: .all).flatMap(\.rows).map(\.chatID) == ["fix"])
  #expect(ChatList.groups(rows, query: "zuse").isEmpty)
}

@Test func snapshotPartitionsActiveAndArchivedTopLevelChats() {
  let chats: [JSONValue] = [
    .object([
      "id": .string("a"), "spaceId": .string("s"), "title": .string("Active"),
      "config": .object(["harness": .string("codex"), "model": .string("m")]),
    ]),
    .object(["id": .string("b"), "archived": .bool(true), "title": .string("Old")]),
    .object(["id": .string("c"), "parentChatId": .string("a"), "archived": .bool(true)]),
    .object(["id": .string("d"), "parentChatId": .string("a")]),
    .object(["title": .string("missing id")]),
  ]
  let (active, archived) = CanonicalWorkspace.partition(chats)
  #expect(active.map(\.id) == ["a"])
  #expect(active.first?.projectId == "s" && active.first?.providerId == "codex")
  #expect(archived.map(\.id) == ["b"])
  #expect(archived.first?.projectId == CanonicalWorkspace.projectless)
}

@Test func ditherCloudCellsStayOnTheGrid() {
  let all = DitherCloud.solid + DitherCloud.faint
  #expect(DitherCloud.solid.count == 49 && DitherCloud.faint.count == 48)
  #expect(all.allSatisfy { $0.x >= 0 && $0.y >= 0 && $0.x + $0.width <= DitherCloud.gridSize })
  var covered = Set<[Int]>()
  for cell in all {
    for x in cell.x..<(cell.x + cell.width) { #expect(covered.insert([x, cell.y]).inserted) }
  }
}

@Test func recentListsKnownActivityFirstThenCatalogOrder() {
  let now = Date()
  func dated(_ id: String, _ minutesAgo: Double?, name: String? = "Zuse", archived: Bool = false)
    -> ChatListRow
  {
    ChatListRow(
      id: "computer|" + id, chatID: id, placeKey: "computer", placeLabel: "Computer",
      projectKey: "p", projectName: name, title: id, archived: archived,
      lastActivity: minutesAgo.map { now.addingTimeInterval(-$0 * 60) })
  }
  let rows = [
    dated("unknown-a", nil), dated("old", 600), dated("new", 5),
    dated("unknown-b", nil, name: "Docs"), dated("gone", 1, archived: true),
  ]
  #expect(ChatList.recent(rows).map(\.chatID) == ["new", "old", "unknown-a", "unknown-b"])
  #expect(ChatList.recent(rows, archive: .archived).map(\.chatID) == ["gone"])
  #expect(ChatList.recent(rows, project: "docs").map(\.chatID) == ["unknown-b"])
  #expect(ChatList.recent(rows, query: "unknown").map(\.chatID) == ["unknown-a", "unknown-b"])
  #expect(ChatList.projectNames(rows + [row("cloud", name: "zuse")]) == ["Zuse", "Docs"])
}
