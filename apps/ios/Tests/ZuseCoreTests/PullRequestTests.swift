import Foundation
import Testing

@testable import ZuseCore

// Payloads shaped exactly like the engine's serde output (`crates/protocol/src/entities.rs`):
// a `ChangeRequestPage` from `ListChangeRequestPage`, and a `ChangeRequestDetail` from
// `GetChangeRequest`, which re-serializes every field with its default.

private let head = String(repeating: "a", count: 40)
private let base = String(repeating: "b", count: 40)

private let pageJSON = """
  {"items":[
    {"provider":"github","author":{"login":"maya","avatarUrl":"https://avatars.githubusercontent.com/u/1"},
     "repository":"acme/widgets","headRefOid":"\(head)","ci":{"state":"passed","totalCount":8},
     "viewerDidAuthor":true,"viewerReviewRequested":false,"number":370,
     "title":"Preview HTML files from the remote computer","url":"https://github.com/acme/widgets/pull/370",
     "state":"open","isDraft":false,"reviewDecision":"approved","additions":815,"deletions":120,
     "mergeability":"mergeable","createdAt":"2026-11-26T10:00:00Z","updatedAt":"2026-11-26T10:30:12.123456789Z"},
    {"provider":"github","author":{"login":"app/release-bot"},"repository":"acme/widgets",
     "number":12,"title":"Old row","url":"https://github.com/acme/widgets/pull/12","state":"merged",
     "isDraft":false,"additions":1,"deletions":0,"mergeability":"unknown",
     "createdAt":"2026-08-10T09:30:00Z","updatedAt":"2026-08-19T12:00:00Z"}
  ],"nextCursor":"Y3Vyc29yOjUw","totalCount":31,"fetchedAt":"2026-11-26T10:31:00Z"}
  """

private let detailJSON = """
  {"title":"Preview HTML files from the remote computer","body":"Fixes the **preview**.",
   "url":"https://github.com/acme/widgets/pull/370","number":370,
   "author":{"login":"maya","avatarUrl":"https://avatars.githubusercontent.com/u/1"},
   "baseRefName":"main","baseRefOid":"\(base)","headRefName":"maya/html-preview","headRefOid":"\(head)",
   "headRepository":{"nameWithOwner":"acme/widgets"},"ci":{"state":"passed","totalCount":2},
   "viewerDidAuthor":true,"viewerReviewRequested":false,"state":"OPEN","isDraft":false,
   "reviewDecision":"APPROVED","mergeable":"MERGEABLE","additions":815,"deletions":120,
   "comments":[{"id":"IC_1","url":"https://github.com/acme/widgets/pull/370#issuecomment-1","replyTo":null,
     "viewerDidAuthor":false,"author":{"login":"review-helper[bot]","avatarUrl":null},"body":"## Walkthrough",
     "state":"","createdAt":"2026-11-26T10:05:00Z","submittedAt":"","diffHunk":""}],
   "reviews":[
     {"id":"PRR_1","url":"","replyTo":null,"viewerDidAuthor":false,"author":{"login":"sam","avatarUrl":null},
      "body":"","state":"COMMENTED","createdAt":"","submittedAt":"2026-11-26T10:10:00Z","diffHunk":""},
     {"id":"PRR_2","url":"","replyTo":null,"viewerDidAuthor":false,"author":{"login":"sam","avatarUrl":null},
      "body":"Looks good","state":"APPROVED","createdAt":"","submittedAt":"2026-11-26T10:20:00Z","diffHunk":""},
     {"id":"PRR_3","url":"","replyTo":null,"viewerDidAuthor":true,"author":{"login":"maya","avatarUrl":null},
      "body":"","state":"COMMENTED","createdAt":"","submittedAt":"2026-11-26T10:21:00Z","diffHunk":""}],
   "reviewThreads":[{"id":"PRRT_1","isResolved":true,"isOutdated":false,"path":"src/preview.rs","line":463,
     "startLine":null,"originalLine":463,"originalStartLine":null,"diffSide":"RIGHT","startDiffSide":null,
     "comments":[{"id":"PRRC_1","url":"","replyTo":null,"viewerDidAuthor":false,
       "author":{"login":"sam","avatarUrl":null},"body":"Report a missing asset on timeout.","state":"",
       "createdAt":"2026-11-26T10:09:00Z","submittedAt":"",
       "diffHunk":"@@ -458,3 +458,6 @@ impl Preview {\\n     let a = 1;\\n     let b = 2;\\n+    let fetch = 3;\\n+    let reply = 4;\\n+    send();\\n     done();"},
      {"id":"PRRC_2","url":"","replyTo":{"id":"PRRC_1"},"viewerDidAuthor":true,
       "author":{"login":"maya","avatarUrl":null},"body":"Fixed.","state":"","createdAt":"2026-11-26T10:15:00Z",
       "submittedAt":"","diffHunk":""}]}],
   "files":[{"path":"src/preview.rs","additions":800,"deletions":120},{"path":"README.md","additions":15,"deletions":0}],
   "statusCheckRollup":[
     {"databaseId":11,"startedAt":"2026-11-26T10:01:00Z","completedAt":"2026-11-26T10:04:00Z","name":"test",
      "context":"","status":"COMPLETED","conclusion":"SUCCESS","state":"","detailsUrl":"https://github.com/acme/widgets/actions/runs/1",
      "targetUrl":""},
     {"databaseId":null,"startedAt":"","completedAt":"","name":"","context":"deploy/preview","status":"",
      "conclusion":"","state":"SUCCESS","detailsUrl":"","targetUrl":"https://preview.example.com/370"}],
   "commits":[{"oid":"\(head)","messageHeadline":"Serve UTF-8","authoredDate":"2026-11-26T10:12:00Z",
     "committedDate":"2026-11-26T10:12:00Z","authors":[{"login":"maya","name":"Maya"}],"additions":10,"deletions":2}],
   "viewerLogin":"maya"}
  """

private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
  try JSONDecoder().decode(type, from: Data(json.utf8))
}

@Test func decodesTheListingPage() throws {
  let page = try decode(PullRequestPage.self, pageJSON)
  #expect(page.items.count == 2)
  #expect(page.nextCursor == "Y3Vyc29yOjUw" && page.totalCount == 31 && page.fetchedAt != nil)
  let first = page.items[0]
  #expect(first.number == 370 && first.state == .open && first.reviewDecision == .approved)
  #expect(
    first.mergeability == .mergeable && first.ci == PullRequestCI(state: .passed, totalCount: 8))
  #expect(first.author.avatarURL?.host == "avatars.githubusercontent.com")
  // Nanosecond precision from chrono still parses.
  #expect(first.updatedAt == PullRequestDate.parse("2026-11-26T10:30:12Z"))
  // Older engines omit CI, head and viewer fields; missing reviewDecision means unknown.
  let old = page.items[1]
  #expect(old.ci.state == .unknown && old.headRefOid.isEmpty && old.viewerDidAuthor == nil)
  #expect(old.reviewDecision == .unknown && old.state == .merged)
  #expect(old.author.isBot && old.author.displayName == "release-bot[bot]")
}

@Test func decodesTheDetail() throws {
  let detail = try decode(PullRequestDetail.self, detailJSON)
  #expect(detail.state == .open && detail.reviewDecision == .approved)
  #expect(detail.mergeability == .mergeable && detail.headRepository == "acme/widgets")
  #expect(detail.repository == "acme/widgets")
  #expect(detail.comments.first?.author.isBot == true)
  #expect(detail.reviewThreads?.first?.comments.last?.replyTo == "PRRC_1")
  #expect(detail.checks.count == 2 && detail.commits?.first?.shortOid == "aaaaaaa")
  #expect(detail.checks[1].title == "deploy/preview" && detail.checks[1].outcome == .passed)
  #expect(detail.checks[1].link?.absoluteString == "https://preview.example.com/370")
  #expect(detail.autoMergeMethod == nil && detail.viewerLogin == "maya")
  #expect(PullRequestMergeGate.of(detail) == .ready)
  #expect(PullRequestStatusLabel.of(detail) == "Ready to merge")
  #expect(PullRequestChecks.summary(detail) == "2 passed")
}

@Test func summaryReadsLeaveThreadsUnavailable() throws {
  // `summary: true` and watch snapshots skip `reviewThreads` (serialized as null).
  var json = try decode(JSONValue.self, detailJSON)
  if case .object(var fields) = json {
    fields["reviewThreads"] = .null
    fields["autoMergeRequest"] = .object(["mergeMethod": .string("SQUASH")])
    json = .object(fields)
  }
  let frame = PullRequestFrame(
    .object(["snapshot": json, "refreshing": .bool(false), "updatedAt": .number(1)]))
  #expect(frame.snapshot?.reviewThreads == nil)
  #expect(frame.snapshot?.autoMergeMethod == "SQUASH")
  #expect(PullRequestFrame(.object(["reset": .bool(true), "snapshot": .null])).reset)
}

@Test func mergeGateExplainsEveryBlockedState() throws {
  let ready = try decode(PullRequestDetail.self, detailJSON)
  var detail = ready
  detail.isDraft = true
  #expect(PullRequestMergeGate.of(detail) == .draft)
  detail = ready
  detail.mergeability = .conflicting
  #expect(PullRequestMergeGate.of(detail) == .blocked(.conflicts))
  detail = ready
  detail.ci = PullRequestCI(state: .failed, totalCount: 2)
  #expect(PullRequestMergeGate.of(detail) == .blocked(.failingChecks))
  #expect(PullRequestMergeGate.offersAutoMerge(detail))
  detail = ready
  detail.ci = PullRequestCI(state: .pending, totalCount: 2)
  #expect(PullRequestMergeGate.of(detail) == .blocked(.pendingChecks))
  detail = ready
  detail.reviewDecision = .changesRequested
  #expect(PullRequestMergeGate.of(detail) == .blocked(.changesRequested))
  detail = ready
  detail.mergeability = .unknown
  #expect(PullRequestMergeGate.of(detail) == .blocked(.unknownStatus))
  #expect(!PullRequestMergeGate.offersAutoMerge(detail))
  detail = ready
  detail.headRefOid = ""
  #expect(PullRequestMergeGate.of(detail) == .blocked(.unknownStatus))
  detail = ready
  detail.state = .merged
  #expect(PullRequestMergeGate.of(detail) == .merged)
  // Unknown CI with check rows falls back to the rows.
  detail = ready
  detail.ci = PullRequestCI()
  #expect(PullRequestMergeGate.of(detail) == .ready)
  detail.checks = []
  #expect(PullRequestMergeGate.of(detail) == .blocked(.unknownStatus))
}

@Test func sectionsFollowTheBoard() throws {
  let page = try decode(PullRequestPage.self, pageJSON)
  var item = page.items[0]
  #expect(PullRequestSection.of(item) == .ready)
  item.ci.state = .pending
  #expect(PullRequestSection.of(item) == .approved)
  item.reviewDecision = .reviewRequired
  #expect(PullRequestSection.of(item) == .review)
  item.viewerReviewRequested = true
  #expect(PullRequestSection.of(item) == .attention)
  item.isDraft = true
  #expect(PullRequestSection.of(item) == .drafts)
  #expect(PullRequestSection.of(page.items[1]) == .merged)
  let groups = PullRequestSection.group(page.items)
  #expect(groups.map(\.section) == [.ready, .merged])
}

@Test func checkSummariesCountOutcomes() throws {
  var detail = try decode(PullRequestDetail.self, detailJSON)
  var failing = detail.checks[0]
  failing.conclusion = "TIMED_OUT"
  var running = detail.checks[0]
  running.conclusion = ""
  running.status = "IN_PROGRESS"
  detail.checks += [failing, running]
  #expect(PullRequestChecks.summary(detail) == "1 failing, 1 running, 2 passed")
  #expect(PullRequestChecks.rollup(detail.checks).state == .failed)
  #expect(PullRequestChecks.ordered(detail.checks).first?.outcome == .failed)
  #expect(PullRequestChecks.label(.failed, raw: "TIMED_OUT") == "Timed out")
  detail.checks = []
  detail.ci = PullRequestCI(state: .noChecks)
  #expect(PullRequestChecks.summary(detail) == "No checks")
}

@Test func reviewersKeepTheirStrongestLatestVerdict() throws {
  let detail = try decode(PullRequestDetail.self, detailJSON)
  let reviewers = PullRequestReviewer.from(detail)
  // The author's own review is left out; a later comment doesn't erase an approval.
  #expect(reviewers.map(\.actor.login) == ["sam"])
  #expect(reviewers.first?.verdict == .approved)
}

@Test func timelineInterleavesCommentsReviewsThreadsAndCommits() throws {
  let detail = try decode(PullRequestDetail.self, detailJSON)
  let ids = PullRequestActivity.timeline(detail).map(\.id)
  #expect(
    ids == [
      "comment:IC_1", "thread:PRRT_1", "review:PRR_1", "commit:\(head)", "review:PRR_2",
      "review:PRR_3",
    ])
  let snippet = PullRequestHunkSnippet.lines(
    detail.reviewThreads?.first?.comments.first?.diffHunk ?? "")
  #expect(snippet.count == 4)
  #expect(snippet.last?.text == "     done();" && snippet.last?.newLine == 463)
  #expect(snippet.first?.kind == .addition && snippet.first?.newLine == 460)
}

@Test func linksCanonicalizeToTheFormTheEngineAccepts() {
  #expect(
    PullRequestLink("https://github.com/acme/widgets/pull/370/files#diff-1")?.url
      == "https://github.com/acme/widgets/pull/370")
  #expect(PullRequestLink("https://github.com/acme/widgets/pull/370")?.shortLabel == "widgets#370")
  #expect(PullRequestLink("https://github.com/acme/widgets/issues/370") == nil)
  #expect(PullRequestLink("http://github.com/acme/widgets/pull/370") == nil)
  #expect(PullRequestLink("https://evil.example/acme/widgets/pull/370") == nil)
  #expect(PullRequestLink("https://github.com/acme/widgets/pull/0") == nil)
  #expect(PullRequestLink("https://user@github.com/acme/widgets/pull/1") == nil)
  #expect(PullRequestLink.validRepository("acme/widgets"))
  #expect(!PullRequestLink.validRepository("acme/../x") && !PullRequestLink.validRepository("acme"))
}

@Test func readRequestsMatchTheEngineParameters() throws {
  let link = try #require(PullRequestLink("https://github.com/acme/widgets/pull/370"))
  let page = try PullRequestCommands.listPage(
    repository: "acme/widgets", filter: .all, after: "abc", cached: true)
  #expect(page.method == "ListChangeRequestPage")
  #expect(page.payload["filter"] == .string("all") && page.payload["after"] == .string("abc"))
  #expect(page.payload["cached"] == .bool(true) && page.payload["refresh"] == .bool(false))
  #expect(throws: ClientError.self) {
    try PullRequestCommands.listPage(repository: "nope", filter: .all)
  }
  #expect(
    PullRequestFilter.allCases.map(\.rawValue) == [
      "all", "authored", "reviewing", "merged", "allStates",
    ])
  let diff = PullRequestCommands.diff(link, head: head, base: base)
  #expect(diff.method == "GetChangeRequestDiff")
  #expect(
    diff.payload["headRefOid"] == .string(head) && diff.payload["baseRefOid"] == .string(base))
  #expect(PullRequestCommands.diff(link, head: "short", base: base).payload["headRefOid"] == nil)
  #expect(PullRequestCommands.detail(link).method == "GetChangeRequest")
  #expect(PullRequestCommands.watch(link).payload == ["url": .string(link.url)])
}

/// Records writes and replays scripted replies, like the engine would.
private actor FakeEngine {
  var calls: [(String, [String: JSONValue])] = []
  var replies: [Result<JSONValue, ClientError>]
  init(_ replies: [Result<JSONValue, ClientError>]) { self.replies = replies }
  func call(_ method: String, _ payload: [String: JSONValue]) throws -> JSONValue {
    calls.append((method, payload))
    return try replies.removeFirst().get()
  }
}

@Test func mergesArePinnedToTheHeadOnScreen() async throws {
  let link = try #require(PullRequestLink("https://github.com/acme/widgets/pull/370"))
  let request = try PullRequestCommands.update(link, .merge(.squash), head: head)
  #expect(request.method == "UpdateChangeRequestAtHead")
  #expect(
    request.payload == [
      "url": .string(link.url), "headRefOid": .string(head),
      "update": .object(["kind": .string("merge"), "method": .string("squash")]),
    ])
  #expect(throws: ClientError.self) {
    try PullRequestCommands.update(link, .merge(.rebase), head: "")
  }
  let auto = try PullRequestCommands.update(link, .enableAutoMerge(.merge), head: head)
  #expect(auto.method == "UpdateChangeRequestAtHead")
  #expect(auto.payload["update"]?["kind"] == .string("enableAutoMerge"))
  let ready = try PullRequestCommands.update(link, .markReady, head: "")
  #expect(ready.method == "UpdateChangeRequest" && ready.payload["headRefOid"] == nil)
  #expect(ready.payload["update"] == .object(["kind": .string("markReady")]))

  let engine = FakeEngine([
    .success(.object(["refusal": .null])),
    .success(
      .object(["refusal": .string("Merge method squash is not allowed on this repository")])),
  ])
  let done = try await PullRequestCommands.send(request) { try await engine.call($0, $1) }
  #expect(done == nil)
  let refused = try await PullRequestCommands.send(request) { try await engine.call($0, $1) }
  #expect(refused == "Merge method squash is not allowed on this repository")
  #expect(await engine.calls.count == 2)
}

@Test func writesAreNeverRetried() async throws {
  let link = try #require(PullRequestLink("https://github.com/acme/widgets/pull/370"))
  let engine = FakeEngine([.failure(ClientError("The request timed out."))])
  let request = try PullRequestCommands.update(link, .merge(.merge), head: head)
  await #expect(throws: ClientError.self) {
    _ = try await PullRequestCommands.send(request) { try await engine.call($0, $1) }
  }
  #expect(await engine.calls.count == 1)
}

@Test func commentsAreValidatedAndDecoded() async throws {
  let link = try #require(PullRequestLink("https://github.com/acme/widgets/pull/370"))
  #expect(throws: ClientError.self) { try PullRequestCommands.comment(link, body: "  \n") }
  #expect(throws: ClientError.self) {
    try PullRequestCommands.comment(link, body: String(repeating: "x", count: 60_001))
  }
  let request = try PullRequestCommands.comment(link, body: "Ship it")
  #expect(request.method == "PostChangeRequestComment")
  #expect(request.payload == ["url": .string(link.url), "body": .string("Ship it")])
  // The engine's reply to `post_comment`: a `ChangeRequestComment` with defaults filled in.
  let reply = try decode(
    JSONValue.self,
    #"{"id":"IC_9","url":"https://github.com/acme/widgets/pull/370#issuecomment-9","replyTo":null,"viewerDidAuthor":true,"author":{"login":"maya","avatarUrl":"https://avatars.githubusercontent.com/u/1"},"body":"Ship it","state":"","createdAt":"2026-11-26T11:00:00Z","submittedAt":"","diffHunk":""}"#
  )
  let engine = FakeEngine([.success(reply)])
  let comment = try await PullRequestCommands.post(request) { try await engine.call($0, $1) }
  #expect(comment.id == "IC_9" && comment.viewerDidAuthor && comment.body == "Ship it")
}

@Test func capabilityErrorsReadAsSentences() {
  #expect(
    PullRequestErrorText.readable(ClientError("capability error: pull_requests.authentication"))
      == "Run gh auth login on your computer, then refresh.")
  #expect(
    PullRequestErrorText.readable(ClientError("unknown method: ListChangeRequestPage"))
      .hasPrefix("Update Zuse"))
}

@Test func agesAreCompact() {
  let now = Date(timeIntervalSince1970: 10_000_000)
  #expect(PullRequestAge.short(now.addingTimeInterval(-30), now: now) == "now")
  #expect(PullRequestAge.short(now.addingTimeInterval(-26 * 60), now: now) == "26m")
  #expect(PullRequestAge.short(now.addingTimeInterval(-3 * 86_400), now: now) == "3d")
  #expect(PullRequestAge.short(now.addingTimeInterval(-40 * 86_400), now: now) == "1mo")
  #expect(PullRequestAge.ago(now.addingTimeInterval(-26 * 60), now: now) == "26m ago")
}

private let samplePatch = """
  diff --git a/crates/ui/src/open_in.rs b/crates/ui/src/open_in.rs
  index 1111111..2222222 100644
  --- a/crates/ui/src/open_in.rs
  +++ b/crates/ui/src/open_in.rs
  @@ -25,3 +25,3 @@ impl Surface {
       let a = 1;
  -    let b = 2;
  +    let b = 3;
       let c = 4;
  @@ -250,2 +250,3 @@ fn picker()
       first();
  +    second();
       third();
  \\ No newline at end of file
  diff --git a/docs/new.md b/docs/new.md
  new file mode 100644
  index 0000000..3333333
  --- /dev/null
  +++ b/docs/new.md
  @@ -0,0 +1,2 @@
  +# New
  +Text
  diff --git a/old name.txt b/new name.txt
  similarity index 100%
  rename from old name.txt
  rename to new name.txt
  diff --git a/logo.png b/logo.png
  index 4444444..5555555 100644
  Binary files a/logo.png and b/logo.png differ
  diff --git a/gone.rs b/gone.rs
  deleted file mode 100644
  --- a/gone.rs
  +++ /dev/null
  @@ -1,1 +0,0 @@
  -fn gone() {}

  """

@Test func parsesUnifiedDiffIntoFilesAndHunks() {
  let files = PullRequestDiff.parse(samplePatch)
  #expect(
    files.map(\.path) == [
      "crates/ui/src/open_in.rs", "docs/new.md", "new name.txt", "logo.png", "gone.rs",
    ])
  let first = files[0]
  #expect(first.fileName == "open_in.rs" && first.directory == "crates/ui/src")
  #expect(first.change == .modified && first.additions == 2 && first.deletions == 1)
  #expect(first.hunks.count == 2)
  #expect(first.hunks[0].hiddenBefore == 24 && first.hunks[0].context == "impl Surface {")
  // 25..27 shown, next hunk starts at 250: 222 unchanged lines between.
  #expect(first.hunks[1].hiddenBefore == 222)
  #expect(first.hunks[1].lines.map(\.newLine) == [250, 251, 252])
  #expect(first.hunks[0].lines[1].kind == .deletion && first.hunks[0].lines[1].oldLine == 26)
  #expect(first.hunks[0].lines[2].text == "    let b = 3;")
  #expect(
    files[1].change == .added && files[1].additions == 2 && files[1].hunks[0].hiddenBefore == 0)
  #expect(
    files[2].change == .renamed && files[2].oldPath == "old name.txt" && files[2].hunks.isEmpty)
  #expect(files[3].binary)
  #expect(files[4].change == .deleted && files[4].deletions == 1)
}

@Test func unquotesGitPaths() {
  #expect(PullRequestDiff.unquote(#""caf\303\251 menu.txt""#) == "café menu.txt")
  let (a, b) = PullRequestDiff.headerPaths(#""a/caf\303\251.txt" "b/caf\303\251.txt""#)
  #expect(a == "café.txt" && b == "café.txt")
}

@Test func decodesTheCheckoutPullRequest() throws {
  // `CheckoutChangeRequestStatus`, as `WatchCheckoutChangeRequest` streams it.
  let status = try decode(
    JSONValue.self,
    #"{"checkoutId":"c1","deviceId":"d1","cwd":"/repo","branch":"maya/html-preview","changeRequest":{"provider":"github","number":370,"title":"Preview HTML","url":"https://github.com/acme/widgets/pull/370","state":"open","baseRef":"main","headRef":"maya/html-preview","isDraft":true,"reviewDecision":"unknown"},"updatedAt":"2026-11-26T10:00:00Z"}"#
  )
  let summary = try #require(PullRequestCheckoutSummary.from(status: status))
  #expect(summary.number == 370 && summary.state == .open && summary.isDraft)
  #expect(summary.headRef == "maya/html-preview")
  let none = try decode(
    JSONValue.self,
    #"{"checkoutId":"c1","deviceId":"d1","cwd":"/repo","branch":"main","changeRequest":null,"updatedAt":"2026-11-26T10:00:00Z"}"#
  )
  #expect(PullRequestCheckoutSummary.from(status: none) == nil)
}

@Test func htmlInCommentsBecomesMarkdown() {
  let body = """
    <!-- internal state -->
    > [!WARNING]
    > Review limit reached
    <details><summary>Walkthrough</summary>

    The change adds a <a href="https://example.com/x">preview</a> route.<br>
    <img alt="diagram" src="https://example.com/d.png">
    </details>

    ```html
    <div>keep me</div>

    ```
    """
  let clean = PullRequestMarkdown.clean(body)
  #expect(!clean.contains("internal state") && !clean.contains("<details"))
  #expect(clean.contains("> **Warning**"))
  #expect(clean.contains("**Walkthrough**"))
  #expect(clean.contains("[preview](https://example.com/x)") && clean.contains("diagram"))
  #expect(clean.contains("<div>keep me</div>\n\n```"))
  #expect(PullRequestMarkdown.firstLine("## Summary\nMore") == "Summary")
}
