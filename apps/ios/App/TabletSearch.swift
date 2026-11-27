import SwiftUI
import ZuseCore

/// What a search finds across the open computer and Zuse Cloud.
@MainActor struct SearchResults {
  let needle: String
  let zappers: [Zapper]
  let sessions: [SessionRow]
  let clouds: [CloudChat]
  let issues: [IssueEntry]
  let pullRequests: [PullRequestListItem]
  /// Project names by id, for a chat's meta line.
  let projectNames: [String: String]

  init(workspace: WorkspaceModel, query: String) {
    let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
    func matches(_ text: String) -> Bool {
      !needle.isEmpty && text.localizedCaseInsensitiveContains(needle)
    }
    let store = workspace.zappers
    let names = Dictionary(
      workspace.projects.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
    self.needle = needle
    projectNames = names
    zappers = store.zappers.filter { matches($0.config.name) || matches($0.config.purpose) }
    sessions =
      workspace.isCloudActive
      ? []
      : workspace.sessions.filter {
        !store.keepsChat($0.id) && (matches($0.title) || matches(names[$0.projectId] ?? ""))
      }
    clouds = workspace.cloudChats.filter {
      !$0.isArchived && (matches($0.title) || matches($0.repositoryDisplayName))
    }
    issues = IssueBoard.entries(store.zappers).filter {
      matches($0.issue.details.title) || matches($0.issue.code)
    }
    pullRequests = workspace.pullRequests.known.values
      .filter { matches($0.title) || matches("\($0.repository)#\($0.number)") }
      .sorted { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
  }

  var isEmpty: Bool {
    zappers.isEmpty && sessions.isEmpty && clouds.isEmpty && issues.isEmpty
      && pullRequests.isEmpty
  }
}

#if os(iOS)
  /// Search on iPad: a full-width field with a close button, chips that narrow it to one kind,
  /// and results grouped by kind. Before you type it lists your Zappers and recent chats.
  struct TabletSearchView: View {
    let workspace: WorkspaceModel
    @Binding var query: String
    /// Leaves search for the tab you came from.
    let close: () -> Void
    let openZapper: (String) -> Void
    let openChat: (SessionRow) -> Void
    let openCloud: (CloudChat) -> Void
    let openIssue: (IssueEntry) -> Void
    let openPullRequest: (PullRequestLink) -> Void
    @State private var scope: Scope?
    @FocusState private var focused: Bool

    enum Scope: String, CaseIterable, Identifiable {
      case zappers = "Zappers"
      case chats = "Chats"
      case issues = "Issues"
      case pullRequests = "Pull requests"
      var id: String { rawValue }
    }

    /// Most rows a group lists before you type.
    private static let recentLimit = 8

    var body: some View {
      let results = SearchResults(workspace: workspace, query: query)
      List {
        if results.needle.isEmpty {
          recent
        } else if results.isEmpty {
          ContentUnavailableView.search(text: results.needle)
            .chatsRow(minHeight: 0).padding(.top, 40)
        } else {
          found(results)
        }
      }
      .listStyle(.plain)
      .environment(\.defaultMinListRowHeight, 20)
      .scrollContentBackground(.hidden).background(AppTheme.surface)
      .scrollDismissesKeyboard(.immediately)
      .navigationTitle("Search").compactNavigationTitle()
      .toolbar(.hidden, for: .navigationBar)
      .topBar { header }
      .task {
        // Search opens ready to type, unless it opened on a query.
        guard query.isEmpty else { return }
        try? await Task.sleep(for: .milliseconds(350))
        focused = true
      }
    }

    // MARK: Header

    private var header: some View {
      VStack(alignment: .leading, spacing: 12) {
        HStack(spacing: 10) {
          field
          Button(action: close) {
            Image(systemName: "xmark").font(.system(size: 15, weight: .semibold))
              .foregroundStyle(.primary).frame(width: 44, height: 44)
              .background(Color.primary.opacity(0.06), in: Circle())
              .overlay { Circle().strokeBorder(Color.primary.opacity(0.07), lineWidth: 0.5) }
          }
          .buttonStyle(.plain).hoverEffect(.highlight)
          .accessibilityLabel("Close search")
          .accessibilityIdentifier("search-close")
        }
        ScrollView(.horizontal, showsIndicators: false) {
          HStack(spacing: 8) {
            ForEach(Scope.allCases) { option in
              Button {
                scope = scope == option ? nil : option
              } label: {
                FilterChip(title: option.rawValue, prominent: scope == option)
              }
              .buttonStyle(.plain)
              .accessibilityAddTraits(scope == option ? .isSelected : [])
            }
          }
        }
        .scrollClipDisabled()
      }
      .padding(.horizontal, 20).padding(.top, 8).padding(.bottom, 10)
    }

    private var field: some View {
      HStack(spacing: 8) {
        Image(systemName: "magnifyingglass").font(.system(size: 16, weight: .medium))
          .foregroundStyle(.secondary)
        TextField("Search Zappers, chats, issues and pull requests", text: $query)
          .font(.system(size: 17)).focused($focused).submitLabel(.search)
          .autocorrectionDisabled().verbatimInput()
          .accessibilityIdentifier("search-field")
        if !query.isEmpty {
          Button {
            query = ""
            focused = true
          } label: {
            Image(systemName: "xmark.circle.fill").font(.system(size: 16))
              .foregroundStyle(.tertiary)
          }
          .buttonStyle(.plain).accessibilityLabel("Clear text")
        }
      }
      .padding(.horizontal, 16).frame(height: 44)
      .background(Color.primary.opacity(0.06), in: Capsule())
      .overlay { Capsule().strokeBorder(Color.primary.opacity(0.07), lineWidth: 0.5) }
      .contentShape(Capsule())
      .onTapGesture { focused = true }
    }

    // MARK: Before typing

    @ViewBuilder private var recent: some View {
      let store = workspace.zappers
      if shows(.zappers) {
        let zappers = store.zappers.filter { $0.status != .archived }
        if !zappers.isEmpty {
          group("Zappers")
          ForEach(zappers.prefix(Self.recentLimit)) { zapper in zapperRow(zapper) }
        }
      }
      if shows(.chats) {
        let sessions =
          workspace.isCloudActive ? [] : workspace.sessions.filter { !store.keepsChat($0.id) }
        let clouds = workspace.cloudChats.filter { !$0.isArchived }
        if !sessions.isEmpty || !clouds.isEmpty {
          group("Recent chats")
          let names = Dictionary(
            workspace.projects.map { ($0.id, $0.name) }, uniquingKeysWith: { first, _ in first })
          ForEach(sessions.prefix(Self.recentLimit)) { session in
            sessionRow(session, project: names[session.projectId])
          }
          ForEach(clouds.prefix(max(0, Self.recentLimit - sessions.count))) { chat in
            cloudRow(chat)
          }
        }
      }
      if scope == .issues {
        let issues = IssueBoard.entries(store.zappers)
        if !issues.isEmpty {
          group("Issues")
          ForEach(issues.prefix(Self.recentLimit * 2)) { entry in issueRow(entry) }
        }
      }
      if scope == .pullRequests {
        let pulls = workspace.pullRequests.known.values
          .sorted { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
        if !pulls.isEmpty {
          group("Recent pull requests")
          ForEach(pulls.prefix(Self.recentLimit * 2)) { item in pullRow(item) }
        }
      }
    }

    // MARK: Results

    @ViewBuilder private func found(_ results: SearchResults) -> some View {
      if shows(.zappers) && !results.zappers.isEmpty {
        group("Zappers")
        ForEach(results.zappers) { zapper in zapperRow(zapper) }
      }
      if shows(.chats) && (!results.sessions.isEmpty || !results.clouds.isEmpty) {
        group("Chats")
        ForEach(results.sessions) { session in
          sessionRow(session, project: results.projectNames[session.projectId])
        }
        ForEach(results.clouds) { chat in cloudRow(chat) }
      }
      if shows(.issues) && !results.issues.isEmpty {
        group("Issues")
        ForEach(results.issues.prefix(30)) { entry in issueRow(entry) }
      }
      if shows(.pullRequests) && !results.pullRequests.isEmpty {
        group("Pull requests")
        ForEach(results.pullRequests.prefix(30)) { item in pullRow(item) }
      }
    }

    private func shows(_ kind: Scope) -> Bool { scope == nil || scope == kind }

    // MARK: Rows

    private func group(_ title: String) -> some View {
      Text(title).font(.system(size: 14, weight: .medium)).foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, minHeight: 34, alignment: .bottomLeading)
        .padding(.top, 14).padding(.bottom, 2)
        .searchRow()
        .accessibilityAddTraits(.isHeader)
    }

    private func zapperRow(_ zapper: Zapper) -> some View {
      Button {
        openZapper(zapper.id)
      } label: {
        HStack(spacing: 12) {
          ZapperTile(zapper: zapper, size: 26)
          Text(zapper.config.name).font(.system(size: 17)).foregroundStyle(.primary)
            .lineLimit(1)
          if zapper.needsYouCount > 0 { ZapperNeedsYouBadge(count: zapper.needsYouCount) }
          Spacer(minLength: 0)
        }
        .padding(.vertical, 10).contentShape(Rectangle())
      }
      .buttonStyle(.plain).searchRow()
      .accessibilityLabel(zapper.config.name)
    }

    private func sessionRow(_ session: SessionRow, project: String?) -> some View {
      let age = workspace.zappers.chats[session.id]?.lastMessageAt.map { CompactTime.label($0) }
      return Button {
        openChat(session)
      } label: {
        ChatRowLabel(
          title: session.title.isEmpty ? "New chat" : session.title,
          meta: [project ?? "No project", age].compactMap { $0 }.joined(separator: " · ")
        ) {
          ProviderMark(providerID: session.providerId, size: 16).foregroundStyle(.secondary)
        }
      }
      .buttonStyle(.plain).searchRow()
    }

    private func cloudRow(_ chat: CloudChat) -> some View {
      Button {
        openCloud(chat)
      } label: {
        ChatRowLabel(title: chat.title, meta: chat.repositoryDisplayName) { DitherCloudIcon() }
      }
      .buttonStyle(.plain).searchRow()
    }

    private func issueRow(_ entry: IssueEntry) -> some View {
      Button {
        openIssue(entry)
      } label: {
        IssueListRow(
          issue: entry.issue, zapper: workspace.zappers.zapper(entry.zapperID),
          account: workspace.account)
      }
      .buttonStyle(.plain).searchRow()
    }

    private func pullRow(_ item: PullRequestListItem) -> some View {
      Button {
        if let link = PullRequestLink(item.url) { openPullRequest(link) }
      } label: {
        PullRequestRow(item: item).padding(.vertical, 8).contentShape(Rectangle())
      }
      .buttonStyle(.plain).searchRow()
    }
  }

  extension View {
    /// Row metrics for iPad search.
    fileprivate func searchRow() -> some View {
      listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 20))
        .listRowSeparator(.hidden).listRowBackground(Color.clear)
    }
  }
#endif
