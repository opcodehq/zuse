import SwiftUI
import ZuseCore

struct FileContentsView: View {
  let rpc: RPCClient?
  let session: SessionRow
  let path: String
  /// Shown as a pane beside the chat rather than in its own sheet.
  var embedded = false
  /// Sample text for offline previews.
  var previewText: String?
  @Environment(\.dismiss) private var dismiss
  @State private var lines: [String] = []
  @State private var error: String?
  @State private var loading = true
  var body: some View {
    if embedded {
      contents
    } else {
      NavigationStack {
        contents
          .navigationTitle(path).compactNavigationTitle()
          .toolbar {
            ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
          }
      }.nativeSheetSize(width: 680, height: 520)
    }
  }

  private var contents: some View {
    Group {
      if loading {
        ProgressView("Loading file…").frame(maxWidth: .infinity, maxHeight: .infinity)
      } else if let error {
        ContentUnavailableView(
          "Cannot open file", systemImage: "doc.badge.ellipsis", description: Text(error))
      } else {
        List(Array(lines.enumerated()), id: \.offset) { index, line in
          HStack(alignment: .top, spacing: 12) {
            Text(String(index + 1)).foregroundStyle(.secondary).frame(
              width: 40, alignment: .trailing)
            Text(line.isEmpty ? " " : line).textSelection(.enabled).frame(
              maxWidth: .infinity, alignment: .leading)
          }.font(.system(.caption, design: .monospaced)).listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
            .listRowInsets(
              embedded ? EdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 16) : nil)
        }.listStyle(.plain).scrollContentBackground(embedded ? .hidden : .automatic)
          .environment(\.defaultMinListRowHeight, embedded ? 18 : 44)
      }
    }
    .safeAreaInset(edge: .top, spacing: 0) {
      if embedded && path.contains("/") {
        Text(path).font(.caption).foregroundStyle(.secondary).lineLimit(1)
          .truncationMode(.head).frame(maxWidth: .infinity, alignment: .leading)
          .padding(.horizontal, 18).padding(.bottom, 6)
      }
    }
    // Restarts when a connection arrives, so a file opened offline fills in once connected.
    .task(id: previewText == nil ? rpc.map(ObjectIdentifier.init)?.hashValue ?? 0 : 1) {
      await load()
    }
  }

  private func load() async {
    loading = true
    error = nil
    defer { loading = false }
    do {
      if let previewText {
        lines = previewText.components(separatedBy: "\n")
        return
      }
      guard let rpc else { throw ClientError("Connect to your computer to read this file.") }
      var payload: [String: JSONValue] = [
        "folderId": .string(session.projectId), "path": .string(path),
      ]
      if let id = session.worktreeId { payload["worktreeId"] = .string(id) }
      let canonical = await rpc.canonical
      let result = try await rpc.call(
        canonical ? "ReadWorkspaceFile" : "git.reviewFileContents",
        canonical ? ["chatId": .string(session.id), "path": .string(path)] : payload)
      guard let source = result[canonical ? "text" : "newContent"].string else {
        throw ClientError("This file was removed or has no text content.")
      }
      let text =
        result["truncated"].bool ? source + "\n\n[File preview truncated by host]" : source
      let parsed = await Task.detached(priority: .userInitiated) {
        text.components(separatedBy: "\n")
      }.value
      try Task.checkCancellation()
      lines = parsed
    } catch { if !Task.isCancelled { self.error = error.localizedDescription } }
  }
}
