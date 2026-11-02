import Testing

@testable import ZuseCore

@Test func composerWaitsForRestoredSessionAndResetsEveryBusyOperation() throws {
  var activity = SessionActivity()
  #expect(activity.begin(.enqueue) == nil)
  #expect(activity.begin(.upload) == nil)
  activity.restored(generation: activity.generation)
  let previous = try SessionActivity.Operation.allForTesting.map { operation in
    let lease = activity.begin(operation)
    return try #require(lease)
  }
  #expect(activity.begin(.send) == nil)
  activity.restart()
  #expect(!activity.isReady)
  for operation in SessionActivity.Operation.allForTesting {
    #expect(!activity.isActive(operation))
  }
  activity.restored(generation: activity.generation)
  let upload = activity.begin(.upload)
  let currentUpload = try #require(upload)
  for lease in previous { activity.end(lease) }
  #expect(activity.isActive(.upload))
  activity.end(currentUpload)
  #expect(!activity.isActive(.upload))
}

@Test func obsoleteRestorationCannotUnlockOrFailTheSelectedChat() {
  var activity = SessionActivity()
  let previous = activity.generation
  activity.restart()
  activity.restored(generation: previous)
  #expect(!activity.isReady)
  activity.failedRestoration("Old chat failed", generation: previous)
  #expect(activity.restorationError == nil)
  activity.failedRestoration("Cache could not be read", generation: activity.generation)
  #expect(activity.restorationError == "Cache could not be read")
  #expect(activity.begin(.enqueue) == nil)
  activity.restored(generation: activity.generation)
  #expect(activity.isReady)
  #expect(activity.restorationError == nil)
}

@Test func duplicateCompletionCannotEndANewerOperation() throws {
  var activity = SessionActivity()
  activity.restored(generation: activity.generation)
  let firstAttempt = activity.begin(.send)
  let first = try #require(firstAttempt)
  activity.end(first)
  let secondAttempt = activity.begin(.send)
  let second = try #require(secondAttempt)
  activity.end(first)
  #expect(activity.isActive(.send))
  activity.end(second)
  #expect(!activity.isActive(.send))
}

extension SessionActivity.Operation {
  fileprivate static let allForTesting: [Self] = [.send, .enqueue, .upload, .page]
}

@Suite struct RetryBackoffTests {
  @Test func firstRetryIsImmediateAndFlappingRetainsBackoff() {
    var retry = RetryBackoff()
    #expect(retry.nextDelay(jitter: 1) == 0)
    #expect(retry.nextDelay(healthyForSeconds: 0.1, jitter: 1) == 1)
    #expect(retry.nextDelay(healthyForSeconds: 0.1, jitter: 1) == 2)
    for _ in 0..<40 { #expect(retry.nextDelay(jitter: 1.25) <= 30) }
    #expect(retry.nextDelay(healthyForSeconds: 30, jitter: 1) == 0)
    #expect(retry.nextDelay(jitter: 0.75) == 0.75)
  }
}
