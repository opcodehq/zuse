import SwiftUI
import ZuseCore

/// One open pull request page: the full read, live snapshots from `WatchChangeRequest`, its
/// diff, and the comment and merge actions.
@MainActor @Observable final class PullRequestDetailModel {
  let link: PullRequestLink
  let store: PullRequestStore
  private(set) var detail: PullRequestDetail?
  private(set) var loading = false
  private(set) var error: String?
  /// A live-update failure; the last good snapshot stays on screen.
  private(set) var watchError: String?
  private(set) var files: [PullRequestDiffFile]?
  private(set) var filesError: String?
  private(set) var loadingFiles = false
  private var filesRevision: String?
  private var fullLoad: Task<Void, Never>?

  init(link: PullRequestLink, store: PullRequestStore) {
    self.link = link
    self.store = store
  }

  /// The full read, review threads included.
  func load(refresh: Bool = false) async {
    loading = detail == nil
    defer { loading = false }
    do {
      let fresh = try await store.detail(link, refresh: refresh)
      guard !Task.isCancelled else { return }
      detail = fresh
      error = nil
    } catch is CancellationError {
    } catch {
      if detail == nil { self.error = PullRequestErrorText.readable(error) }
    }
  }

  /// Follows live snapshots for as long as the page is visible, reconnecting with backoff.
  func watch() async {
    var backoff = RetryBackoff()
    while !Task.isCancelled {
      let started = ContinuousClock.now
      do {
        for try await frame in try await store.watch(link) {
          apply(frame)
        }
      } catch is CancellationError {
        return
      } catch {
        watchError = PullRequestErrorText.readable(error)
      }
      guard !Task.isCancelled, !store.preview else { return }
      let healthy = Double(started.duration(to: .now).components.seconds)
      try? await Task.sleep(for: .seconds(backoff.nextDelay(healthyForSeconds: healthy)))
    }
  }

  func apply(_ frame: PullRequestFrame) {
    if frame.reset {
      scheduleFullLoad()
      return
    }
    if let error = frame.error { watchError = PullRequestErrorText.readable(ClientError(error)) }
    guard !frame.refreshing, var snapshot = frame.snapshot else { return }
    watchError = nil
    guard let current = detail else {
      detail = snapshot
      scheduleFullLoad()
      return
    }
    // Snapshots are summary reads: keep the threads from the full read while the head holds.
    let sameHead = snapshot.headRefOid == current.headRefOid
    if snapshot.reviewThreads == nil, sameHead { snapshot.reviewThreads = current.reviewThreads }
    let discussionChanged =
      snapshot.comments.count != current.comments.count
      || snapshot.reviews.count != current.reviews.count
    guard snapshot != current else { return }
    detail = snapshot
    if !sameHead || discussionChanged { scheduleFullLoad() }
  }

  private func scheduleFullLoad() {
    fullLoad?.cancel()
    fullLoad = Task { [weak self] in await self?.load() }
  }

  /// The diff, pinned to the head and base on screen; reloaded when either moves.
  func loadFiles(force: Bool = false) async {
    guard let detail else { return }
    let revision = detail.headRefOid + ".." + detail.baseRefOid
    guard force || filesRevision != revision || files == nil else { return }
    loadingFiles = true
    defer { loadingFiles = false }
    do {
      let parsed = try await store.diff(link, head: detail.headRefOid, base: detail.baseRefOid)
      guard !Task.isCancelled else { return }
      files = parsed
      filesRevision = revision
      filesError = nil
    } catch is CancellationError {
    } catch {
      filesError = PullRequestErrorText.readable(error)
    }
  }

  // MARK: Actions

  /// Posts a comment and shows the one GitHub stored.
  func comment(_ body: String) async throws {
    let comment = try await store.comment(link, body: body)
    guard var current = detail else { return }
    if !current.comments.contains(where: { $0.id == comment.id && !comment.id.isEmpty }) {
      current.comments.append(comment)
    }
    detail = current
  }

  /// Returns GitHub's reason when it declined.
  func update(_ update: PullRequestUpdate) async throws -> String? {
    guard let detail else { throw ClientError("Refresh the pull request first.") }
    let refusal = try await store.update(link, update, head: detail.headRefOid)
    await load(refresh: true)
    return refusal
  }
}
