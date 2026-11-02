import Foundation
import Testing

@testable import ZuseCore

private actor CatalogProbe {
  var running = 0
  var peak = 0
  func fetch(_ id: String) async throws -> [SessionRow] {
    running += 1
    peak = max(peak, running)
    defer { running -= 1 }
    try await Task.sleep(for: .milliseconds(2))
    if id == "failed" { throw ClientError("offline") }
    return [
      SessionRow(
        id: id + "-chat", projectId: id, title: id, providerId: "a", model: "m", worktreeId: nil)
    ]
  }
}

@Test func projectCatalogBoundsConcurrencyAndPreservesPartialResults() async {
  let probe = CatalogProbe()
  let projects = (0..<50).map(String.init) + ["failed"]
  let result = await SessionCatalog.load(projects) { try await probe.fetch($0) }
  #expect(result.rows.count == 50)
  #expect(result.rows.map(\.projectId) == Array(projects.dropLast()))
  #expect(result.failedProjectIDs == ["failed"])
  #expect(await probe.peak <= 3)
}

@Test func catalogRejectsCrossProjectRowsAndDeduplicatesIdentities() async {
  let row = SessionRow(
    id: "chat", projectId: "a", title: "Chat", providerId: "agent", model: "model", worktreeId: nil)
  let result = await SessionCatalog.load(["a", "b"]) { _ in [row, row] }
  #expect(result.rows.map(\.id) == ["chat"])
  #expect(result.failedProjectIDs == ["b"])
}
