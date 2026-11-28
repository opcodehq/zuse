#if DEBUG
  import SwiftUI
  import ZuseCore

  /// A sample chat in the Rust engine's transcript shape, so `--native-preview` exercises the same
  /// `CanonicalTranscript` → `ChatTimeline` path a connected computer does.
  enum PreviewConversation {
    static let screenshotPath = "/preview/uploads/0f3a9c1e-composer.png"

    /// Entries as `WatchDocMessages` sends them. `running` adds a turn still in progress.
    static func entries(now: Date = Date(), running: Bool) -> [JSONValue] {
      let start = Int(now.timeIntervalSince1970 * 1000) - 3_600_000
      var entries: [JSONValue] = [
        entry(
          "turn-1", "user", at: start,
          [
            text(
              "t0",
              "The chat composer feels cramped on iPhone. Can you tighten it up and make sure the tests still pass? Here’s what I’m seeing:"
            ),
            part(
              "i0", "image",
              [
                "path": .string(screenshotPath), "name": .string("composer.png"),
                "mimeType": .string("image/png"),
              ]),
          ]),
        entry(
          "reply-1", "assistant", at: start + 2_000, duration: 48_000,
          [
            part(
              "r1", "reasoning",
              [
                "text": .string(
                  "The screenshot shows the action row wrapping under the text field. I should find where the composer is laid out, adjust the spacing, and keep every control at a 44-point touch target."
                )
              ]),
            text("n1", "I’ll start by finding where the composer is laid out."),
            tool(
              "s1", ["kind": .string("search"), "pattern": .string("struct Composer")],
              output:
                "App/Composer.swift:12: struct Composer: View {\nApp/ComposerActions.swift:4: struct ComposerActions: View {"
            ),
            tool(
              "f1", ["kind": .string("readFile"), "path": .string("App/Composer.swift")],
              output: "84 lines: input, attachment menu, model label and send control"),
            tool(
              "e1", ["kind": .string("editFile"), "path": .string("App/Composer.swift")],
              extra: [
                "diff": .object([
                  "path": .string("App/Composer.swift"),
                  "oldText": .string(
                    "HStack(spacing: 16) {\n  attachmentMenu\n  Spacer()\n  modelLabel\n  sendButton\n}\n.padding(.vertical, 14)"
                  ),
                  "newText": .string(
                    "HStack(spacing: 8) {\n  attachmentMenu\n  Spacer(minLength: 4)\n  modelLabel\n  sendButton\n}\n.padding(.vertical, 8)"
                  ),
                ]),
                "diffStats": .array([
                  .object([
                    "path": .string("App/Composer.swift"), "additions": .number(3),
                    "deletions": .number(3),
                  ])
                ]),
              ]),
            tool(
              "x1",
              ["kind": .string("exec"), "command": .string("swift test --filter ComposerTests")],
              output:
                "Test Suite 'ComposerTests' passed\n  ✓ testCompactLayout (0.12s)\n  ✓ testAttachmentMenu (0.08s)\n  ✓ testSendAndStop (0.10s)\nExecuted 3 tests, with 0 failures"
            ),
            text(
              "a1",
              """
              ### Composer tightened

              The action row now stays on one line up to the **XXL** text size, and every control keeps a 44-point touch target.

              - Halved the spacing between the attachment, model and send controls
              - Let the spacer shrink to 4 points before anything wraps
              - Trimmed the vertical padding from 14 to 8 points

              ```swift
              HStack(spacing: 8) {
                attachmentMenu
                Spacer(minLength: 4)
                modelLabel
                sendButton
              }
              ```

              | Check | Result |
              | --- | --- |
              | Composer tests | 3 passed |
              | Smallest touch target | 44 pt |
              | Wraps at XXL | No |
              """),
          ]),
        entry(
          "turn-2", "user", at: start + 600_000,
          [text("t0", "Nice. Can you also run the full suite and the snapshot tests?")]),
        entry(
          "reply-2", "assistant", at: start + 602_000, duration: 21_000,
          [
            tool(
              "x2", ["kind": .string("exec"), "command": .string("swift test")],
              output: "Executed 214 tests, with 0 failures (38.2s)"),
            tool(
              "g2", ["kind": .string("glob"), "pattern": .string("**/*.snap")],
              output: "12 files"),
            tool(
              "x3", ["kind": .string("exec"), "command": .string("bun run snapshots")],
              output: "error: the snapshot server isn’t running on port 4005", error: true),
            text(
              "a2",
              "The full suite passes: **214 tests**, no failures. The snapshot run couldn’t start because its server isn’t running, so nothing was compared. Start it with `bun run snapshot-server` and I’ll run them again."
            ),
            part("err", "error", ["message": .string("Snapshot server unavailable on port 4005.")]),
          ]),
      ]
      if running {
        let live = Int(now.timeIntervalSince1970 * 1000) - 14_000
        entries += [
          entry(
            "turn-3", "user", at: live - 1_000,
            [text("t0", "Start the snapshot server and run them again.")]),
          entry(
            "reply-3", "assistant", at: live, status: "streaming",
            [
              part(
                "r3", "reasoning",
                [
                  "text": .string(
                    "Start the server in the background, wait for the port, then run the snapshots."
                  )
                ]),
              tool(
                "x4",
                ["kind": .string("exec"), "command": .string("bun run snapshot-server &")],
                output: "Listening on http://127.0.0.1:4005"),
              tool(
                "x5", ["kind": .string("exec"), "command": .string("bun run snapshots")],
                resolved: false),
            ]),
        ]
      }
      return entries
    }

    private static func entry(
      _ id: String, _ role: String, at created: Int, duration: Int? = nil,
      status: String = "complete", _ parts: [JSONValue]
    ) -> JSONValue {
      var value: [String: JSONValue] = [
        "id": .string(id), "role": .string(role), "parts": .array(parts),
        "createdAt": .number(Double(created)), "deviceId": .string("preview-mac"),
        "status": .string(status),
      ]
      if let duration { value["durationMs"] = .number(Double(duration)) }
      return .object(value)
    }
    private static func part(_ id: String, _ kind: String, _ fields: [String: JSONValue])
      -> JSONValue
    {
      var value = fields
      value["id"] = .string(id)
      value["kind"] = .string(kind)
      return .object(value)
    }
    private static func text(_ id: String, _ body: String) -> JSONValue {
      part(id, "text", ["text": .string(body)])
    }
    private static func tool(
      _ id: String, _ call: [String: JSONValue], output: String? = nil, resolved: Bool = true,
      error: Bool = false, extra: [String: JSONValue] = [:]
    ) -> JSONValue {
      var fields: [String: JSONValue] = [
        "call": .object(call), "resolved": .bool(resolved), "isError": .bool(error),
      ]
      if let output { fields["output"] = .string(output) }
      fields.merge(extra) { $1 }
      return part(id, "tool", fields)
    }
  }

  /// Stand-in pictures for attachments in previews, drawn instead of read from a computer.
  @MainActor enum PreviewImages {
    static func image(for path: String) -> NativeImage? {
      guard path.hasPrefix("/preview/") else { return nil }
      let renderer = ImageRenderer(content: SampleScreenshot())
      renderer.scale = 2
      #if os(macOS)
        return renderer.nsImage
      #else
        return renderer.uiImage
      #endif
    }
  }

  /// A small drawing of a cramped composer, for the sample chat's attachment.
  private struct SampleScreenshot: View {
    var body: some View {
      ZStack {
        LinearGradient(
          colors: [
            Color(red: 0.16, green: 0.18, blue: 0.24), Color(red: 0.08, green: 0.09, blue: 0.12),
          ],
          startPoint: .top, endPoint: .bottom)
        VStack(alignment: .leading, spacing: 10) {
          ForEach(0..<3) { index in
            RoundedRectangle(cornerRadius: 4).fill(.white.opacity(0.14))
              .frame(width: [150, 110, 130][index], height: 8)
          }
          Spacer()
          VStack(alignment: .leading, spacing: 8) {
            RoundedRectangle(cornerRadius: 4).fill(.white.opacity(0.35)).frame(
              width: 120, height: 8)
            HStack(spacing: 14) {
              Circle().fill(.white.opacity(0.25)).frame(width: 18)
              Spacer()
              Capsule().fill(.white.opacity(0.25)).frame(width: 46, height: 14)
              Circle().fill(Color(red: 0.55, green: 0.79, blue: 0.13)).frame(width: 22)
            }
          }
          .padding(10)
          .background(.white.opacity(0.1), in: RoundedRectangle(cornerRadius: 14))
        }
        .padding(16)
      }
      .frame(width: 200, height: 200)
    }
  }
#endif
