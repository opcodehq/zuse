#if DEBUG
  import Foundation
  import ZuseCore

  /// Offline samples for `--pr-preview`, `--pr-detail-preview`, `--pr-files-preview`,
  /// `--pr-merge-preview`, `--pr-blocked-preview`, `--pr-checks-preview`, `--pr-commits-preview`
  /// and `--pr-activity-preview`. Shaped like the engine's serde output and decoded with the
  /// same models as live data.
  enum PullRequestPreview {
    static let repository = "northwind/atlas"
    static let docsRepository = "northwind/atlas-docs"
    static let readyURL = "https://github.com/northwind/atlas/pull/370"
    static let blockedURL = "https://github.com/northwind/atlas/pull/366"
    static let draftURL = "https://github.com/northwind/atlas/pull/372"

    /// The pull request each preview argument opens.
    static func entryURL(_ arguments: [String]) -> String? {
      if arguments.contains("--pr-blocked-preview") { return blockedURL }
      if arguments.contains("--pr-draft-preview") { return draftURL }
      let detail = [
        "--pr-detail-preview", "--pr-files-preview", "--pr-merge-preview", "--pr-checks-preview",
        "--pr-commits-preview", "--pr-activity-preview", "--pr-expanded-preview",
        "--pr-confirm-preview",
      ]
      return arguments.contains(where: detail.contains) ? readyURL : nil
    }

    private static func stamp(_ minutesAgo: Double) -> JSONValue {
      let formatter = ISO8601DateFormatter()
      return .string(formatter.string(from: Date().addingTimeInterval(-minutesAgo * 60)))
    }
    private static func oid(_ seed: Int) -> String {
      let hex = Array("0123456789abcdef")
      return String((0..<40).map { hex[($0 * 7 + seed * 13) % 16] })
    }
    private static func actor(_ login: String) -> JSONValue {
      .object(["login": .string(login), "avatarUrl": .null])
    }

    private struct Row {
      let number: Int
      let title: String
      let author: String
      let minutes: Double
      var repository = PullRequestPreview.repository
      var state = "open"
      var draft = false
      var review = "reviewRequired"
      var mergeable = "mergeable"
      var ci = "passed"
      var requested = false
      var additions = 120
      var deletions = 30
    }

    private static let rows: [Row] = [
      Row(
        number: 370, title: "Preview HTML files from the remote computer", author: "maya-chen",
        minutes: 26, review: "approved", additions: 815, deletions: 120),
      Row(
        number: 369, title: "Prepare 0.27.10", author: "maya-chen", minutes: 120,
        review: "approved", additions: 12, deletions: 4),
      Row(
        number: 368, title: "Resume chats after provider usage limits", author: "devon-park",
        minutes: 60 * 26, review: "approved", additions: 240, deletions: 61),
      Row(
        number: 366, title: "Settle lifecycle operations stuck past their deadline",
        author: "maya-chen", minutes: 45, ci: "failed", additions: 310, deletions: 88),
      Row(
        number: 365, title: "Show sidekick agents as subagents in the activity view",
        author: "sam-ortiz", minutes: 180, requested: true, additions: 512, deletions: 140),
      Row(
        number: 364, title: "Keep the new chat draft when switching chats", author: "devon-park",
        minutes: 60 * 5, review: "changesRequested", additions: 64, deletions: 12),
      Row(
        number: 363, title: "Stop retrying startup when the organization is suspended",
        author: "lena-novak", minutes: 60 * 30, mergeable: "conflicting", additions: 90,
        deletions: 33),
      Row(
        number: 361, title: "Add workspace search across projects", author: "sam-ortiz",
        minutes: 60 * 9, ci: "pending", additions: 1204, deletions: 210),
      Row(
        number: 359, title: "Polish chat titles and file tabs", author: "lena-novak",
        minutes: 60 * 50, additions: 88, deletions: 47),
      Row(
        number: 357, title: "Explain blocked merges in the review sheet", author: "maya-chen",
        minutes: 60 * 24 * 9, mergeable: "unknown", ci: "unknown", additions: 140, deletions: 22),
      Row(
        number: 372, title: "Draft: rework the pairing screen layout", author: "maya-chen",
        minutes: 15, draft: true, additions: 402, deletions: 380),
      Row(
        number: 371, title: "WIP: offline queue for comments", author: "devon-park",
        minutes: 60 * 3, draft: true, ci: "pending", additions: 77, deletions: 9),
      Row(
        number: 88, title: "Document the release checklist", author: "release-helper[bot]",
        minutes: 60 * 24 * 14, repository: docsRepository, review: "unknown", additions: 44,
        deletions: 2),
      Row(
        number: 355, title: "Speed up transcript rendering for long chats", author: "sam-ortiz",
        minutes: 60 * 24 * 3, state: "merged", review: "approved", additions: 260,
        deletions: 190),
      Row(
        number: 352, title: "Try a denser sidebar", author: "lena-novak", minutes: 60 * 24 * 6,
        state: "closed", additions: 50, deletions: 50),
    ]

    private static func item(_ row: Row) -> JSONValue {
      .object([
        "provider": .string("github"), "author": actor(row.author),
        "repository": .string(row.repository), "headRefOid": .string(oid(row.number)),
        "ci": .object(["state": .string(row.ci), "totalCount": .number(8)]),
        "viewerDidAuthor": .bool(row.author == "maya-chen"),
        "viewerReviewRequested": .bool(row.requested), "number": .number(Double(row.number)),
        "title": .string(row.title),
        "url": .string("https://github.com/\(row.repository)/pull/\(row.number)"),
        "state": .string(row.state), "isDraft": .bool(row.draft),
        "reviewDecision": .string(row.review), "additions": .number(Double(row.additions)),
        "deletions": .number(Double(row.deletions)), "mergeability": .string(row.mergeable),
        "createdAt": stamp(row.minutes + 60), "updatedAt": stamp(row.minutes),
      ])
    }

    static func pages() -> [PullRequestFilter: [PullRequestListItem]] {
      let items = rows.compactMap { try? item($0).decode(PullRequestListItem.self) }
      let open = items.filter { $0.state == .open }
      return [
        .all: open,
        .authored: open.filter { $0.viewerDidAuthor == true },
        .reviewing: open.filter { $0.viewerReviewRequested == true },
        .merged: items.filter { $0.state == .merged },
        .allStates: items,
      ]
    }

    private static func check(
      _ name: String, conclusion: String, minutes: Double, duration: Double = 3
    ) -> JSONValue {
      .object([
        "databaseId": .number(Double(abs(name.hashValue % 100_000))),
        "startedAt": stamp(minutes + duration),
        "completedAt": conclusion.isEmpty ? .string("") : stamp(minutes),
        "name": .string(name), "context": .string(""),
        "status": .string(conclusion.isEmpty ? "IN_PROGRESS" : "COMPLETED"),
        "conclusion": .string(conclusion), "state": .string(""),
        "detailsUrl": .string("https://github.com/\(repository)/actions/runs/1"),
        "targetUrl": .string(""),
      ])
    }

    private static func comment(
      _ id: String, _ login: String, _ body: String, minutes: Double, state: String = "",
      review: Bool = false, hunk: String = "", replyTo: String? = nil
    ) -> JSONValue {
      .object([
        "id": .string(id), "url": .string(readyURL + "#" + id),
        "replyTo": replyTo.map { .object(["id": .string($0)]) } ?? .null,
        "viewerDidAuthor": .bool(login == "maya-chen"), "author": actor(login),
        "body": .string(body), "state": .string(state),
        "createdAt": review ? .string("") : stamp(minutes),
        "submittedAt": review ? stamp(minutes) : .string(""), "diffHunk": .string(hunk),
      ])
    }

    private static let summary = """
      A remote chat’s agent linked `sandbox-guide/index.html`. Opening it showed \
      `No such file or directory (os error 2)`. A remote computer’s chat names its engine’s own \
      device id, so `system_open_path` assumed the file was on this machine’s disk. The web \
      preview then tried to serve it from the wrong place.

      ## What changed
      - Resolve preview paths against the device that owns the chat.
      - Serve remote HTML, CSS and fonts through the byte-preserving workspace read.
      - Send `text/html; charset=utf-8` for in-memory documents.

      ## Testing
      - New server test checks the `Content-Type` header.
      - Opened a guide on a remote computer and on this machine.
      """

    private static let botBody = """
      <!-- walkthrough-start -->
      > [!NOTE]
      > Automated review summary for this pull request.

      <details><summary>Walkthrough</summary>

      The preview server now asks the owning device for files. Remote reads keep their bytes, \
      so fonts and images load.
      </details>

      | Area | Risk |
      | --- | --- |
      | Preview server | Moderate |
      | File reads | Low |
      """

    private static let threadHunk = """
      @@ -455,6 +455,9 @@ impl FilesSurface {
           let target = self.preview.target();
           let fetch: RemoteFetch = Arc::new(move |path| {
      +        let (reply, answer) = std::sync::mpsc::channel();
      +        requests.unbounded_send((path.to_owned(), reply)).ok()?;
      +        answer.recv_timeout(REMOTE_ASSET_TIMEOUT).ok()
           });
      """

    private static let fontHunk = """
      @@ -509,4 +509,6 @@ async fn read_remote_asset(
               .read_workspace_file(&path)
      +        .await
      +        .ok()?
               .text
               .map(String::into_bytes)
      """

    static func details() -> [String: PullRequestDetail] {
      let ready: JSONValue = .object([
        "title": .string("Preview HTML files from the remote computer"),
        "body": .string(summary), "url": .string(readyURL), "number": .number(370),
        "author": actor("maya-chen"), "baseRefName": .string("main"),
        "baseRefOid": .string(oid(1)), "headRefName": .string("maya-chen/remote-html-preview"),
        "headRefOid": .string(oid(370)),
        "headRepository": .object(["nameWithOwner": .string(repository)]),
        "ci": .object(["state": .string("passed"), "totalCount": .number(8)]),
        "viewerDidAuthor": .bool(true), "viewerReviewRequested": .bool(false),
        "state": .string("OPEN"), "isDraft": .bool(false), "reviewDecision": .string("APPROVED"),
        "mergeable": .string("MERGEABLE"), "additions": .number(815), "deletions": .number(120),
        "comments": .array([
          comment("IC_1", "review-helper[bot]", botBody, minutes: 51),
          comment(
            "IC_2", "maya-chen",
            "Thanks! Fixed in the latest push: remote text responses now send `charset=utf-8`, and the server test checks the header.",
            minutes: 12),
        ]),
        "reviews": .array([
          comment("PRR_1", "sam-ortiz", "", minutes: 44, state: "COMMENTED", review: true),
          comment(
            "PRR_2", "sam-ortiz", "Looks good once the timeout case reports a missing asset.",
            minutes: 20, state: "APPROVED", review: true),
          comment("PRR_3", "devon-park", "", minutes: 8, state: "APPROVED", review: true),
        ]),
        "reviewThreads": .array([
          .object([
            "id": .string("PRRT_1"), "isResolved": .bool(true), "isOutdated": .bool(false),
            "path": .string("crates/ui/src/files/web_preview.rs"), "line": .number(463),
            "startLine": .null, "originalLine": .number(463), "originalStartLine": .null,
            "diffSide": .string("RIGHT"), "startDiffSide": .null,
            "comments": .array([
              comment(
                "PRRC_1", "sam-ortiz",
                "**Report a missing asset when the preview wait times out.**\n\nIf `style.css` takes more than 20 seconds, `recv_timeout` returns an error and the handler sends a 404. If the reply arrives later, the page has already given up.",
                minutes: 43, hunk: threadHunk),
              comment(
                "PRRC_2", "maya-chen",
                "Fixed: a timeout now answers 504 and logs the path, so it reads as slow rather than missing.",
                minutes: 30, replyTo: "PRRC_1"),
            ]),
          ]),
          .object([
            "id": .string("PRRT_2"), "isResolved": .bool(false), "isOutdated": .bool(false),
            "path": .string("crates/ui/src/files/remote_assets.rs"), "line": .number(514),
            "startLine": .number(511), "originalLine": .number(514),
            "originalStartLine": .number(511), "diffSide": .string("RIGHT"),
            "startDiffSide": .string("RIGHT"),
            "comments": .array([
              comment(
                "PRRC_3", "review-helper[bot]",
                "**Read non-image assets as bytes so fonts load.**\n\n`read_remote_asset` serves non-image files from the text field, converted with `String::into_bytes`. A font referenced by a stylesheet then reaches the browser with altered bytes.",
                minutes: 40, hunk: fontHunk)
            ]),
          ]),
        ]),
        "files": .array([]),
        "statusCheckRollup": .array([
          check("build (macos)", conclusion: "SUCCESS", minutes: 30, duration: 9),
          check("build (linux)", conclusion: "SUCCESS", minutes: 31, duration: 7),
          check("build (windows)", conclusion: "SUCCESS", minutes: 29, duration: 12),
          check("test", conclusion: "SUCCESS", minutes: 28, duration: 6),
          check("lint", conclusion: "SUCCESS", minutes: 33, duration: 1),
          check("native app", conclusion: "SUCCESS", minutes: 27, duration: 14),
          check("docs", conclusion: "SKIPPED", minutes: 34, duration: 0),
          check("preview deploy", conclusion: "SUCCESS", minutes: 26, duration: 2),
        ]),
        "commits": .array([
          commit(1, "Resolve preview paths on the owning device", minutes: 70),
          commit(2, "Serve remote assets as bytes", minutes: 47),
          commit(3, "Send charset=utf-8 for in-memory HTML", minutes: 14),
        ]),
        "viewerLogin": .string("maya-chen"),
      ])
      var all: [String: PullRequestDetail] = [:]
      guard let readyDetail = try? ready.decode(PullRequestDetail.self) else { return all }
      all[readyURL] = readyDetail

      var blocked = readyDetail
      blocked.title = "Settle lifecycle operations stuck past their deadline"
      blocked.url = blockedURL
      blocked.number = 366
      blocked.headRefName = "maya-chen/lifecycle-deadline"
      blocked.reviewDecision = .reviewRequired
      blocked.reviews = []
      blocked.reviewThreads = []
      blocked.additions = 310
      blocked.deletions = 88
      blocked.ci = PullRequestCI(state: .failed, totalCount: 8)
      if let failing = try? check("test", conclusion: "FAILURE", minutes: 20, duration: 6)
        .decode(PullRequestCheck.self),
        let running = try? check("native app", conclusion: "", minutes: 2)
          .decode(PullRequestCheck.self)
      {
        blocked.checks =
          Array(readyDetail.checks.filter { !["test", "native app"].contains($0.name) })
          + [failing, running]
      }
      blocked.comments = []
      all[blockedURL] = blocked

      var draft = readyDetail
      draft.title = "Draft: rework the pairing screen layout"
      draft.url = draftURL
      draft.number = 372
      draft.isDraft = true
      draft.reviewDecision = .unknown
      draft.reviews = []
      draft.reviewThreads = []
      draft.comments = []
      all[draftURL] = draft
      return all
    }

    private static func commit(_ index: Int, _ headline: String, minutes: Double) -> JSONValue {
      .object([
        "oid": .string(oid(index * 31)), "messageHeadline": .string(headline),
        "authoredDate": stamp(minutes), "committedDate": stamp(minutes),
        "authors": .array([.object(["login": .string("maya-chen"), "name": .string("Maya Chen")])]),
        "additions": .number(Double(40 * index)), "deletions": .number(Double(9 * index)),
      ])
    }

    static let patch = """
      diff --git a/crates/ui/src/files/open_in.rs b/crates/ui/src/files/open_in.rs
      index 3f1c2aa..9b0d4e1 100644
      --- a/crates/ui/src/files/open_in.rs
      +++ b/crates/ui/src/files/open_in.rs
      @@ -25,3 +25,3 @@ impl FilesSurface {
       impl FilesSurface {
      -    pub(crate) fn open_in(&mut self, path: &Path) -> bool {
      +    pub(crate) fn open_in(&mut self, path: &Path, device: &DeviceId) -> bool {
               self.preview.reset();
      @@ -244,11 +244,11 @@ impl FilesSurface {
           pub(crate) fn fixture_open_file_picker(&mut self) {
               self.preview.open_in.targets = open_in_targets();
               self.preview.open_in.detected = true;
               if let Some(path) = self
                   .preview
                   .active
                   .as_deref()
      -            .and_then(|path| self.system_open_path(path))
      +            .and_then(|path| self.system_open_path(path, &self.device))
               {
                   self.open_file_picker(path, window, cx);
               }
      diff --git a/crates/ui/src/files/web_preview.rs b/crates/ui/src/files/web_preview.rs
      index 81aa0c2..f00ba11 100644
      --- a/crates/ui/src/files/web_preview.rs
      +++ b/crates/ui/src/files/web_preview.rs
      @@ -455,6 +455,11 @@ impl FilesSurface {
           let target = self.preview.target();
           let fetch: RemoteFetch = Arc::new(move |path| {
      -        None
      +        let (reply, answer) = std::sync::mpsc::channel();
      +        requests.unbounded_send((path.to_owned(), reply)).ok()?;
      +        match answer.recv_timeout(REMOTE_ASSET_TIMEOUT) {
      +            Ok(bytes) => bytes,
      +            Err(_) => Some(RemoteAsset::timed_out(path)),
      +        }
           });
           self.preview.serve(target, fetch);
       }
      @@ -1631,9 +1636,12 @@ impl FilesSurface {
                   match result {
                       Ok(WriteWorkspaceFile { hash, .. }) => {
                           let hash = file.hash.clone();
      +                    // The saved text replaces the remote HTML the preview is showing,
      +                    // so a reload reads the new bytes rather than the cached ones.
      +                    let saved_text = self.pending_save.as_ref().map(|save| save.text.clone());
                           if document.file_hash == hash {
      -                        loaded.content = None;
      +                        loaded.content = saved_text;
                           }
                           loaded.size = file.size;
                           loaded.modified = file.modified;
                       }
      diff --git a/crates/ui/src/files/remote_assets.rs b/crates/ui/src/files/remote_assets.rs
      new file mode 100644
      index 0000000..5c2e7d1
      --- /dev/null
      +++ b/crates/ui/src/files/remote_assets.rs
      @@ -0,0 +1,9 @@
      +//! Byte-preserving reads for files the web preview serves from a remote computer.
      +use std::time::Duration;
      +
      +pub(crate) const REMOTE_ASSET_TIMEOUT: Duration = Duration::from_secs(20);
      +
      +/// Text assets keep `charset=utf-8`; everything else is served as read.
      +pub(crate) fn content_type(path: &str) -> &'static str {
      +    mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream")
      +}
      diff --git a/docs/preview.md b/docs/preview.md
      index 1a2b3c4..5d6e7f8 100644
      --- a/docs/preview.md
      +++ b/docs/preview.md
      @@ -12,3 +12,4 @@ Previews open in the built-in browser.
       ## Remote computers
      -Remote previews are not supported yet.
      +Files on a remote computer preview the same way: the computer that owns the chat
      +serves them, fonts and images included.

      diff --git a/assets/preview-icon.png b/assets/preview-icon.png
      index 0a1b2c3..4d5e6f7 100644
      Binary files a/assets/preview-icon.png and b/assets/preview-icon.png differ
      """
  }

  extension PullRequestStore {
    func loadSamplePreview() {
      loadPreview(
        repositories: [
          PullRequestRepository(slug: PullRequestPreview.repository, project: "atlas"),
          PullRequestRepository(slug: PullRequestPreview.docsRepository, project: "atlas-docs"),
        ],
        pages: PullRequestPreview.pages(),
        details: PullRequestPreview.details(),
        diffs: [
          PullRequestPreview.readyURL: PullRequestPreview.patch,
          PullRequestPreview.blockedURL: PullRequestPreview.patch,
          PullRequestPreview.draftURL: PullRequestPreview.patch,
        ])
    }
  }
#endif
