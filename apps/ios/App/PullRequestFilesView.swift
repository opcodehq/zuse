import SwiftUI
import ZuseCore

/// The Files tab: one collapsible section per file with a unified, numbered diff.
struct PullRequestFilesView: View {
  let model: PullRequestDetailModel
  @State private var collapsed: Set<String> = []
  @State private var viewed: Set<String> = []
  @State private var revealed: Set<String> = []
  /// Lines shown per file before asking; long generated files stay cheap to scroll past.
  private let lineBudget = 600

  var body: some View {
    Group {
      if let files = model.files {
        if files.isEmpty {
          PullRequestUnavailable(
            title: "No file changes", message: "This pull request doesn’t change any files.",
            symbol: "doc"
          )
          .padding(.top, 40)
        } else {
          list(files)
        }
      } else if let error = model.filesError {
        PullRequestUnavailable(
          title: "Couldn’t load the diff", message: error, symbol: "exclamationmark.triangle"
        ) {
          Task { await model.loadFiles(force: true) }
        }
        .padding(.top, 40)
      } else {
        ProgressView("Loading changes…").frame(maxWidth: .infinity).padding(.top, 80)
      }
    }
  }

  private func list(_ files: [PullRequestDiffFile]) -> some View {
    LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
      summary(files).padding(.horizontal, 16).padding(.top, 4).padding(.bottom, 6)
      ForEach(files) { file in
        Section {
          if !collapsed.contains(file.id) {
            fileBody(file).padding(.bottom, 18)
          }
        } header: {
          header(file).padding(.horizontal, 10).padding(.vertical, 5)
        }
      }
    }
  }

  private func summary(_ files: [PullRequestDiffFile]) -> some View {
    HStack(spacing: 8) {
      Text(files.count == 1 ? "1 file changed" : "\(files.count) files changed")
        .font(.system(size: 15, weight: .medium))
      PullRequestLineCounts(
        additions: files.reduce(0) { $0 + $1.additions },
        deletions: files.reduce(0) { $0 + $1.deletions })
      Spacer()
      Text("\(viewed.count)/\(files.count) viewed").font(.system(size: 13))
        .foregroundStyle(.secondary).monospacedDigit()
    }
  }

  private func header(_ file: PullRequestDiffFile) -> some View {
    let isCollapsed = collapsed.contains(file.id)
    let isViewed = viewed.contains(file.id)
    return HStack(spacing: 8) {
      Button {
        withAnimation(.snappy(duration: 0.2)) { toggle(&collapsed, file.id) }
      } label: {
        HStack(spacing: 8) {
          Image(systemName: isCollapsed ? "chevron.right" : "chevron.down")
            .font(.system(size: 11, weight: .bold)).foregroundStyle(.tertiary).frame(width: 12)
          PullRequestFileBadge(file: file)
          Text(file.fileName).font(.system(size: 15, weight: .semibold)).lineLimit(1)
            .layoutPriority(1)
          if !file.directory.isEmpty {
            Text(file.directory).font(.system(size: 13)).foregroundStyle(.secondary)
              .lineLimit(1).truncationMode(.head)
          }
          Spacer(minLength: 4)
          PullRequestLineCounts(additions: file.additions, deletions: file.deletions, size: 13)
            .fixedSize()
        }
        .contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel("\(file.path), \(isCollapsed ? "collapsed" : "expanded")")
      Button {
        withAnimation(.snappy(duration: 0.2)) {
          toggle(&viewed, file.id)
          if viewed.contains(file.id) {
            collapsed.insert(file.id)
          } else {
            collapsed.remove(file.id)
          }
        }
      } label: {
        Image(systemName: isViewed ? "checkmark.square.fill" : "square")
          .font(.system(size: 19, weight: .regular))
          .foregroundStyle(isViewed ? AppTheme.accent : Color.secondary.opacity(0.7))
          .frame(width: 30, height: 30).contentShape(Rectangle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel(isViewed ? "Viewed" : "Mark as viewed")
    }
    .padding(.leading, 12).padding(.trailing, 8).frame(height: 46)
    .floatingGlass(radius: 23)
    .contextMenu {
      Button("Copy path", systemImage: "doc.on.doc") { NativePlatform.copy(file.path) }
    }
  }

  @ViewBuilder private func fileBody(_ file: PullRequestDiffFile) -> some View {
    if file.binary {
      note("Binary file not shown", symbol: "doc")
    } else if file.hunks.isEmpty {
      if file.change == .renamed, let old = file.oldPath {
        note("Renamed from \(old) without changes", symbol: "arrow.right.doc.on.clipboard")
      } else {
        note("No text changes", symbol: "doc")
      }
    } else {
      let hunks = visibleHunks(file)
      VStack(alignment: .leading, spacing: 0) {
        ForEach(hunks.shown) { hunk in
          if hunk.hiddenBefore > 0 || hunk.id > 0 {
            PullRequestUnmodifiedSeparator(count: hunk.hiddenBefore, context: hunk.context)
          }
          PullRequestHunkView(lines: hunk.lines)
        }
        if hunks.remaining > 0 {
          Button {
            revealed.insert(file.id)
          } label: {
            Text("Show \(hunks.remaining.formatted()) more lines")
              .font(.system(size: 14, weight: .medium)).frame(maxWidth: .infinity)
              .frame(height: 38)
          }
          .buttonStyle(.plain)
          .background(Color.primary.opacity(0.05), in: Capsule())
          .padding(.horizontal, 16).padding(.top, 10)
        }
      }
      .padding(.top, 4)
    }
  }

  private func visibleHunks(_ file: PullRequestDiffFile) -> (
    shown: [PullRequestDiffHunk], remaining: Int
  ) {
    guard !revealed.contains(file.id) else { return (file.hunks, 0) }
    var budget = lineBudget
    var shown: [PullRequestDiffHunk] = []
    var remaining = 0
    for hunk in file.hunks {
      if budget <= 0 {
        remaining += hunk.lines.count
        continue
      }
      var part = hunk
      if hunk.lines.count > budget {
        part.lines = Array(hunk.lines.prefix(budget))
        remaining += hunk.lines.count - budget
      }
      budget -= part.lines.count
      shown.append(part)
    }
    return (shown, remaining)
  }

  private func note(_ text: String, symbol: String) -> some View {
    Label(text, systemImage: symbol).font(.system(size: 14)).foregroundStyle(.secondary)
      .padding(.horizontal, 22).padding(.vertical, 12)
  }

  private func toggle(_ set: inout Set<String>, _ id: String) {
    if !set.insert(id).inserted { set.remove(id) }
  }
}

/// A small coloured mark for the file's type.
struct PullRequestFileBadge: View {
  let file: PullRequestDiffFile
  var body: some View {
    Image(systemName: symbol)
      .font(.system(size: 12, weight: .semibold))
      .foregroundStyle(tint)
      .frame(width: 18, height: 18)
  }
  private var symbol: String {
    switch file.change {
    case .added: return "plus.square"
    case .deleted: return "minus.square"
    case .renamed: return "arrow.right.square"
    case .modified: break
    }
    switch file.fileExtension {
    case "md", "txt", "rst": return "doc.text"
    case "png", "jpg", "jpeg", "gif", "svg", "webp", "ico": return "photo"
    case "json", "yml", "yaml", "toml", "lock", "plist": return "gearshape"
    default: return "chevron.left.forwardslash.chevron.right"
    }
  }
  private var tint: Color {
    switch file.change {
    case .added: return PullRequestTint.addition
    case .deleted: return PullRequestTint.deletion
    case .renamed: return .blue
    case .modified: break
    }
    switch file.fileExtension {
    case "rs": return Color(red: 0.87, green: 0.42, blue: 0.2)
    case "swift": return .orange
    case "ts", "tsx": return .blue
    case "js", "jsx", "mjs": return Color(red: 0.8, green: 0.65, blue: 0.0)
    case "py": return .indigo
    case "go": return .cyan
    case "md", "txt": return .secondary
    default: return .secondary
    }
  }
}

/// "↕ 218 unmodified lines", between hunks.
struct PullRequestUnmodifiedSeparator: View {
  let count: Int
  let context: String
  var body: some View {
    HStack(spacing: 8) {
      Rectangle().fill(Color.primary.opacity(0.07)).frame(height: 1)
      HStack(spacing: 5) {
        Image(systemName: "arrow.up.and.down").font(.system(size: 10, weight: .semibold))
        Text(
          count > 0
            ? "\(count.formatted()) unmodified \(count == 1 ? "line" : "lines")" : "Next change")
      }
      .font(.system(size: 12, weight: .medium)).foregroundStyle(.secondary)
      .padding(.horizontal, 10).frame(height: 24)
      .background(Color.primary.opacity(0.06), in: Capsule())
      .fixedSize()
      Rectangle().fill(Color.primary.opacity(0.07)).frame(height: 1)
    }
    .padding(.vertical, 8)
    .accessibilityElement(children: .combine)
  }
}

/// One hunk: fixed line numbers on the left, monospaced code that scrolls sideways.
struct PullRequestHunkView: View {
  let lines: [DiffLine]
  private let rowHeight: CGFloat = 19
  var body: some View {
    let digits = String(lines.compactMap { $0.newLine ?? $0.oldLine }.max() ?? 0).count
    HStack(alignment: .top, spacing: 0) {
      VStack(alignment: .trailing, spacing: 0) {
        ForEach(lines) { line in
          Text((line.newLine ?? line.oldLine).map(String.init) ?? "")
            .frame(width: CGFloat(max(digits, 2)) * 7.5 + 4, alignment: .trailing)
            .padding(.leading, 12).padding(.trailing, 10)
            .frame(height: rowHeight)
            .foregroundStyle(line.kind == .context ? Color.secondary.opacity(0.7) : Color.secondary)
            .background(tint(line).opacity(0.16))
            .overlay(alignment: .leading) {
              if line.kind != .context { tint(line).opacity(0.75).frame(width: 3) }
            }
        }
      }
      .font(.system(size: 11, design: .monospaced))
      PullRequestCodeScroller {
        ForEach(lines) { line in
          Text(line.text.isEmpty ? " " : line.text)
            .font(.system(size: 12.5, design: .monospaced))
            .fixedSize()
            .padding(.trailing, 16)
            .frame(height: rowHeight)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(tint(line).opacity(0.1))
        }
      }
    }
    .textSelection(.enabled)
  }
  private func tint(_ line: DiffLine) -> Color {
    switch line.kind {
    case .addition: return PullRequestTint.addition
    case .deletion: return PullRequestTint.deletion
    default: return .clear
    }
  }
}
