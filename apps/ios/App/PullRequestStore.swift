import SwiftUI
import ZuseCore

/// A GitHub repository behind one of the open computer's projects.
struct PullRequestRepository: Hashable, Identifiable {
  let slug: String
  let project: String
  var id: String { slug }
}

/// Pull requests for the open computer: which repositories its projects point at, the open
/// count for the home row, and every read and write the pull request screens make. All of it
/// goes through the computer's own `gh`, so nothing here talks to GitHub directly.
@MainActor @Observable final class PullRequestStore {
  /// Whether the open computer can list pull requests (a canonical engine).
  private(set) var available = false
  private(set) var repositories: [PullRequestRepository] = []
  private(set) var resolvingRepositories = false
  /// Open pull requests across every repository, once counted.
  private(set) var openCount: Int?
  private(set) var preview = false
  /// Rows seen in a listing, by URL: the detail read has no opening date of its own.
  private(set) var known: [String: PullRequestListItem] = [:]
  private var rpc: RPCClient?
  private var tasks: [Task<Void, Never>] = []
  private var previewPages: [PullRequestFilter: [PullRequestListItem]] = [:]
  private var previewDetails: [String: PullRequestDetail] = [:]
  private var previewDiffs: [String: String] = [:]
  /// Bumped when a preview write changes a detail, so open pages re-read it.
  private(set) var previewRevision = 0

  // MARK: Connection

  func attach(_ client: RPCClient) {
    detach()
    rpc = client
    tasks.append(
      Task { [weak self] in
        guard await client.canonical else { return }
        guard let self, self.rpc === client else { return }
        self.available = true
        await self.resolveRepositories()
        await self.refreshOpenCount(cached: true)
      })
  }

  func detach() {
    for task in tasks { task.cancel() }
    tasks = []
    rpc = nil
    guard !preview else { return }
    available = false
    repositories = []
    openCount = nil
  }

  private func call(_ request: PullRequestCommands.Request) async throws -> JSONValue {
    guard let rpc else { throw ClientError("Connect to your computer to see pull requests.") }
    return try await rpc.call(request.method, request.payload)
  }

  /// Every local project's GitHub repository, in project order, without duplicates.
  func resolveRepositories() async {
    guard let rpc, !preview else { return }
    resolvingRepositories = true
    defer { resolvingRepositories = false }
    do {
      let identity = try await rpc.call("EngineInfo")
      let spaces = try await rpc.snapshot("WatchSpaces").array.filter {
        identity["deviceId"].string == nil || $0["deviceId"] == identity["deviceId"]
      }
      var found: [PullRequestRepository] = []
      for space in spaces {
        guard let path = space["path"].string, !path.isEmpty else { continue }
        let reply = try? await call(PullRequestCommands.repository(cwd: path))
        guard let slug = reply?.string, PullRequestLink.validRepository(slug),
          !found.contains(where: { $0.slug.lowercased() == slug.lowercased() })
        else { continue }
        found.append(
          PullRequestRepository(
            slug: slug,
            project: space["name"].string ?? URL(fileURLWithPath: path).lastPathComponent))
      }
      guard self.rpc === rpc else { return }
      repositories = found
    } catch {
      // Projects stay unknown; the list explains how to add one.
    }
  }

  func refreshOpenCount(cached: Bool = false) async {
    guard !preview, !repositories.isEmpty else { return }
    var total = 0
    for repository in repositories {
      guard
        let request = try? PullRequestCommands.listPage(
          repository: repository.slug, filter: .all, cached: cached),
        let page = try? await call(request).decode(PullRequestPage.self)
      else { return }
      total += page.totalCount ?? page.items.count
    }
    openCount = total
  }

  // MARK: Reads

  /// One page for one repository.
  func page(
    _ repository: String, filter: PullRequestFilter, after: String? = nil, refresh: Bool = false,
    cached: Bool = false
  ) async throws -> PullRequestPage {
    let page: PullRequestPage
    if preview {
      try await Task.sleep(for: .milliseconds(150))
      let items = (previewPages[filter] ?? []).filter { $0.repository == repository }
      page = PullRequestPage(items: items, totalCount: items.count)
    } else {
      page = try await call(
        PullRequestCommands.listPage(
          repository: repository, filter: filter, after: after, refresh: refresh, cached: cached)
      ).decode(PullRequestPage.self)
    }
    if known.count > 2000 { known.removeAll() }
    for item in page.items { known[item.url] = item }
    return page
  }

  func detail(_ link: PullRequestLink, refresh: Bool = false) async throws -> PullRequestDetail {
    if preview {
      guard let detail = previewDetails[link.url] else {
        throw ClientError("This pull request isn’t in the preview.")
      }
      return detail
    }
    return try await call(PullRequestCommands.detail(link, refresh: refresh))
      .decode(PullRequestDetail.self)
  }

  /// Live snapshots of one pull request. Previews never change on their own.
  func watch(_ link: PullRequestLink) async throws -> AsyncThrowingStream<PullRequestFrame, Error> {
    if preview {
      return AsyncThrowingStream { _ in }
    }
    guard let rpc else { throw ClientError("Connect to your computer to see pull requests.") }
    let request = PullRequestCommands.watch(link)
    let values = try await rpc.stream(request.method, request.payload)
    return AsyncThrowingStream { continuation in
      let task = Task {
        do {
          for try await value in values { continuation.yield(PullRequestFrame(value)) }
          continuation.finish()
        } catch { continuation.finish(throwing: error) }
      }
      continuation.onTermination = { _ in task.cancel() }
    }
  }

  func diff(_ link: PullRequestLink, head: String, base: String) async throws
    -> [PullRequestDiffFile]
  {
    let patch: String
    if preview {
      try await Task.sleep(for: .milliseconds(150))
      patch = previewDiffs[link.url] ?? ""
    } else {
      patch = try await call(PullRequestCommands.diff(link, head: head, base: base)).string ?? ""
    }
    return await Task.detached(priority: .userInitiated) { PullRequestDiff.parse(patch) }.value
  }

  /// The pull request for a chat's checkout and branch, resolved on the chat's computer.
  func checkoutPullRequest(chatID: String) async -> PullRequestCheckoutSummary? {
    guard let rpc, available, !preview else { return nil }
    guard let chats = try? await rpc.snapshot("WatchChats").array,
      let chat = chats.first(where: { $0["id"].string == chatID }),
      let cwd = chat["cwd"].string, !cwd.isEmpty
    else { return nil }
    var payload: [String: JSONValue] = ["cwd": .string(cwd)]
    if let branch = chat["branch"].string, !branch.isEmpty { payload["branch"] = .string(branch) }
    guard let status = try? await rpc.snapshot("WatchCheckoutChangeRequest", payload) else {
      return nil
    }
    return PullRequestCheckoutSummary.from(status: status)
  }

  // MARK: Writes (explicit user actions, sent once)

  func comment(_ link: PullRequestLink, body: String) async throws -> PullRequestComment {
    let request = try PullRequestCommands.comment(link, body: body)
    if preview {
      let comment = PullRequestComment(
        id: UUID().uuidString, viewerDidAuthor: true, author: PullRequestActor(login: "you"),
        body: body, createdAt: ISO8601DateFormatter().string(from: Date()))
      previewDetails[link.url]?.comments.append(comment)
      return comment
    }
    return try await PullRequestCommands.post(request) { [weak self] method, payload in
      guard let rpc = await self?.rpc else { throw ClientError("The connection is unavailable.") }
      return try await rpc.call(method, payload)
    }
  }

  /// Returns GitHub's reason when it declined.
  func update(_ link: PullRequestLink, _ update: PullRequestUpdate, head: String) async throws
    -> String?
  {
    let request = try PullRequestCommands.update(link, update, head: head)
    if preview {
      try await Task.sleep(for: .milliseconds(600))
      applyPreview(update, to: link.url)
      return nil
    }
    return try await PullRequestCommands.send(request) { [weak self] method, payload in
      guard let rpc = await self?.rpc else { throw ClientError("The connection is unavailable.") }
      return try await rpc.call(method, payload)
    }
  }

  // MARK: Preview

  #if DEBUG
    func loadPreview(
      repositories: [PullRequestRepository], pages: [PullRequestFilter: [PullRequestListItem]],
      details: [String: PullRequestDetail], diffs: [String: String]
    ) {
      preview = true
      available = true
      self.repositories = repositories
      previewPages = pages
      previewDetails = details
      previewDiffs = diffs
      openCount = pages[.all]?.count
      for item in pages.values.flatMap({ $0 }) { known[item.url] = item }
    }
  #endif

  private func applyPreview(_ update: PullRequestUpdate, to url: String) {
    guard var detail = previewDetails[url] else { return }
    switch update {
    case .merge: detail.state = .merged
    case .enableAutoMerge(let method): detail.autoMergeMethod = method.rawValue.uppercased()
    case .disableAutoMerge: detail.autoMergeMethod = nil
    case .markReady: detail.isDraft = false
    }
    previewDetails[url] = detail
    previewRevision += 1
  }
}
