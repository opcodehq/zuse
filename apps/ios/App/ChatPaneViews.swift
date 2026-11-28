import SwiftUI
import ZuseCore

/// Everything a chat's panes need from the chat page that hosts them.
@MainActor struct ChatPaneContext {
  let workspace: WorkspaceModel
  let store: ChatPaneStore
  let session: SessionRow
  /// The Zapper whose coordinator conversation this is.
  let zapper: Zapper?
  /// The chat checkout's pull request.
  let pullRequest: PullRequestCheckoutSummary?
  let showsChanges: Bool
  /// Offline sample data (`--panes-preview` without a computer).
  let preview: Bool
  let push: (ZapperRoute) -> Void
  let openChat: (SessionRow) -> Void
  let closeZapper: () -> Void

  var chatID: String { session.id }
  var rpc: RPCClient? { workspace.rpc }
  var connected: Bool { workspace.connectionState == "Connected" && workspace.rpc != nil }

  func open(_ kind: ChatPaneKind, target: String? = nil, title: String) {
    withAnimation(.snappy(duration: 0.35)) {
      store.open(chatID, kind, target: target, title: title)
      store.setOverview(chatID, false)
    }
  }

  func close(_ pane: String) {
    withAnimation(.snappy(duration: 0.3)) { store.close(chatID, pane) }
  }

  /// Starts a new thread off this chat and opens it, the way desktop's "+ New tab" does.
  func newThread() {
    guard !preview else { return }
    Task {
      do {
        let session = try await workspace.createSideThread(parent: chatID)
        openChat(session)
      } catch {
        workspace.error = error.localizedDescription
      }
    }
  }
}

/// What the pane picker knows beyond the chat itself, loaded from the computer.
struct ChatPaneSources: Equatable {
  var terminals: [TerminalSessionInfo] = []
  var documents: [ZapperFile] = []
  /// Terminals and files need the current engine protocol.
  var workspaceFiles = false

  @MainActor static func load(_ context: ChatPaneContext) async -> ChatPaneSources {
    var sources = ChatPaneSources()
    if context.preview {
      sources.workspaceFiles = true
      sources.terminals = [TerminalSessionInfo(id: "preview-terminal", cwd: "~", shell: "zsh")]
      if let zapper = context.zapper {
        sources.documents =
          (try? await context.workspace.zappers.files(zapper.id))?.filter(
            ZapperDrive.isDocument) ?? []
      }
      return sources
    }
    guard context.connected, let rpc = context.rpc, await rpc.canonical else { return sources }
    sources.workspaceFiles = true
    async let terminals = try? TerminalClient(rpc: rpc).list(chatID: context.chatID)
    if let zapper = context.zapper {
      sources.documents =
        (try? await context.workspace.zappers.files(zapper.id))?.filter(ZapperDrive.isDocument)
        ?? []
    }
    sources.terminals = await terminals ?? []
    return sources
  }
}

extension ChatPaneKind {
  var defaultTitle: String {
    switch self {
    case .threads: return "Threads"
    case .pullRequests: return "Pull requests"
    case .issues: return "Issues"
    case .terminal: return "Terminal"
    case .files: return "Files"
    case .docs: return "Docs"
    case .overview: return "Overview"
    case .changes: return "Changes"
    case .pullRequest: return "Pull request"
    case .file: return "File"
    case .document: return "Document"
    }
  }

  var symbol: String {
    switch self {
    case .threads: return "bubble.left.and.bubble.right"
    case .pullRequests, .pullRequest: return AppIcon.pullRequest
    case .issues: return "circle.dashed"
    case .terminal: return "terminal"
    case .files: return "doc.on.doc"
    case .docs, .document: return "doc.richtext"
    case .overview: return AppIcon.zapper
    case .changes: return "plusminus"
    case .file: return "doc.text"
    }
  }
}

// MARK: - Picker

/// The last page: a grid of the panes this chat can open.
struct PaneSelectionView: View {
  let context: ChatPaneContext
  let sources: ChatPaneSources

  struct Tile: Identifiable {
    let kind: ChatPaneKind
    var title: String
    var count: Int?
    var target: String?
    var id: String { kind.rawValue }
  }

  var tiles: [Tile] {
    var tiles: [Tile] = []
    let files = sources.workspaceFiles
    if let zapper = context.zapper {
      tiles.append(Tile(kind: .threads, title: "Threads", count: zapper.workers.count))
      let pulls = ZapperPullRequests.items(zapper)
      if !pulls.isEmpty {
        tiles.append(Tile(kind: .pullRequests, title: "Pull requests", count: pulls.count))
      }
      let open = zapper.issues.filter { !$0.details.status.terminal }.count
      tiles.append(Tile(kind: .issues, title: "Issues", count: open == 0 ? nil : open))
    } else {
      if context.showsChanges { tiles.append(Tile(kind: .changes, title: "Changes")) }
      if let pull = context.pullRequest {
        tiles.append(
          Tile(kind: .pullRequest, title: "Pull request #\(pull.number)", target: pull.url))
      }
    }
    if files {
      let live = sources.terminals.count
      tiles.append(Tile(kind: .terminal, title: "Terminals", count: live == 0 ? nil : live))
      tiles.append(Tile(kind: .files, title: "Files"))
    }
    if context.zapper != nil {
      if !sources.documents.isEmpty {
        tiles.append(Tile(kind: .docs, title: "Docs", count: sources.documents.count))
      }
      tiles.append(Tile(kind: .overview, title: "Overview"))
    }
    return tiles
  }

  var body: some View {
    // Two tiles across a phone, more on wider pages.
    let columns = [GridItem(.adaptive(minimum: 160, maximum: 250), spacing: 14)]
    GeometryReader { proxy in
      ScrollView {
        tileGrid(columns)
          .padding(.horizontal, 20).padding(.vertical, 20)
          .frame(maxWidth: 760).frame(maxWidth: .infinity)
          // A short grid sits in the middle of the page, as the cards do in the overview.
          .frame(minHeight: proxy.size.height)
      }
      .scrollIndicators(.hidden)
    }
  }

  private func tileGrid(_ columns: [GridItem]) -> some View {
    VStack(alignment: .leading, spacing: 14) {
      LazyVGrid(columns: columns, spacing: 14) {
        ForEach(tiles) { tile in
          Button {
            open(tile)
          } label: {
            PaneTile(symbol: tile.kind.symbol, title: tile.title, count: tile.count)
          }
          .buttonStyle(PaneTileButtonStyle())
          .accessibilityIdentifier("pane-tile-\(tile.kind.rawValue)")
        }
      }
      // Under the grid, like desktop's "+ New tab": start a fresh thread off this chat.
      if context.connected || context.preview {
        NewThreadRow { context.newThread() }
      }
      if tiles.isEmpty && !(context.connected || context.preview) {
        Text("Connect to your computer to open terminals and files beside this chat.")
          .font(.subheadline).foregroundStyle(.secondary).frame(maxWidth: .infinity)
          .padding(.top, 40)
      }
    }
  }

  private func open(_ tile: Tile) {
    switch tile.kind {
    case .terminal:
      // Pick up a shell this chat already has before opening another.
      let attached = context.store.attachedTerminals(context.chatID)
      if let free = sources.terminals.first(where: { !attached.contains($0.id) }) {
        context.open(.terminal, target: free.id, title: TerminalSessionModel.label(free))
      } else {
        context.open(.terminal, title: "Terminal")
      }
    case .pullRequest:
      context.open(.pullRequest, target: tile.target, title: tile.title)
    default:
      context.open(tile.kind, title: tile.kind.defaultTitle)
    }
  }
}

/// One picker tile: icon top-left, count top-right, name bottom-left.
struct PaneTile: View {
  let symbol: String
  let title: String
  var count: Int?
  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(alignment: .top) {
        AppIcon(name: symbol, size: 18, weight: .semibold).foregroundStyle(AppTheme.accent)
          .frame(width: 40, height: 40)
          .background(
            AppTheme.accent.opacity(0.14),
            in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        Spacer(minLength: 4)
        if let count {
          Text("\(count)").font(.system(size: 13, weight: .semibold)).monospacedDigit()
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8).frame(minWidth: 24, minHeight: 22)
            .background(Color.primary.opacity(0.08), in: Capsule())
        }
      }
      Spacer(minLength: 20)
      Text(title).font(.system(size: 18, weight: .semibold)).foregroundStyle(.primary)
        .lineLimit(2).minimumScaleFactor(0.8).multilineTextAlignment(.leading)
    }
    .padding(18)
    .frame(maxWidth: .infinity, minHeight: 140, alignment: .leading)
    .aspectRatio(1, contentMode: .fit)
    .background(
      Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 24, style: .continuous)
    )
    .overlay(
      RoundedRectangle(cornerRadius: 24, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.06), lineWidth: 1)
    )
    .contentShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
    .accessibilityElement(children: .ignore)
    .accessibilityLabel(count.map { "\(title), \($0)" } ?? title)
    .accessibilityAddTraits(.isButton)
  }
}

private struct PaneTileButtonStyle: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .scaleEffect(configuration.isPressed ? 0.96 : 1)
      .opacity(configuration.isPressed ? 0.85 : 1)
      .animation(.snappy(duration: 0.18), value: configuration.isPressed)
  }
}

/// The full-width action under the picker grid that starts a new thread off this chat.
private struct NewThreadRow: View {
  let action: () -> Void
  var body: some View {
    Button(action: action) {
      HStack(spacing: 10) {
        Image(systemName: "plus").font(.system(size: 15, weight: .bold))
          .foregroundStyle(AppTheme.accent).frame(width: 28, height: 28)
          .background(
            AppTheme.accent.opacity(0.14),
            in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        Text("New thread").font(.system(size: 16, weight: .semibold)).foregroundStyle(.primary)
        Spacer(minLength: 0)
      }
      .padding(.horizontal, 14).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
      .background(
        Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 18, style: .continuous)
      )
      .contentShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
    }
    .buttonStyle(PaneTileButtonStyle())
    .accessibilityIdentifier("pane-new-thread")
    .accessibilityLabel("New thread")
  }
}

// MARK: - Pane chrome

/// A pane's title row: what it is, its own actions, and a close button.
struct PaneHeader<Actions: View>: View {
  let pane: ChatPane
  let close: () -> Void
  @ViewBuilder var actions: Actions

  var body: some View {
    HStack(spacing: 10) {
      AppIcon(name: pane.kind.symbol, size: 15, weight: .medium).foregroundStyle(.secondary)
        .frame(width: 20)
      Text(pane.title).font(.system(size: 17, weight: .semibold)).lineLimit(1)
        .truncationMode(.middle)
        .accessibilityAddTraits(.isHeader)
      Spacer(minLength: 8)
      actions
      Button(action: close) {
        Image(systemName: "xmark").font(.system(size: 13, weight: .semibold))
          .foregroundStyle(.secondary)
          .frame(width: 32, height: 32)
          .background(Color.primary.opacity(0.07), in: Circle())
          .contentShape(Circle())
      }
      .buttonStyle(.plain)
      .accessibilityLabel("Close \(pane.title)")
      .accessibilityIdentifier("pane-close")
    }
    .padding(.leading, 22).padding(.trailing, 16)
    .frame(height: 56)
  }
}

extension PaneHeader where Actions == EmptyView {
  init(pane: ChatPane, close: @escaping () -> Void) {
    self.init(pane: pane, close: close) { EmptyView() }
  }
}

/// One opened pane: its header and content.
struct ChatPaneContent: View {
  let pane: ChatPane
  let context: ChatPaneContext
  let sources: ChatPaneSources
  @Environment(\.openURL) private var openURL

  var body: some View {
    VStack(spacing: 0) {
      PaneHeader(pane: pane, close: { context.close(pane.id) }) { actions }
      content.frame(maxWidth: .infinity, maxHeight: .infinity)
    }
    .background(AppTheme.surface)
  }

  @ViewBuilder private var actions: some View {
    if pane.kind == .pullRequest, let url = pane.target.flatMap(URL.init(string:)) {
      Button {
        openURL(url)
      } label: {
        Image(systemName: "safari").font(.system(size: 15)).frame(width: 32, height: 32)
      }
      .buttonStyle(.plain).foregroundStyle(.secondary)
      .accessibilityLabel("Open on GitHub")
    }
  }

  @ViewBuilder private var content: some View {
    let workspace = context.workspace
    switch pane.kind {
    case .threads:
      if let zapper = context.zapper { ZapperThreadsPane(context: context, zapper: zapper) }
    case .pullRequests:
      if let zapper = context.zapper { ZapperPullRequestsPane(context: context, zapper: zapper) }
    case .issues:
      if let zapper = context.zapper { ZapperIssuesPane(context: context, zapper: zapper) }
    case .terminal:
      TerminalPaneView(
        model: context.store.terminal(pane, chat: context.chatID), rpc: context.rpc,
        preview: context.preview)
    case .files:
      FilesPaneView(
        model: context.store.fileTree(pane, chat: context.chatID), rpc: context.rpc,
        preview: context.preview
      ) { entry in
        context.open(.file, target: entry.path, title: entry.name)
      }
    case .docs:
      if let zapper = context.zapper {
        ZapperDocsPane(context: context, zapper: zapper, documents: sources.documents)
      }
    case .overview:
      if let zapper = context.zapper {
        ZapperDetailView(
          workspace: workspace, zapperID: zapper.id, push: context.push,
          close: context.closeZapper, sidePanel: true)
      }
    case .changes:
      if let rpc = context.rpc {
        DiffView(rpc: rpc, session: context.session, embedded: true)
      } else {
        PaneUnavailable(
          symbol: "plusminus", text: "Connect to your computer to see this chat’s changes.")
      }
    case .pullRequest:
      if let link = pane.target.flatMap(PullRequestLink.init) {
        PullRequestDetailView(store: workspace.pullRequests, link: link, embedded: true)
      }
    case .file:
      if let path = pane.target {
        FileContentsView(
          rpc: context.rpc, session: context.session, path: path, embedded: true,
          previewText: context.preview ? Self.sampleFile : nil)
      }
    case .document:
      if let zapper = context.zapper, let path = pane.target {
        ZapperDocumentView(
          store: workspace.zappers, zapperID: zapper.id, path: path, embedded: true)
      }
    }
  }

  private static let sampleFile = """
    import SwiftUI

    /// The chat page: its transcript first, opened panes after it and the picker last.
    struct ChatPanes: View {
      let chat: String
      var body: some View {
        Text(chat)
      }
    }
    """
}

struct PaneUnavailable: View {
  let symbol: String
  let text: String
  var body: some View {
    VStack(spacing: 10) {
      Image(systemName: symbol).font(.system(size: 28)).foregroundStyle(.tertiary)
      Text(text).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
    }
    .padding(32).frame(maxWidth: .infinity, maxHeight: .infinity)
  }
}

// MARK: - List panes

/// The pull requests a Zapper's issues link to, newest issue first, each once.
enum ZapperPullRequests {
  struct Item: Identifiable, Hashable {
    let url: String
    let title: String
    let issue: String
    var id: String { url }
  }

  static func items(_ zapper: Zapper) -> [Item] {
    var seen = Set<String>()
    var items: [Item] = []
    for issue in zapper.issues.sorted(by: { $0.updatedAt > $1.updatedAt }) {
      for pull in issue.pullRequests where seen.insert(pull.url).inserted {
        items.append(Item(url: pull.url, title: pull.title, issue: issue.code))
      }
    }
    return items
  }
}

/// One set of metrics every list pane (Files, Issues, Threads, Pull requests, Docs) shares, so
/// they read tight and line up: a compact row height, a single left inset, and a search field
/// with matching margins.
enum PaneMetrics {
  /// Row tap target: compact, never under the 44pt minimum.
  static let rowHeight: CGFloat = 44
  /// The left/right inset every row, header and the search field align to.
  static let inset: CGFloat = 16
  /// The gap under the search field before the list starts.
  static let searchGap: CGFloat = 8
  /// The search field's height.
  static let searchHeight: CGFloat = 40
  /// The icon column every row leads with, and its gap to the text.
  static let leading: CGFloat = 22
  static let gap: CGFloat = 10
  static let titleSize: CGFloat = 15
  static let detailSize: CGFloat = 13
}

private struct PaneList<Row: View, Empty: View>: View {
  @Binding var query: String
  let count: Int
  @ViewBuilder let rows: Row
  @ViewBuilder let empty: Empty
  var body: some View {
    VStack(spacing: 0) {
      PaneSearchField(text: $query)
        .padding(.horizontal, PaneMetrics.inset).padding(.bottom, PaneMetrics.searchGap)
      if count == 0 {
        empty.frame(maxWidth: .infinity, maxHeight: .infinity)
      } else {
        List { rows }.listStyle(.plain).scrollContentBackground(.hidden)
      }
    }
  }
}

private struct PaneRow<Leading: View>: View {
  let title: String
  let detail: String
  @ViewBuilder let leading: Leading
  var body: some View {
    HStack(spacing: PaneMetrics.gap) {
      leading.frame(width: PaneMetrics.leading)
      VStack(alignment: .leading, spacing: 2) {
        Text(title).font(.system(size: PaneMetrics.titleSize)).foregroundStyle(.primary)
          .lineLimit(1)
        if !detail.isEmpty {
          Text(detail).font(.system(size: PaneMetrics.detailSize)).foregroundStyle(.secondary)
            .lineLimit(1)
        }
      }
      Spacer(minLength: 0)
    }
    .frame(minHeight: PaneMetrics.rowHeight)
    .contentShape(Rectangle())
    .accessibilityElement(children: .combine)
  }
}

extension View {
  /// The shared list-row chrome: clear background, no separators, one left/right inset.
  fileprivate func paneRow() -> some View {
    listRowBackground(Color.clear)
      .listRowSeparator(.hidden)
      .listRowInsets(
        EdgeInsets(top: 0, leading: PaneMetrics.inset, bottom: 0, trailing: PaneMetrics.inset))
  }
}

private func matches(_ query: String, _ text: String...) -> Bool {
  let query = query.trimmingCharacters(in: .whitespaces)
  return query.isEmpty || text.contains { $0.localizedCaseInsensitiveContains(query) }
}

/// A Zapper's threads; each opens its conversation.
struct ZapperThreadsPane: View {
  let context: ChatPaneContext
  let zapper: Zapper
  @State private var query = ""
  var body: some View {
    let store = context.workspace.zappers
    let threads = store.orderedThreads(zapper).filter {
      matches(query, store.title(of: $0.chatId, fallback: $0.objective), $0.objective)
    }
    PaneList(query: $query, count: threads.count) {
      ForEach(threads) { worker in
        let rank = store.rank(of: worker)
        let title = store.title(of: worker.chatId, fallback: worker.objective)
        Button {
          guard !worker.queued else { return }
          context.push(
            .chat(
              store.session(
                for: worker.chatId, fallbackTitle: worker.objective,
                known: context.workspace.sessions + context.workspace.archivedSessions)))
        } label: {
          PaneRow(
            title: title,
            detail: title == worker.objective ? rank.label : rank.label + " · " + worker.objective
          ) {
            ZapperThreadMark(rank: rank)
          }
        }
        .buttonStyle(.plain)
        .paneRow()
      }
    } empty: {
      PaneUnavailable(
        symbol: "bubble.left.and.bubble.right",
        text: query.isEmpty ? "No threads yet." : "No threads match “\(query)”.")
    }
  }
}

/// Pull requests linked from a Zapper's issues; each opens as its own pane.
struct ZapperPullRequestsPane: View {
  let context: ChatPaneContext
  let zapper: Zapper
  @State private var query = ""
  @Environment(\.openURL) private var openURL
  var body: some View {
    let store = context.workspace.pullRequests
    let items = ZapperPullRequests.items(zapper).filter { matches(query, $0.title, $0.url) }
    PaneList(query: $query, count: items.count) {
      ForEach(items) { item in
        let link = PullRequestLink(item.url)
        let known = store.known[item.url]
        Button {
          if let link, store.available || context.preview {
            context.open(.pullRequest, target: item.url, title: "#\(link.number)")
          } else if let url = URL(string: item.url) {
            openURL(url)
          }
        } label: {
          PaneRow(
            title: item.title.isEmpty ? (link?.shortLabel ?? item.url) : item.title,
            detail: [link?.shortLabel, item.issue].compactMap { $0 }.joined(separator: " · ")
          ) {
            PullRequestGlyph(
              state: known?.state ?? .open, draft: known?.isDraft ?? false, size: 15)
          }
        }
        .buttonStyle(.plain)
        .paneRow()
      }
    } empty: {
      PaneUnavailable(symbol: "arrow.triangle.pull", text: "No pull requests match.")
    }
  }
}

/// A Zapper's issues by status; finished groups fold away.
struct ZapperIssuesPane: View {
  let context: ChatPaneContext
  let zapper: Zapper
  @State private var query = ""
  @State private var opened: Set<ZapperIssueStatus> = []
  var body: some View {
    let board = zapper.issueBoard().map { group in
      (
        status: group.status,
        issues: group.issues.filter { matches(query, $0.details.title, $0.code) }
      )
    }.filter { !$0.issues.isEmpty }
    PaneList(query: $query, count: board.count) {
      ForEach(board, id: \.status) { group in
        let shows = !group.status.terminal || opened.contains(group.status) || !query.isEmpty
        Section {
          if shows {
            ForEach(group.issues) { issue in
              Button {
                context.push(.issue(zapper: zapper.id, issue: issue.id))
              } label: {
                IssueListRow(issue: issue, zapper: zapper, account: context.workspace.account)
                  .contentShape(Rectangle())
              }
              .buttonStyle(.plain)
              .paneRow()
            }
          }
        } header: {
          Button {
            guard group.status.terminal else { return }
            withAnimation(.snappy) {
              if opened.remove(group.status) == nil { opened.insert(group.status) }
            }
          } label: {
            HStack(spacing: 6) {
              Text(group.status.label)
              Text("\(group.issues.count)").monospacedDigit().foregroundStyle(.tertiary)
              if group.status.terminal && query.isEmpty {
                Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold))
                  .rotationEffect(.degrees(shows ? 90 : 0))
              }
            }
            .font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
          }
          .buttonStyle(.plain)
          .accessibilityAddTraits(group.status.terminal ? .isButton : .isHeader)
        }
      }
    } empty: {
      PaneUnavailable(
        symbol: "circle.dashed",
        text: query.isEmpty ? "No issues for \(zapper.config.name) yet." : "No issues match.")
    }
  }
}

/// A Zapper's Drive documents; each opens as its own pane.
struct ZapperDocsPane: View {
  let context: ChatPaneContext
  let zapper: Zapper
  let documents: [ZapperFile]
  @State private var query = ""
  var body: some View {
    let shown = documents.filter { matches(query, ZapperDrive.title($0.path), $0.content) }
    PaneList(query: $query, count: shown.count) {
      ForEach(shown) { file in
        Button {
          context.open(.document, target: file.path, title: ZapperDrive.title(file.path))
        } label: {
          PaneRow(title: ZapperDrive.title(file.path), detail: ZapperDrive.preview(file.content)) {
            Image(systemName: "doc.richtext").font(.system(size: 15)).foregroundStyle(.secondary)
          }
        }
        .buttonStyle(.plain)
        .paneRow()
      }
    } empty: {
      PaneUnavailable(symbol: "doc.richtext", text: "Nothing in Drive yet.")
    }
  }
}

// MARK: - Host

/// A chat page with its panes: the chat itself first, opened panes after it, the picker last.
struct ChatPaneHost<Main: View>: View {
  let context: ChatPaneContext
  @ViewBuilder let main: () -> Main
  @State private var sources = ChatPaneSources()

  var body: some View {
    let store = context.store
    let deck = store.deck(context.chatID)
    ChatPanePager(store: store, chatID: context.chatID, label: label) { page in
      switch page {
      case .main: main()
      case .pane(let pane): ChatPaneContent(pane: pane, context: context, sources: sources)
      case .selection: PaneSelectionView(context: context, sources: sources)
      }
    }
    // The picker's counts refresh whenever it comes into view.
    .task(id: refreshKey(deck)) {
      store.rpc = context.rpc
      sources = await ChatPaneSources.load(context)
    }
    #if DEBUG
      .task { seedPreview() }
    #endif
  }

  private func refreshKey(_ deck: ChatPaneDeck) -> String {
    [
      context.rpc.map { "\(ObjectIdentifier($0).hashValue)" } ?? "offline",
      context.connected ? "connected" : "offline",
      context.store.isOverview(context.chatID) ? "overview" : "pages",
      deck.focus == ChatPaneDeck.selectionID ? "picker" : "",
      "\(context.zapper?.revision ?? 0)",
    ].joined(separator: "|")
  }

  private func label(_ page: ChatPanePage) -> (title: String, symbol: String)? {
    switch page {
    case .main:
      return (context.zapper?.config.name ?? context.session.title, "bubble.left")
    case .pane(let pane): return (pane.title, pane.kind.symbol)
    case .selection: return nil
    }
  }

  #if DEBUG
    private func seedPreview() {
      var kinds: [(ChatPaneKind, String?, String)] = []
      if context.preview, !context.workspace.pullRequests.preview {
        context.workspace.pullRequests.loadSamplePreview()
      }
      if context.zapper != nil {
        kinds = [
          (.threads, nil, "Threads"), (.pullRequests, nil, "Pull requests"),
          (.issues, nil, "Issues"), (.terminal, nil, "Terminal"), (.files, nil, "Files"),
        ]
        if context.preview {
          kinds += [(.docs, nil, "Docs"), (.document, "NOTES.md", "Notes")]
        }
      } else {
        kinds = [(.terminal, nil, "Terminal"), (.files, nil, "Files")]
        if context.preview {
          kinds += [
            (.pullRequest, PullRequestPreview.readyURL, "Pull request #370"),
            (.file, "apps/ios/App/ChatPanes.swift", "ChatPanes.swift"),
          ]
        } else if context.showsChanges {
          kinds.insert((.changes, nil, "Changes"), at: 0)
        }
      }
      context.store.seedPreview(context.chatID, kinds: kinds)
    }
  #endif
}

#if DEBUG
  /// Offline samples for `--panes-preview`.
  enum PanePreview {
    /// The sample chat's checkout pull request.
    static let pullRequest = try? JSONValue.object([
      "number": .number(370), "title": .string("Preview HTML files from the remote computer"),
      "url": .string(PullRequestPreview.readyURL), "state": .string("OPEN"),
      "isDraft": .bool(false), "baseRef": .string("main"), "headRef": .string("remote-preview"),
    ]).decode(PullRequestCheckoutSummary.self)
  }
#endif
