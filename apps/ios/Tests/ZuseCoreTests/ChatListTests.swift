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

private func cloudChat(_ id: String, title: String = "", repo: String = "acme/web") -> CloudChat {
  CloudChat(
    workspaceId: "ws-" + id, projectId: "proj-" + id, chatId: id, initialSessionId: "s-" + id,
    activeSessionId: nil, title: title, repositoryDisplayName: repo, state: "ready",
    desiredState: "ready", runtimeState: "running")
}

@Test func previewCloudChatsJSONDecodes() throws {
  let json =
    #"[{"workspaceId":"ws-1","projectId":"proj_web","chatId":"c1","initialSessionId":"s1","activeSessionId":"s1","title":"Ship the pricing page","repositoryDisplayName":"acme/web","state":"ready","desiredState":"ready","runtimeState":"running"},{"workspaceId":"ws-2","projectId":"proj_api","chatId":"c2","initialSessionId":"s2","activeSessionId":null,"title":"Investigate the webhook retries","repositoryDisplayName":"acme/api","state":"paused","desiredState":"ready","runtimeState":"paused"}]"#
  let chats = try JSONDecoder().decode([CloudChat].self, from: Data(json.utf8))
  #expect(chats.count == 2)
  #expect(chats.first?.title == "Ship the pricing page")
  #expect(chats.last?.activeSessionId == nil)
}

@Test func homeRowsKeepCloudChatsWhileAComputerIsConnecting() {
  let cloud = [cloudChat("c1", title: "Fix flake"), cloudChat("c2")]
  // A computer that is connecting: no sessions have arrived yet, but cloud already loaded.
  let connecting = ChatList.homeRows(
    computerLabel: "Studio", includesComputerChats: true, cloudChats: cloud)
  #expect(connecting.allSatisfy { $0.isCloud })
  #expect(connecting.map(\.chatID) == ["c1", "c2"])
  #expect(connecting.first?.title == "Fix flake")
  #expect(connecting.last?.title == "Untitled chat")  // empty titles get a placeholder
  // A cloud workspace is the active source: computer chats are omitted, cloud stays visible.
  let cloudActive = ChatList.homeRows(
    computerLabel: "Studio", sessions: [sessionRow("local")], includesComputerChats: false,
    cloudChats: cloud)
  #expect(cloudActive.map(\.chatID) == ["c1", "c2"])
}

@Test func homeRowsListComputerChatsThenCloudExcludingZapperChats() {
  let rows = ChatList.homeRows(
    computerLabel: "Studio", projectNames: ["p": "Zuse"],
    sessions: [sessionRow("local", project: "p"), sessionRow("zapper", project: "p")],
    archivedSessions: [sessionRow("old", project: "p", title: "Done")],
    excludedChatIDs: ["zapper"], lastActivity: ["local": Date(timeIntervalSince1970: 1000)],
    includesComputerChats: true, cloudChats: [cloudChat("c1")])
  #expect(rows.map(\.chatID) == ["local", "old", "c1"])
  #expect(rows.first?.placeKey == ChatList.computerPlace)
  #expect(rows.first?.projectName == "Zuse")
  #expect(rows.first?.lastActivity == Date(timeIntervalSince1970: 1000))
  #expect(rows[1].archived)
  #expect(rows.last?.isCloud == true)
  // The composition flows into the ordered/flattened list the Chats tab renders.
  #expect(ChatList.recent(rows, archive: .all).map(\.chatID) == ["local", "old", "c1"])
}

private func sessionRow(_ id: String, project: String = "p", title: String = "Chat") -> SessionRow {
  SessionRow(
    id: id, projectId: project, title: title, providerId: "codex", model: "m", worktreeId: nil)
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
