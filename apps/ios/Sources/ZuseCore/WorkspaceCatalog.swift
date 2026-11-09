import Foundation

public struct SessionRow: Codable, Identifiable, Hashable, Sendable {
  public let id: String
  public let projectId: String
  public let title: String
  public let providerId: String
  public let model: String
  public let worktreeId: String?
  public init(
    id: String, projectId: String, title: String, providerId: String, model: String,
    worktreeId: String?
  ) {
    self.id = id
    self.projectId = projectId
    self.title = title
    self.providerId = providerId
    self.model = model
    self.worktreeId = worktreeId
  }
}
public struct ProjectRow: Codable, Identifiable, Hashable, Sendable {
  public let id: String
  public let name: String
  public init(id: String, name: String) {
    self.id = id
    self.name = name
  }
}

public enum SessionCatalog {
  public struct Result: Sendable {
    public let rows: [SessionRow]
    public let failedProjectIDs: [String]
  }
  private struct ProjectResult: Sendable {
    let id: String
    let rows: [SessionRow]?
  }
  /// Limit fan-out so a large project catalog cannot overwhelm a reconnecting runtime.
  public static func load(
    _ projectIDs: [String], fetch: @escaping @Sendable (String) async throws -> [SessionRow]
  ) async -> Result {
    await withTaskGroup(of: ProjectResult.self) { group in
      var iterator = projectIDs.makeIterator()
      func add(_ id: String) {
        group.addTask {
          do {
            try Task.checkCancellation()
            let rows = try await fetch(id)
            guard rows.allSatisfy({ $0.projectId == id }) else {
              throw ClientError("The runtime returned chats from another project.")
            }
            var seen = Set<String>()
            return ProjectResult(id: id, rows: rows.filter { seen.insert($0.id).inserted })
          } catch { return ProjectResult(id: id, rows: nil) }
        }
      }
      for _ in 0..<3 { if let id = iterator.next() { add(id) } }
      var results: [String: [SessionRow]] = [:]
      var failures: [String] = []
      for await result in group {
        if Task.isCancelled {
          group.cancelAll()
          break
        }
        if let rows = result.rows { results[result.id] = rows } else { failures.append(result.id) }
        if let next = iterator.next() { add(next) }
      }
      return Result(rows: projectIDs.flatMap { results[$0] ?? [] }, failedProjectIDs: failures)
    }
  }
}
