import SwiftUI
import ZuseCore

struct DiffView: View {
  let rpc: RPCClient
  let session: SessionRow
  @Environment(\.dismiss) private var dismiss
  @State private var scope = "unstaged"
  @State private var patches: [Patch] = []
  @State private var error: String?
  @State private var loading = false
  @State private var file: Patch?
  @State private var requestID = UUID()
  struct Patch: Identifiable, Sendable {
    let id: String
    let lines: [DiffLine]
    let mode: String
    let truncated: Bool
    let error: String?
  }
  @State private var collapsed: Set<String> = []
  var body: some View {
    NavigationStack {
      VStack(spacing: 0) {
        Picker("Changes", selection: $scope) {
          Text("Working tree").tag("unstaged")
          Text("Staged").tag("staged")
          Text("Branch").tag("branch")
        }.pickerStyle(.segmented).padding(.horizontal, 16).padding(.vertical, 10)
        if loading { ProgressView("Loading changes…").padding() }
        if let error {
          Text(error).font(.callout).foregroundStyle(.red).padding()
          Button("Retry") { Task { await load() } }
        }
        if !loading && patches.isEmpty && error == nil {
          ContentUnavailableView("No changes", systemImage: "checkmark.circle")
        }
        ScrollView {
          LazyVStack(spacing: 16, pinnedViews: [.sectionHeaders]) {
            ForEach(patches) { patch in
              Section {
                if !collapsed.contains(patch.id) {
                  if let error = patch.error {
                    Text(error).font(.caption).foregroundStyle(.red).padding(12)
                  } else if patch.mode == "binary" {
                    Label("Binary file changed", systemImage: "doc").font(.caption).padding(12)
                  } else {
                    LazyVStack(spacing: 0) {
                      ForEach(patch.lines.filter { $0.kind != .header || $0.text.hasPrefix("@@") })
                      { line in
                        diffLine(line)
                      }
                    }.textSelection(.enabled)
                    if patch.truncated {
                      Label("Patch truncated by server", systemImage: "exclamationmark.triangle")
                        .font(.caption).foregroundStyle(.orange).padding(10)
                    }
                  }
                }
              } header: {
                HStack(spacing: 8) {
                  Button {
                    if !collapsed.insert(patch.id).inserted { collapsed.remove(patch.id) }
                  } label: {
                    HStack(spacing: 8) {
                      Image(
                        systemName: collapsed.contains(patch.id) ? "chevron.right" : "chevron.down"
                      )
                      .font(.system(size: 9, weight: .semibold))
                      Image(systemName: "doc.text").foregroundStyle(.secondary)
                      Text(patch.id).font(.system(size: 14, weight: .medium, design: .monospaced))
                        .lineLimit(1).truncationMode(.middle)
                    }.frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
                  }.buttonStyle(.plain)
                  Text("+\(patch.lines.filter { $0.kind == .addition }.count)").foregroundStyle(
                    .green)
                  Text("−\(patch.lines.filter { $0.kind == .deletion }.count)").foregroundStyle(
                    .red)
                  Button {
                    file = patch
                  } label: {
                    Image(systemName: "arrow.up.right.square")
                  }
                  .buttonStyle(.plain).accessibilityLabel("View working-copy file")
                }.font(.system(size: 13, weight: .medium, design: .monospaced))
                  .padding(.horizontal, 12).frame(minHeight: 40)
                  .background(.bar)
              }
            }
          }.padding(.bottom, 24).frame(maxWidth: 960).frame(maxWidth: .infinity)
        }
      }.background(AppTheme.surface).navigationTitle("Changes").compactNavigationTitle()
        .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        .task(id: scope) { await load() }
        .sheet(item: $file) { patch in FileContentsView(rpc: rpc, session: session, path: patch.id)
        }
    }
  }
  private func diffLine(_ line: DiffLine) -> some View {
    let tint: Color = line.kind == .addition ? .green : line.kind == .deletion ? .red : .secondary
    return HStack(alignment: .top, spacing: 0) {
      if line.kind != .header {
        HStack(spacing: 4) {
          Text(line.oldLine.map(String.init) ?? "").frame(width: 25, alignment: .trailing)
          Text(line.newLine.map(String.init) ?? "").frame(width: 25, alignment: .trailing)
        }.font(.system(size: 11, design: .monospaced)).foregroundStyle(.tertiary)
          .padding(.trailing, 8).padding(.top, 3)
      }
      Text(line.text)
        .font(.system(size: 12.5, design: .monospaced)).lineSpacing(2)
        .foregroundStyle(line.kind == .header ? Color.secondary : Color.primary)
        .frame(maxWidth: .infinity, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true).padding(.vertical, 3).padding(.trailing, 10)
    }.padding(.leading, 4)
      .background(
        tint.opacity(
          line.kind == .addition || line.kind == .deletion ? 0.09 : line.kind == .header ? 0.04 : 0)
      )
      .overlay(alignment: .leading) {
        if line.kind == .addition || line.kind == .deletion { tint.opacity(0.6).frame(width: 2) }
      }
  }
  private func load() async {
    let request = UUID()
    requestID = request
    patches = []
    error = nil
    loading = true
    defer { if requestID == request { loading = false } }
    let requestedScope = scope
    do {
      if await rpc.canonical {
        let snapshot = try await CanonicalWorkspace(rpc).diff(
          chatID: session.id, scope: requestedScope)
        let loaded = await Task.detached(priority: .userInitiated) {
          snapshot["files"].array.compactMap { file -> Patch? in
            guard let path = file["path"].string else { return nil }
            let patch = GitPatch.file(
              snapshot["patch"].string ?? "", path: path, oldPath: file["oldPath"].string)
            return Patch(
              id: path, lines: DiffLine.parse(patch ?? ""),
              mode: file["binary"].bool ? "binary" : requestedScope,
              truncated: snapshot["truncated"].bool,
              error: patch == nil && !file["binary"].bool ? "Patch unavailable for this file." : nil
            )
          }
        }.value
        guard requestedScope == scope, requestID == request, !Task.isCancelled else { return }
        patches = loaded
        return
      }

      var payload: [String: JSONValue] = [
        "folderId": .string(session.projectId), "scope": .string(requestedScope),
      ]
      if let worktree = session.worktreeId { payload["worktreeId"] = .string(worktree) }
      let stream = try await rpc.stream("git.reviewPatches", payload)
      for try await item in stream {
        try Task.checkCancellation()
        guard requestedScope == scope, requestID == request else { return }
        let result = item["result"]
        let patch = result["patch"].string ?? ""
        let lines = await Task.detached(priority: .userInitiated) { DiffLine.parse(patch) }.value
        guard requestedScope == scope, requestID == request, !Task.isCancelled else { return }
        patches.append(
          Patch(
            id: item["path"].string ?? UUID().uuidString, lines: lines,
            mode: result["mode"].string ?? "", truncated: result["truncated"].bool,
            error: item["error"].string))
      }
    } catch {
      if !Task.isCancelled && requestedScope == scope && requestID == request {
        self.error = error.localizedDescription
      }
    }
  }
}
