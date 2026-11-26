#if DEBUG
  import Foundation
  import ZuseCore

  extension WorkspaceModel {
    static func previewModelCatalog() throws -> [ModelProvider] {
      let json =
        #"{"providers":{"Preview":{"displayName":"OpenAI · sample","defaultModelId":"sample-balanced","models":[{"id":"sample-balanced","label":"GPT-4.1","optionDescriptors":[{"id":"reasoning","kind":"select","defaultId":"medium","options":[{"id":"low","label":"Low"},{"id":"medium","label":"Medium"},{"id":"high","label":"High"},{"id":"max","label":"Max"}]}]},{"id":"sample-fast","label":"GPT-4.1 mini"},{"id":"sample-reasoning","label":"o3"},{"id":"sample-code","label":"o4-mini"}]},"sample-anthropic":{"displayName":"Anthropic · sample","models":[{"id":"sample-sonnet","label":"Claude Sonnet 4"},{"id":"sample-opus","label":"Claude Opus 4"}]},"sample-google":{"displayName":"Google · sample","models":[{"id":"sample-pro","label":"Gemini 2.5 Pro"},{"id":"sample-flash","label":"Gemini 2.5 Flash"}]}}}"#
      return try ModelProvider.parse(JSONDecoder().decode(JSONValue.self, from: Data(json.utf8)))
    }
    func choosePreviewModel(_ model: String) {
      guard ProcessInfo.processInfo.arguments.contains("--native-preview"),
        let current = selectedSession,
        (try? Self.previewModelCatalog().flatMap(\.models).contains { $0.id == model }) == true
      else { return }
      let updated = SessionRow(
        id: current.id, projectId: current.projectId, title: current.title,
        providerId: (try? Self.previewModelCatalog().first {
          $0.models.contains { $0.id == model }
        }?.id) ?? current.providerId, model: model, worktreeId: current.worktreeId)
      selectedSession = updated
      if let index = sessions.firstIndex(where: { $0.id == current.id }) {
        sessions[index] = updated
      }
    }
    func loadPreview(selected: SessionRow? = nil) throws {
      let session =
        selected
        ?? SessionRow(
          id: "preview", projectId: "preview-project",
          title: ProcessInfo.processInfo.arguments.contains("--conversation-preview")
            ? "Compact composer" : "Native chat preview",
          providerId: "Preview", model: "sample-balanced", worktreeId: nil)
      selectedSession =
        ProcessInfo.processInfo.arguments.contains("--home-preview") && selected == nil
        ? nil : session
      sessions = [session]
      if ProcessInfo.processInfo.arguments.contains("--home-preview") {
        sessions = [
          "Polish the mobile navigation", "Review authentication changes", "Add workspace search",
          "Fix reconnect after sleep", "Simplify the chat composer",
        ].enumerated().map { index, title in
          SessionRow(
            id: "preview-\(index)", projectId: "preview-project", title: title,
            providerId: "Preview", model: "sample-balanced", worktreeId: nil)
        }
        archivedSessions = [
          SessionRow(
            id: "preview-archived", projectId: "preview-project", title: "Try the old sidebar",
            providerId: "Preview", model: "sample-balanced", worktreeId: nil)
        ]
      }
      projects = [ProjectRow(id: "preview-project", name: "Zuse · Preview")]
      if ProcessInfo.processInfo.arguments.contains("--zapper-preview") { loadZapperPreview() }
      selectedProject = projects.first
      let count = ProcessInfo.processInfo.arguments.contains("--large-transcript") ? 10_000 : 30
      var messages: [JSONValue] = (0..<count).map { index in
        let user = index % 3 == 0
        let text =
          user
          ? "Review the changes for task \(index)."
          : """
          ### Task \(index)
          The implementation keeps **stable message identities** while history loads.

          ```swift
          let result = try await client.connect()
          ```

          | Check | Result |
          | --- | --- |
          | Transcript | Ready |

          Next, verify the updated behavior on iPhone and iPad.
          """
        return .object([
          "id": .string("preview-\(index)"), "sessionId": .string(session.id),
          "role": .string(user ? "user" : "assistant"),
          "content": .object(["_tag": .string(user ? "user" : "assistant"), "text": .string(text)]),
          "createdAt": .string("2026-10-18T00:00:00Z"),
        ])
      }
      if ProcessInfo.processInfo.arguments.contains("--conversation-preview")
        || (ProcessInfo.processInfo.arguments.contains("--home-preview") && selected != nil)
      {
        let contents: [JSONValue] = [
          .object([
            "_tag": .string("user"),
            "text": .string("Make the chat composer more compact and check the changes."),
          ]),
          .object([
            "_tag": .string("assistant"),
            "text": .string("I’ll tighten the spacing and keep the actions on one row."),
          ]),
          .object([
            "_tag": .string("thinking"),
            "text": .string(
              "Keep the input flexible while preserving accessible touch targets for attachments and send."
            ),
          ]),
          .object([
            "_tag": .string("tool_use"), "tool": .string("Read"), "itemId": .string("read-preview"),
            "input": .object(["file_path": .string("App/Composer.swift")]),
          ]),
          .object([
            "_tag": .string("tool_result"), "itemId": .string("read-preview"),
            "output": .string(
              "Composer.swift · 84 lines\nInput, attachment menu, model label, and send control."),
            "isError": .bool(false),
          ]),
          .object([
            "_tag": .string("tool_use"), "tool": .string("Edit"), "itemId": .string("edit-preview"),
            "input": .object([
              "file_path": .string("App/Composer.swift"), "old_string": .string("spacing: 16"),
              "new_string": .string("spacing: 8"),
            ]),
          ]),
          .object([
            "_tag": .string("tool_result"), "itemId": .string("edit-preview"),
            "output": .string("Updated spacing in Composer.swift."), "isError": .bool(false),
          ]),
          .object([
            "_tag": .string("tool_use"), "tool": .string("Run"), "itemId": .string("run-preview"),
            "input": .object(["command": .string("swift test")]),
          ]),
          .object([
            "_tag": .string("tool_result"), "itemId": .string("run-preview"),
            "output": .string(
              "Sample test output\n✓ Composer layout\n✓ Attachment actions\n✓ Send and stop states"),
            "isError": .bool(false),
          ]),
          .object([
            "_tag": .string("assistant"),
            "text": .string(
              "Updated the composer with a growing text area and a fixed action row.\n\n- Smaller send button, with a full touch target\n- Compact attachment and model controls\n- Less spacing between tool calls\n\n*This is a sample conversation for previewing the interface.*"
            ),
          ]),
        ]
        messages = contents.enumerated().map { index, content in
          .object([
            "id": .string("conversation-\(index)"), "sessionId": .string(session.id),
            "role": .string(index == 0 ? "user" : "assistant"), "content": content,
            "createdAt": .string("2026-10-18T00:00:00Z"),
          ])
        }
      }
      if ProcessInfo.processInfo.arguments.contains("--visual-preview") {
        messages = [
          .object([
            "id": .string("visual"), "sessionId": .string(session.id), "role": .string("assistant"),
            "content": .object([
              "_tag": .string("ui_spec"), "version": .number(1),
              "spec": .string(
                """
                summary = Stat("Completed tasks", "12", "Native rendering")
                chart = BarChart("Build checks", [{label: "Passed", value: 12}, {label: "Pending", value: 2}], "checks")
                root = Card([summary, chart, Steps([{label: "Review changes", status: "done"}, {label: "Run simulator", status: "active"}])], "Project overview")
                """),
            ]), "createdAt": .string("2026-10-18T00:00:00Z"),
          ])
        ]
      }
      if ProcessInfo.processInfo.arguments.contains("--diagram-preview") {
        messages = [
          .object([
            "id": .string("diagram"), "sessionId": .string(session.id),
            "role": .string("assistant"),
            "content": .object([
              "_tag": .string("assistant"),
              "text": .string(
                """
                ### Native diagram preview
                ```mermaid
                flowchart LR
                  A[Ready] --> B[Complete]
                ```
                """),
            ]), "createdAt": .string("2026-10-18T00:00:00Z"),
          ])
        ]
      }
      try transcript.apply(
        .object([
          "kind": .string("snapshot"), "sessionId": .string(session.id),
          "throughVersion": .number(0),
          "projection": .object(["messages": .array(messages), "status": .string("idle")]),
        ]), sessionID: session.id)
      connectionState = "Preview · offline"
      if ProcessInfo.processInfo.arguments.contains("--model-picker-preview") {
        draft = "Review the changes"
      }
    }

    /// Sample Zappers for `--zapper-preview`: one busy release coordinator and one paused one.
    func loadZapperPreview() {
      let now = Int64(Date().timeIntervalSince1970 * 1000)
      let minute: Int64 = 60_000
      func worker(_ id: String, _ objective: String, queue: Int? = nil) -> ZapperWorker {
        ZapperWorker(
          chatId: "zapper-worker-" + id, deviceId: "preview-mac", spaceId: "preview-project",
          objective: objective, queuePosition: queue)
      }
      func activity(
        _ id: String, _ kind: String, _ text: String, ago: Int64, actor: String? = nil
      ) -> ZapperIssueActivity {
        ZapperIssueActivity(
          id: id, actor: actor, createdAt: now - ago * minute, kind: kind, text: text)
      }
      func config(_ name: String, _ purpose: String, _ mascot: String, _ color: String)
        -> ZapperConfig
      {
        ZapperConfig(
          name: name, purpose: purpose, homeDeviceId: "preview-mac",
          repositories: [ZapperRepository(deviceId: "preview-mac", spaceId: "preview-project")],
          coordinatorModel: ZapperModel(harness: "claude-code", model: "claude-sonnet-4"),
          workerDeviceId: "preview-mac", maxConcurrentWorkers: 3, maxFollowups: 3,
          avatar: ZapperAvatar(mascot: mascot, color: color))
      }
      let release = Zapper(
        id: "release", revision: 42, activityRevision: 9,
        config: config(
          "Release lead",
          "Get Zuse 2.4 out the door: keep main green, land the open fixes and write the release notes.",
          "bolt", "teal"),
        status: .active, coordinatorChatId: "zapper-release-2",
        previousCoordinatorChats: ["zapper-release-0"],
        workers: [
          worker("search", "Speed up workspace search on large repositories"),
          worker("sync", "Fix the flaky sync test on CI"),
          worker("notes", "Draft the 2.4 release notes"),
          worker("onboarding", "Tighten the first-run onboarding copy"),
          worker("icons", "Update the app icons for the new brand", queue: 1),
        ],
        decisions: [
          ZapperDecision(
            id: "ship-date", question: "Ship 2.4 today, or wait for the Windows build to pass?"),
          ZapperDecision(id: "changelog", question: "Use the new changelog format?", answer: "Yes"),
        ],
        issues: [
          ZapperIssue(
            id: "minimum-macos", number: 3,
            details: ZapperIssueDetails(
              title: "Raise the minimum macOS version to 15", status: .done, owner: .zapper),
            createdAt: now - 2_880 * minute, updatedAt: now - 1_440 * minute),
          ZapperIssue(
            id: "search", number: 5,
            details: ZapperIssueDetails(
              title: "Workspace search is slow on large repositories", status: .inReview,
              owner: .thread(chatId: "zapper-worker-search")),
            createdAt: now - 600 * minute, updatedAt: now - 35 * minute,
            threads: ["zapper-worker-search"],
            pullRequests: [
              ZapperIssuePullRequest(
                url: "https://github.com/zuse-dev/zuse/pull/418",
                title: "Index workspace files once")
            ]),
          ZapperIssue(
            id: "flaky-sync", number: 7,
            details: ZapperIssueDetails(
              title: "Sync test flakes on CI",
              description:
                "`sync_resumes_after_restart` fails about one run in five on the Linux runners.\n\nThe failure always follows a file-watcher event that lands before the first snapshot.",
              status: .inProgress, priority: .urgent, assigned: true, labels: ["ci", "sync"],
              owner: .thread(chatId: "zapper-worker-sync"),
              doneCriteria: "Ten consecutive green runs on the Linux runners."),
            createdAt: now - 300 * minute, updatedAt: now - 4 * minute,
            threads: ["zapper-worker-sync"],
            activity: [
              activity("a1", "created", "Created the issue", ago: 300, actor: "zapper-release-2"),
              activity(
                "a2", "thread", "Started thread · In progress", ago: 290, actor: "zapper-release-2"
              ),
              activity(
                "a3", "workerReport",
                "Worker update\nReproduced the flake: the test races the file watcher.", ago: 4,
                actor: "zapper-worker-sync"),
            ]),
          ZapperIssue(
            id: "release-notes", number: 8,
            details: ZapperIssueDetails(
              title: "Release notes for 2.4",
              description:
                "Cover the new Zappers page, faster search and the reconnect fixes. Keep it under 300 words.",
              status: .needsYou, priority: .high, dueDate: "2026-11-28", labels: ["release"],
              owner: .user, doneCriteria: "Notes approved and published on the changelog."),
            createdAt: now - 180 * minute, updatedAt: now - 12 * minute,
            threads: ["zapper-worker-notes"],
            activity: [
              activity("b1", "created", "Created the issue", ago: 180, actor: "zapper-release-2"),
              activity(
                "b2", "question",
                "Should the notes mention the pricing change, or save it for the blog post?\n\nRecommendation: save it for the blog post so the notes stay about the product.",
                ago: 12, actor: "zapper-release-2"),
            ]),
          ZapperIssue(
            id: "icons", number: 9,
            details: ZapperIssueDetails(
              title: "Refresh the app icons for the new brand", status: .inProgress,
              owner: .thread(chatId: "zapper-worker-icons")),
            createdAt: now - 90 * minute, updatedAt: now - 20 * minute,
            threads: ["zapper-worker-icons"]),
          ZapperIssue(
            id: "crash-audit", number: 10,
            details: ZapperIssueDetails(
              title: "Audit crash reports from 2.3", status: .todo, priority: .low, assigned: true,
              owner: .zapper),
            createdAt: now - 60 * minute, updatedAt: now - 60 * minute),
        ])
      let docs = Zapper(
        id: "docs", revision: 7,
        config: config(
          "Docs gardener", "Keep the user guide in step with every release.", "squire", "violet"),
        status: .paused, coordinatorChatId: "zapper-docs-0",
        workers: [worker("guide", "Document the Zappers page")])
      let titles: [String: String] = [
        "zapper-worker-search": "Speed up workspace search",
        "zapper-worker-sync": "Fix flaky sync test",
        "zapper-worker-notes": "Draft 2.4 release notes",
        "zapper-worker-onboarding": "Tighten onboarding copy",
        "zapper-worker-guide": "Document the Zappers page",
        "zapper-release-2": "Release lead", "zapper-release-0": "Release lead · planning",
        "zapper-docs-0": "Docs gardener",
      ]
      var chats: [String: ZapperChatInfo] = [:]
      for (id, title) in titles {
        chats[id] = ZapperChatInfo(
          title: title, spaceId: "preview-project", harness: "Preview", model: "sample-balanced",
          archived: false,
          lastMessagePreview: id == "zapper-release-2"
            ? "Main is green again. Two threads are running and one needs your call on the release notes."
            : nil,
          lastMessageAt: id == "zapper-worker-onboarding" ? nil : Date().addingTimeInterval(-600))
      }
      let notes = """
        # Release notes in progress

        ## Blockers
        - **Windows build** fails at code signing about half the time.
        - The sync test flake is reproduced; a fix is on CI.

        ## Ready
        - Search is 7x faster on the monorepo (PR #418).
        - Reconnect after sleep is fixed.

        | Check | Status |
        | --- | --- |
        | macOS | Green |
        | Linux | Green |
        | Windows | Flaky |
        """
      let files: [ZapperFile] = [
        ZapperFile(path: "INSTRUCTIONS.md", version: 1, content: release.config.purpose),
        ZapperFile(
          path: "INSTRUCTIONS.md", version: 2,
          content:
            "# Release lead\n\nGet Zuse 2.4 out the door.\n\n- Keep `main` green; fix before you add.\n- One thread per issue, small pull requests.\n- Ask before anything that changes pricing or public copy."
        ),
        ZapperFile(path: "NOTES.md", version: 1, content: "# Notes\n\nStarted the 2.4 checklist."),
        ZapperFile(
          path: "NOTES.md", version: 2,
          content:
            "# Notes\n\n- Windows build is flaky on the signing step.\n- Search PR needs a benchmark."
        ),
        ZapperFile(path: "NOTES.md", version: 3, content: notes),
        ZapperFile(path: "DIGEST.md", version: 1, content: ""),
        ZapperFile(
          path: "DIGEST.md", version: 4,
          content:
            "# Digest\n\nTwo threads running, one queued. Waiting on you for the release-notes question and the ship date."
        ),
        ZapperFile(path: "WORKERS.md", version: 1, content: ""),
        ZapperFile(
          path: "WORKERS.md", version: 2,
          content:
            "# Thread rules\n\n- Open a draft pull request early.\n- Run `swift test` before reporting done."
        ),
      ]
      let events: [ZapperEvent] = [
        ZapperEvent(
          id: "e3", zapperId: "release", workerChatId: "zapper-worker-search", kind: "settled",
          summary:
            "Opened PR #418 with the new search index. Benchmarks: 4.1 s → 0.6 s on the monorepo.",
          acknowledged: true),
        ZapperEvent(
          id: "e2", zapperId: "release", workerChatId: "zapper-worker-notes", kind: "needsInput",
          summary: "Drafted the 2.4 notes. Waiting on whether to include the pricing change.",
          acknowledged: false),
        ZapperEvent(
          id: "e1", zapperId: "release", workerChatId: "zapper-worker-sync", kind: "settled",
          summary:
            "Reproduced the flake: the sync test races the file watcher. Pushed a fix that waits for the first snapshot; CI is running.",
          acknowledged: false),
      ]
      // Opening a chat reloads the preview; keep changes made on the Zapper screens.
      if !zappers.preview {
        zappers.loadPreview(
          zappers: [release, docs], chats: chats,
          running: ["zapper-worker-sync": .working, "zapper-worker-notes": .awaitingInput],
          files: ["release": files], events: ["release": events])
      }
      let rows = titles.map { id, title in
        SessionRow(
          id: id, projectId: "preview-project", title: title, providerId: "Preview",
          model: "sample-balanced", worktreeId: nil)
      }.sorted { $0.id < $1.id }
      sessions += rows.filter { row in !sessions.contains { $0.id == row.id } }
    }
  }
#endif
