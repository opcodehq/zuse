import SwiftUI
import ZuseCore

/// A chat checkout's folders, loaded as they are opened, and file-name search.
@MainActor @Observable final class FileTreeModel {
  let chatID: String
  private(set) var children: [String: [WorkspaceFileEntry]] = [:]
  private(set) var expanded: Set<String> = []
  private(set) var loading: Set<String> = []
  private(set) var failure: String?
  private(set) var results: [WorkspaceFileEntry]?
  private(set) var searching = false
  @ObservationIgnored private var preview = false

  init(chatID: String) { self.chatID = chatID }

  var rows: [FileTree.Row] { FileTree.rows(children: children, expanded: expanded) }
  var loaded: Bool { children[""] != nil }

  func loadRoot(_ rpc: RPCClient?) async {
    guard !preview, children[""] == nil else { return }
    await load("", rpc)
  }

  func toggle(_ folder: WorkspaceFileEntry, rpc: RPCClient?) {
    if expanded.remove(folder.path) != nil { return }
    expanded.insert(folder.path)
    guard children[folder.path] == nil, !preview else { return }
    Task { await load(folder.path, rpc) }
  }

  func refresh(_ rpc: RPCClient?) async {
    guard !preview else { return }
    let open = [""] + expanded.sorted()
    for folder in open { await load(folder, rpc) }
  }

  private func load(_ folder: String, _ rpc: RPCClient?) async {
    guard let rpc else {
      failure = "Connect to your computer to browse its files."
      return
    }
    loading.insert(folder)
    defer { loading.remove(folder) }
    do {
      children[folder] = try await WorkspaceFiles(rpc: rpc, chatID: chatID).list(folder)
      if folder.isEmpty { failure = nil }
    } catch {
      if folder.isEmpty { failure = error.localizedDescription } else { expanded.remove(folder) }
    }
  }

  func search(_ query: String, rpc: RPCClient?) async {
    let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !query.isEmpty else {
      results = nil
      return
    }
    if preview {
      results = children.values.flatMap { $0 }.filter {
        $0.path.localizedCaseInsensitiveContains(query)
      }
      return
    }
    guard let rpc else { return }
    // Typing settles before asking the computer.
    try? await Task.sleep(for: .milliseconds(220))
    guard !Task.isCancelled else { return }
    searching = true
    defer { searching = false }
    do {
      let found = try await WorkspaceFiles(rpc: rpc, chatID: chatID).search(query)
      if !Task.isCancelled { results = found }
    } catch {
      if !Task.isCancelled { results = [] }
    }
  }

  #if DEBUG
    func loadPreview() {
      guard !preview else { return }
      preview = true
      func entries(_ folder: String, _ names: [String]) -> [WorkspaceFileEntry] {
        FileTree.sorted(
          names.map { name in
            let directory = name.hasSuffix("/")
            let clean = directory ? String(name.dropLast()) : name
            return WorkspaceFileEntry(
              path: folder.isEmpty ? clean : folder + "/" + clean,
              kind: directory ? .directory : .file)
          })
      }
      children = [
        "": entries(
          "",
          [
            ".github/", "apps/", "crates/", "docs/", "scripts/", ".gitignore", "Cargo.lock",
            "Cargo.toml", "CHANGELOG.md", "LICENSE", "README.md",
          ]),
        "apps": entries("apps", ["ios/", "web/", "android/"]),
        "apps/ios": entries(
          "apps/ios", ["App/", "Sources/", "Tests/", "Package.swift", "README.md"]),
        "apps/ios/App": entries(
          "apps/ios/App",
          ["ChatPanes.swift", "FilesPane.swift", "TerminalPane.swift", "ZuseApp.swift"]),
        "crates": entries("crates", ["engine/", "protocol/", "ui/"]),
      ]
      expanded = ["apps", "apps/ios"]
    }
  #else
    func loadPreview() {}
  #endif
}

/// The Files pane: a searchable tree. Folders open in place; files open as their own pane.
struct FilesPaneView: View {
  let model: FileTreeModel
  let rpc: RPCClient?
  let preview: Bool
  let openFile: (WorkspaceFileEntry) -> Void
  @State private var query = ""

  var body: some View {
    VStack(spacing: 0) {
      PaneSearchField(text: $query, prompt: "Search files")
        .padding(.horizontal, 20).padding(.bottom, 10)
      Group {
        if let results = model.results {
          if results.isEmpty && !model.searching {
            ContentUnavailableView.search(text: query)
          } else {
            List(results) { entry in
              Button {
                if entry.isDirectory {
                  query = ""
                } else {
                  openFile(entry)
                }
              } label: {
                FileRowLabel(entry: entry, depth: 0, expanded: false, showsPath: true)
              }
              .buttonStyle(.plain)
              .listRowBackground(Color.clear).listRowSeparator(.hidden)
              .listRowInsets(EdgeInsets(top: 0, leading: 22, bottom: 0, trailing: 22))
            }
            .listStyle(.plain)
          }
        } else if let failure = model.failure, !model.loaded {
          ContentUnavailableView(
            "Can’t show files", systemImage: "folder.badge.questionmark",
            description: Text(failure))
        } else if !model.loaded {
          ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if model.rows.isEmpty {
          ContentUnavailableView("No files", systemImage: "folder")
        } else {
          List(model.rows) { row in
            Button {
              if row.entry.isDirectory {
                withAnimation(.snappy(duration: 0.25)) { model.toggle(row.entry, rpc: rpc) }
              } else {
                openFile(row.entry)
              }
            } label: {
              FileRowLabel(
                entry: row.entry, depth: row.depth,
                expanded: model.expanded.contains(row.entry.path),
                loading: model.loading.contains(row.entry.path))
            }
            .buttonStyle(.plain)
            .listRowBackground(Color.clear).listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets(top: 0, leading: 22, bottom: 0, trailing: 22))
            .accessibilityIdentifier("file-row-\(row.entry.path)")
          }
          .listStyle(.plain)
          .refreshable { await model.refresh(rpc) }
        }
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    .scrollContentBackground(.hidden)
    .task(id: rpc.map { ObjectIdentifier($0) }) {
      if preview { model.loadPreview() } else { await model.loadRoot(rpc) }
    }
    .task(id: query) { await model.search(query, rpc: rpc) }
  }
}

private struct FileRowLabel: View {
  let entry: WorkspaceFileEntry
  let depth: Int
  let expanded: Bool
  var loading = false
  var showsPath = false

  var body: some View {
    HStack(spacing: 10) {
      FileIcon(entry: entry).frame(width: 22)
      VStack(alignment: .leading, spacing: 1) {
        HStack(spacing: 5) {
          Text(entry.name).font(.system(size: 16)).lineLimit(1).truncationMode(.middle)
            .foregroundStyle(entry.ignored ? .secondary : .primary)
          if entry.isDirectory && !showsPath {
            if loading {
              ProgressView().controlSize(.mini)
            } else {
              Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.tertiary)
                .rotationEffect(.degrees(expanded ? 90 : 0))
            }
          }
        }
        if showsPath, entry.path != entry.name {
          Text(entry.path).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            .truncationMode(.head)
        }
      }
      Spacer(minLength: 0)
    }
    .padding(.leading, CGFloat(depth) * 20)
    .frame(minHeight: 44)
    .contentShape(Rectangle())
    .accessibilityElement(children: .combine)
    .accessibilityValue(entry.isDirectory ? (expanded ? "Open" : "Closed") : "")
  }
}

/// A file's kind at a glance: folders in blue, then a symbol by extension.
struct FileIcon: View {
  let entry: WorkspaceFileEntry
  var body: some View {
    if entry.isDirectory {
      Image(systemName: "folder.fill").font(.system(size: 17)).foregroundStyle(Color.blue)
    } else {
      Image(systemName: Self.symbol(entry.name)).font(.system(size: 15))
        .foregroundStyle(.secondary)
    }
  }

  static func symbol(_ name: String) -> String {
    let lower = name.lowercased()
    let ext = (lower as NSString).pathExtension
    if lower.hasPrefix(".git") { return "arrow.triangle.branch" }
    switch ext {
    case "swift": return "swift"
    case "md", "markdown", "txt", "rst": return "doc.plaintext"
    case "json", "toml", "yaml", "yml", "lock", "plist", "xml": return "curlybraces"
    case "png", "jpg", "jpeg", "gif", "svg", "webp", "heic": return "photo"
    case "sh", "zsh", "bash", "fish": return "terminal"
    case "html", "css": return "globe"
    case "rs", "ts", "tsx", "js", "jsx", "py", "go", "rb", "kt", "java", "c", "h", "cpp", "m":
      return "chevron.left.forwardslash.chevron.right"
    default: return "doc"
    }
  }
}

/// The rounded search field the list panes share.
struct PaneSearchField: View {
  @Binding var text: String
  var prompt = "Search"
  var body: some View {
    HStack(spacing: 8) {
      Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
      TextField(prompt, text: $text)
        .textFieldStyle(.plain)
        .autocorrectionDisabled()
        #if os(iOS)
          .textInputAutocapitalization(.never)
        #endif
        .submitLabel(.search)
      if !text.isEmpty {
        Button {
          text = ""
        } label: {
          Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Clear search")
      }
    }
    .font(.system(size: 16))
    .padding(.horizontal, 14).frame(height: 42)
    .background(Color.primary.opacity(0.06), in: Capsule())
  }
}
