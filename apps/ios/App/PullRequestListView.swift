import SwiftUI
import ZuseCore

/// The open computer's pull requests: every repository its projects point at, merged into one
/// list, grouped the way the desktop board groups them.
@MainActor @Observable final class PullRequestListModel {
  private(set) var items: [PullRequestListItem] = []
  private(set) var total: Int?
  private(set) var loading = false
  private(set) var loadingMore = false
  private(set) var error: String?
  /// Repositories that failed while others loaded.
  private(set) var failures: [String: String] = [:]
  private var cursors: [String: String] = [:]
  private var generation = 0
  let store: PullRequestStore
  init(store: PullRequestStore) { self.store = store }

  var hasMore: Bool { !cursors.isEmpty }

  /// Shows the engine's stored first pages at once, then reads them again when they've aged
  /// past a minute. `refresh` always asks the provider.
  func load(
    filter: PullRequestFilter, repository: String?, refresh: Bool = false, cached: Bool = true
  ) async {
    generation += 1
    let current = generation
    let repositories = scope(repository)
    loading = true
    error = nil
    cursors = [:]
    defer { if current == generation { loading = false } }
    guard !repositories.isEmpty else {
      items = []
      total = 0
      return
    }
    var loaded: [PullRequestListItem] = []
    var counts = 0
    var nextCursors: [String: String] = [:]
    var failed: [String: String] = [:]
    var stale = false
    let useCache = cached && !refresh
    await withTaskGroup(of: (String, Result<PullRequestPage, Error>).self) { group in
      for slug in repositories {
        group.addTask { [store] in
          do {
            let page = try await store.page(
              slug, filter: filter, refresh: refresh, cached: useCache)
            return (slug, .success(page))
          } catch { return (slug, .failure(error)) }
        }
      }
      for await (slug, result) in group {
        switch result {
        case .success(let page):
          loaded += page.items
          counts += page.totalCount ?? page.items.count
          if let cursor = page.nextCursor { nextCursors[slug] = cursor }
          if useCache, let fetched = page.fetchedAt, Date().timeIntervalSince(fetched) > 60 {
            stale = true
          }
        case .failure(let error):
          failed[slug] = PullRequestErrorText.readable(error)
        }
      }
    }
    guard current == generation, !Task.isCancelled else { return }
    items = Self.sorted(loaded, filter: filter)
    total = counts
    cursors = nextCursors
    failures = failed
    if loaded.isEmpty, let first = failed.values.first { error = first }
    if stale {
      loading = false
      await load(filter: filter, repository: repository, cached: false)
    }
  }

  func loadMore(filter: PullRequestFilter) async {
    guard !loading, !loadingMore, !cursors.isEmpty else { return }
    loadingMore = true
    defer { loadingMore = false }
    let current = generation
    for (slug, cursor) in cursors {
      do {
        let page = try await store.page(slug, filter: filter, after: cursor)
        guard current == generation else { return }
        let known = Set(items.map(\.url))
        items = Self.sorted(items + page.items.filter { !known.contains($0.url) }, filter: filter)
        cursors[slug] = page.nextCursor
      } catch {
        guard current == generation, !Task.isCancelled else { return }
        failures[slug] = PullRequestErrorText.readable(error)
        cursors[slug] = nil
      }
    }
  }

  private func scope(_ repository: String?) -> [String] {
    let all = store.repositories.map(\.slug)
    guard let repository else { return all }
    return all.filter { $0 == repository }
  }

  private static func sorted(_ items: [PullRequestListItem], filter: PullRequestFilter)
    -> [PullRequestListItem]
  {
    items.sorted { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
  }
}

struct PullRequestListView: View {
  let store: PullRequestStore
  /// The pull request open beside the list, highlighted in it.
  var selectedURL: String?
  let open: (PullRequestLink) -> Void
  @State private var model: PullRequestListModel
  @AppStorage("native.pullRequests.filter") private var filterRaw = PullRequestFilter.all.rawValue
  @State private var repository: String?
  private var isTablet: Bool {
    #if os(iOS)
      UIDevice.current.userInterfaceIdiom == .pad
    #else
      true
    #endif
  }

  init(
    store: PullRequestStore, selectedURL: String? = nil,
    open: @escaping (PullRequestLink) -> Void
  ) {
    self.store = store
    self.selectedURL = selectedURL
    self.open = open
    _model = State(initialValue: PullRequestListModel(store: store))
  }

  private var filter: PullRequestFilter { PullRequestFilter(rawValue: filterRaw) ?? .all }

  var body: some View {
    List {
      filterBar
        .listRowInsets(EdgeInsets(top: 2, leading: 16, bottom: 10, trailing: 16))
        .listRowSeparator(.hidden).listRowBackground(Color.clear)
      content
    }
    .listStyle(.plain)
    .environment(\.defaultMinListRowHeight, 20)
    .scrollContentBackground(.hidden)
    .contentMargins(.horizontal, isTablet ? 8 : 0, for: .scrollContent)
    .background(AppTheme.surface)
    .navigationTitle("Pull requests")
    .compactNavigationTitle()
    .task(id: "\(filterRaw)|\(repository ?? "")|\(store.repositories.map(\.slug))") {
      await model.load(filter: filter, repository: repository)
      updateOpenCount()
    }
    .refreshable {
      await store.resolveRepositories()
      await model.load(filter: filter, repository: repository, refresh: true)
      updateOpenCount()
    }
  }

  /// The home row's count follows the freshest full open listing.
  private func updateOpenCount() {
    guard filter == .all, repository == nil, model.failures.isEmpty, let total = model.total
    else { return }
    store.setOpenCount(total)
  }

  // MARK: Filters

  private var filterBar: some View {
    HStack(spacing: 8) {
      Menu {
        Picker("Show", selection: $filterRaw) {
          ForEach(PullRequestFilter.allCases) { option in
            Text(option.label).tag(option.rawValue)
          }
        }
      } label: {
        HStack(spacing: 5) {
          Text(filter.label)
          Image(systemName: "chevron.down").font(.system(size: 10, weight: .bold))
        }
        .font(.system(size: 15, weight: .semibold))
        .foregroundStyle(AppTheme.surface)
        .padding(.horizontal, 14).frame(height: 36)
        .background(Color.primary, in: Capsule())
        .contentShape(Capsule())
      }
      .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
      .accessibilityLabel("Showing \(filter.label)")
      .accessibilityIdentifier("pr-filter")
      if store.repositories.count > 1 {
        Menu {
          Picker("Repository", selection: $repository) {
            Text("All repositories").tag(String?.none)
            ForEach(store.repositories) { repository in
              Text(repository.slug).tag(Optional(repository.slug))
            }
          }
        } label: {
          HStack(spacing: 5) {
            Image(systemName: repository == nil ? "plus" : "folder")
              .font(.system(size: 12, weight: .semibold))
            Text(
              repository.map { $0.split(separator: "/").last.map(String.init) ?? $0 }
                ?? "Repository"
            )
            .lineLimit(1)
          }
          .font(.system(size: 15, weight: .medium))
          .foregroundStyle(.primary)
          .padding(.horizontal, 14).frame(height: 36)
          .background(AppTheme.card, in: Capsule())
          .overlay { Capsule().strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5) }
          .contentShape(Capsule())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden)
      }
      Spacer(minLength: 0)
      if model.loading && !model.items.isEmpty { ProgressView().controlSize(.small) }
    }
  }

  // MARK: Content

  @ViewBuilder private var content: some View {
    if store.repositories.isEmpty {
      Group {
        if store.resolvingRepositories {
          ProgressView().frame(maxWidth: .infinity).padding(.top, 80)
        } else {
          PullRequestUnavailable(
            title: "No GitHub projects",
            message:
              "Add a project that’s a GitHub repository on your computer to see its pull requests here.",
            symbol: "folder.badge.questionmark"
          )
          .padding(.top, 40)
        }
      }
      .listRowSeparator(.hidden).listRowBackground(Color.clear)
    } else if model.items.isEmpty {
      Group {
        if model.loading {
          ProgressView().frame(maxWidth: .infinity).padding(.top, 80)
        } else if let error = model.error {
          PullRequestUnavailable(
            title: "Couldn’t load pull requests", message: error, symbol: "exclamationmark.triangle"
          ) {
            Task { await model.load(filter: filter, repository: repository, refresh: true) }
          }
          .padding(.top, 40)
        } else {
          PullRequestUnavailable(title: emptyTitle, message: emptyMessage).padding(.top, 40)
        }
      }
      .listRowSeparator(.hidden).listRowBackground(Color.clear)
    } else {
      ForEach(PullRequestSection.group(model.items), id: \.section) { group in
        Section {
          ForEach(group.items) { item in
            PullRequestRow(
              item: item, attention: group.section == .attention, showsOwner: showsOwner
            )
            .contentShape(Rectangle())
            .onTapGesture { if let link = PullRequestLink(item.url) { open(link) } }
            .accessibilityAddTraits(.isButton)
            .listRowInsets(EdgeInsets(top: 7, leading: 16, bottom: 7, trailing: 16))
            .listRowSeparator(.hidden)
            .listRowBackground(SelectedRowBackground(selected: item.url == selectedURL))
            .contextMenu { rowMenu(item) }
          }
        } header: {
          HStack(spacing: 6) {
            Text(group.section.label).foregroundStyle(.secondary)
            Text("\(group.items.count)").foregroundStyle(.tertiary).monospacedDigit()
          }
          .font(.system(size: 13, weight: .medium))
          .textCase(nil)
          .padding(.leading, 0).padding(.top, 8).padding(.bottom, 2)
          .frame(maxWidth: .infinity, alignment: .leading)
          .background(AppTheme.surface)
          .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
        }
      }
      footer.listRowSeparator(.hidden).listRowBackground(Color.clear)
    }
  }

  @ViewBuilder private var footer: some View {
    VStack(spacing: 10) {
      ForEach(model.failures.sorted(by: { $0.key < $1.key }), id: \.key) { slug, message in
        Label("\(slug): \(message)", systemImage: "exclamationmark.triangle")
          .font(.footnote).foregroundStyle(.secondary)
          .frame(maxWidth: .infinity, alignment: .leading)
      }
      if model.hasMore {
        Button {
          Task { await model.loadMore(filter: filter) }
        } label: {
          Group {
            if model.loadingMore {
              ProgressView().controlSize(.small)
            } else {
              Text(moreLabel)
            }
          }
          .font(.system(size: 15, weight: .medium)).frame(maxWidth: .infinity).frame(height: 40)
        }
        .buttonStyle(.plain)
        .background(AppTheme.card, in: Capsule())
        .disabled(model.loadingMore)
      }
    }
    .padding(.horizontal, 16).padding(.vertical, 12)
  }

  /// `owner/name#1` only when owners differ; one owner reads as `name#1`.
  private var showsOwner: Bool {
    Set(store.repositories.map { $0.slug.split(separator: "/").first ?? "" }).count > 1
  }

  private var moreLabel: String {
    if let total = model.total, total > model.items.count {
      return "Load \(total - model.items.count) more"
    }
    return "Load more"
  }

  @ViewBuilder private func rowMenu(_ item: PullRequestListItem) -> some View {
    if let url = URL(string: item.url), url.scheme == "https" {
      Link(destination: url) { Label("Open on GitHub", systemImage: "arrow.up.right.square") }
      Button("Copy link", systemImage: "link") { NativePlatform.copy(item.url) }
      ShareLink(item: url)
    }
  }

  private var emptyTitle: String {
    switch filter {
    case .all: return "No open pull requests"
    case .authored: return "Nothing open from you"
    case .reviewing: return "No reviews waiting"
    case .merged: return "Nothing merged yet"
    case .allStates: return "No pull requests"
    }
  }
  private var emptyMessage: String {
    let place =
      repository ?? (store.repositories.count == 1 ? store.repositories[0].slug : "your projects")
    switch filter {
    case .reviewing: return "When someone asks for your review in \(place), it shows up here."
    default: return "Pull requests in \(place) show up here."
    }
  }
}

/// One pull request: state mark, title, then author, repository and number, and age.
struct PullRequestRow: View {
  let item: PullRequestListItem
  var attention = false
  var showsOwner = true
  var body: some View {
    HStack(alignment: .top, spacing: 12) {
      PullRequestGlyph(state: item.state, draft: item.isDraft, size: 16)
        .frame(width: 20, height: 22)
      VStack(alignment: .leading, spacing: 3) {
        Text(item.title.isEmpty ? "Untitled pull request" : item.title)
          .font(.system(size: 16, weight: .medium)).foregroundStyle(.primary).lineLimit(1)
        HStack(spacing: 5) {
          PullRequestAvatar(actor: item.author, size: 16)
          Text(meta).foregroundStyle(.secondary).lineLimit(1)
          if attention, let status = PullRequestRowStatus.attention(item) {
            Text("·").foregroundStyle(.tertiary)
            Text(status).foregroundStyle(PullRequestTint.attention).lineLimit(1)
              .layoutPriority(1)
          }
        }
        .font(.system(size: 13))
      }
      Spacer(minLength: 0)
    }
    .accessibilityElement(children: .combine)
  }
  private var meta: String {
    let repository =
      showsOwner ? item.repository : String(item.repository.split(separator: "/").last ?? "")
    return [
      item.author.displayName, "\(repository)#\(item.number)", PullRequestAge.short(item.updatedAt),
    ]
    .filter { !$0.isEmpty }.joined(separator: " · ")
  }
}
