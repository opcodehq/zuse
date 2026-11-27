import SwiftUI
import ZuseCore

/// Pages pushed for Zappers. Each carries its Zapper, so any stack can show them.
enum ZapperRoute: Hashable {
  case overview(String)
  case chat(SessionRow)
  case issue(zapper: String, issue: String)
  case document(zapper: String, path: String)
}

enum ZapperSection: String, CaseIterable, Identifiable {
  case overview, issues, threads, drive
  var id: String { rawValue }
  var title: String {
    switch self {
    case .overview: return "Overview"
    case .issues: return "Issues"
    case .threads: return "Threads"
    case .drive: return "Drive"
    }
  }
}

/// The page for one Zapper route, in whichever stack pushed it.
struct ZapperDestination<Chat: View>: View {
  let workspace: WorkspaceModel
  let route: ZapperRoute
  let push: (ZapperRoute) -> Void
  let close: () -> Void
  @ViewBuilder let chat: (SessionRow) -> Chat
  var body: some View {
    switch route {
    case .overview(let id):
      ZapperDetailView(workspace: workspace, zapperID: id, push: push, close: close)
    case .chat(let session): chat(session)
    case .issue(let zapper, let issue):
      ZapperIssueView(workspace: workspace, zapperID: zapper, issueID: issue, push: push)
    case .document(let zapper, let path):
      ZapperDocumentView(store: workspace.zappers, zapperID: zapper, path: path)
    }
  }
}

/// A Zapper's own navigation stack. It opens on the coordinator's conversation; its title in
/// the header opens the overview, with threads, issues and docs.
struct ZapperNavigator<Chat: View>: View {
  let workspace: WorkspaceModel
  let zapperID: String
  @Binding var path: [ZapperRoute]
  /// Selects a conversation before its transcript is shown.
  let select: (SessionRow) -> Void
  let close: () -> Void
  /// Dresses each page pushed on the stack, like the wide layout's overview inspector.
  var page: ((AnyView) -> AnyView)?
  @ViewBuilder let chat: (SessionRow) -> Chat
  private var store: ZapperStore { workspace.zappers }
  var body: some View {
    NavigationStack(path: $path) {
      root
        .navigationDestination(for: ZapperRoute.self) { route in
          dressed(
            ZapperDestination(
              workspace: workspace, route: route, push: push, close: close, chat: chat))
        }
    }
    .onChange(of: path) { _, path in selectVisibleChat(path) }
  }
  @ViewBuilder private var root: some View {
    if let zapper = store.zapper(zapperID) {
      let session = coordinator(zapper)
      chat(session)
        // A fresh conversation changes the coordinator chat under the same page.
        .task(id: session.id) { if path.isEmpty { select(session) } }
    } else {
      // Still loading, or removed: the overview explains either.
      ZapperDetailView(workspace: workspace, zapperID: zapperID, push: push, close: close)
    }
  }
  @ViewBuilder private func dressed(_ content: some View) -> some View {
    if let page { page(AnyView(content)) } else { content }
  }
  /// The coordinator chat, titled with the Zapper's name.
  private func coordinator(_ zapper: Zapper) -> SessionRow {
    let row = store.session(
      for: zapper.coordinatorChatId, fallbackTitle: zapper.config.name,
      known: workspace.sessions + workspace.archivedSessions)
    return SessionRow(
      id: row.id, projectId: row.projectId, title: zapper.config.name,
      providerId: row.providerId, model: row.model, worktreeId: row.worktreeId)
  }
  private func push(_ route: ZapperRoute) {
    // The coordinator is the root page: going to it goes back rather than stacking a copy.
    if case .chat(let session) = route, session.id == store.zapper(zapperID)?.coordinatorChatId {
      path.removeAll()
      return
    }
    path.append(route)
  }
  /// One transcript loads at a time: keep it on the topmost chat the stack shows.
  private func selectVisibleChat(_ path: [ZapperRoute]) {
    for route in path.reversed() {
      if case .chat(let session) = route {
        select(session)
        return
      }
    }
    if let zapper = store.zapper(zapperID) { select(coordinator(zapper)) }
  }
}

struct ZapperDetailView: View {
  let workspace: WorkspaceModel
  let zapperID: String
  let push: (ZapperRoute) -> Void
  let close: () -> Void
  /// Beside the conversation, as the wide layout's inspector: the page leaves the header's
  /// title and actions to the conversation, which already has them.
  var sidePanel = false
  @State private var section: ZapperSection = .overview
  @State private var events: [ZapperEvent] = []
  @State private var files: [ZapperFile] = []
  @State private var answering: ZapperDecision?
  @State private var messaging: ZapperWorker?
  @State private var confirmation: ManageAction?
  @State private var expandedPurpose = false
  @State private var showsAllReports = false
  @State private var showsClosedIssues = false
  private var store: ZapperStore { workspace.zappers }

  enum ManageAction: Identifiable {
    case stop, archive, delete, fresh
    var id: Self { self }
  }

  var body: some View {
    Group {
      if let zapper = store.zapper(zapperID) {
        content(zapper)
      } else if store.loaded || store.preview {
        ContentUnavailableView {
          Label("Zapper removed", systemImage: "trash")
        } description: {
          Text("This Zapper is no longer on \(workspace.connectedComputer ?? "this computer").")
        } actions: {
          Button("Back to chats", action: close)
        }
      } else {
        ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
      }
    }
    .background(AppTheme.surface)
    .onAppear(perform: applyLaunchPreview)
  }

  private func content(_ zapper: Zapper) -> some View {
    List {
      Section {
        ZapperHero(
          zapper: zapper, status: statusLine(zapper), busy: store.busy[zapper.id],
          expandedPurpose: $expandedPurpose,
          toggle: { status in
            Task {
              await store.perform(
                .setStatus(status), on: zapper, label: status == .paused ? "Pause" : "Resume")
            }
          }
        )
        .listRowBackground(Color.clear).listRowInsets(
          EdgeInsets(top: 8, leading: 4, bottom: 4, trailing: 4))
      }
      if let pending = store.unconfirmed[zapper.id] {
        Section { UnconfirmedRow(store: store, pending: pending) }
      }
      Section {
        Picker("Section", selection: $section) {
          ForEach(ZapperSection.allCases) { section in
            Text(sectionTitle(section, zapper: zapper)).tag(section)
          }
        }
        .pickerStyle(.segmented).labelsHidden()
        .listRowBackground(Color.clear).listRowInsets(
          EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0)
        )
        .accessibilityIdentifier("zapper-sections")
      }
      switch section {
      case .overview: overview(zapper)
      case .issues: issues(zapper)
      case .threads: threads(zapper)
      case .drive: drive(zapper)
      }
    }
    .zapperList()
    .modifier(PageHeader(title: zapper.config.name, enabled: !sidePanel) { manageMenu(zapper) })
    .task(id: "\(zapper.id)|\(zapper.revision)|\(zapper.activityRevision)") {
      await reload(zapper.id)
    }
    .refreshable { await reload(zapper.id) }
    .sheet(item: $answering) { decision in
      ZapperAnswerSheet(
        title: "Answer", prompt: decision.question, placeholder: "Your answer",
        action: "Send answer"
      ) { text in
        var answered = decision
        answered.answer = text
        return await store.perform(.decide(answered), on: zapper, label: "Answer")
      }
    }
    .sheet(item: $messaging) { worker in
      ZapperAnswerSheet(
        title: "Message thread", prompt: store.title(of: worker.chatId, fallback: worker.objective),
        placeholder: "What should this thread do next?", action: "Send",
        footnote: "Your message becomes the thread’s new objective."
      ) { text in
        await store.perform(
          .messageWorker(chatId: worker.chatId, text: text, automatic: false), on: zapper,
          label: "Message")
      }
    }
    .confirmationDialog(
      confirmationTitle(zapper), isPresented: confirmationPresented, titleVisibility: .visible,
      presenting: confirmation
    ) { action in
      Button(confirmationButton(action), role: action == .fresh ? nil : .destructive) {
        Task { await manage(action, zapper: zapper) }
      }
    } message: { action in
      Text(confirmationMessage(action, zapper: zapper))
    }
    .alert(
      "Couldn’t update \(zapper.config.name)", isPresented: failurePresented(zapper.id)
    ) {
      Button("OK") { store.failure.removeValue(forKey: zapper.id) }
    } message: {
      Text(store.failure[zapper.id] ?? "")
    }
  }

  // MARK: Header

  private func statusLine(_ zapper: Zapper) -> String {
    let working = store.workingCount(zapper)
    guard zapper.status == .active else { return zapper.status.label }
    switch working {
    case 0: return "Active"
    case 1: return "1 thread working"
    default: return "\(working) threads working"
    }
  }

  private func threadSession(_ chat: String, fallback: String) -> SessionRow {
    store.session(
      for: chat, fallbackTitle: fallback, known: workspace.sessions + workspace.archivedSessions)
  }

  private func sectionTitle(_ section: ZapperSection, zapper: Zapper) -> String {
    section.title
  }

  // MARK: Overview

  @ViewBuilder private func overview(_ zapper: Zapper) -> some View {
    let asks = zapper.needsYouItems
    if !asks.isEmpty {
      Section {
        ForEach(asks) { item in
          Button {
            switch item {
            case .decision(let decision): answering = decision
            case .issue(let id, _, _, _): push(.issue(zapper: zapper.id, issue: id))
            }
          } label: {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
              Image(
                systemName: item.isDecision
                  ? "questionmark.bubble.fill" : "person.fill.questionmark"
              )
              .foregroundStyle(.orange).frame(width: 20)
              Text(item.title.components(separatedBy: "\n\n")[0]).font(.subheadline)
                .foregroundStyle(.primary)
                .multilineTextAlignment(.leading).lineLimit(4)
              Spacer(minLength: 4)
              Text("Answer").font(.subheadline.weight(.semibold)).foregroundStyle(AppTheme.accent)
            }
            .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
        }
      } header: {
        Text("Needs you").foregroundStyle(.orange)
      }
    }
    let live = store.orderedThreads(zapper).filter { store.rank(of: $0) != .idle }
    if !live.isEmpty {
      Section("In flight") {
        ForEach(live.prefix(4)) { worker in threadRow(worker, zapper: zapper) }
        if live.count > 4 {
          Button("See all \(live.count) threads") { section = .threads }
            .font(.subheadline)
        }
      }
    }
    reports(zapper)
    let open = zapper.issues.filter { !$0.details.status.terminal }
    if !open.isEmpty {
      Section("Issues") {
        ForEach(ZapperIssueStatus.board.filter { !$0.terminal }, id: \.self) { status in
          let count = open.filter { $0.details.status == status }.count
          if count > 0 {
            Button {
              section = .issues
            } label: {
              HStack(spacing: 10) {
                IssueStatusIcon(status: status).frame(width: 20)
                Text(status.label).foregroundStyle(.primary)
                Spacer()
                Text("\(count)").monospacedDigit().foregroundStyle(.secondary)
              }
              .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
          }
        }
      }
    }
    if !zapper.config.repositories.isEmpty {
      Section("Repositories") {
        ForEach(zapper.config.repositories, id: \.self) { repository in
          Label {
            Text(projectName(repository.spaceId))
          } icon: {
            Image(systemName: "folder").foregroundStyle(.secondary)
          }
        }
      }
    }
    Section("Setup") {
      LabeledContent("Coordinator", value: modelLabel(zapper.config.coordinatorModel))
      LabeledContent("Threads at once", value: "\(zapper.config.maxConcurrentWorkers)")
      LabeledContent("Automatic follow-ups", value: "\(zapper.config.maxFollowups)")
    }
  }

  @ViewBuilder private func reports(_ zapper: Zapper) -> some View {
    if !events.isEmpty {
      let newest = Array(events.reversed())
      let shown = showsAllReports ? newest : Array(newest.prefix(5))
      let unread = events.filter { !$0.acknowledged }.map(\.id)
      Section {
        ForEach(shown) { event in
          ReportRow(event: event, source: reportSource(event, zapper: zapper)) {
            Task { await acknowledge(zapper.id, [event.id]) }
            if !event.workerChatId.isEmpty {
              push(.chat(threadSession(event.workerChatId, fallback: "Thread")))
            } else if let code = event.issueCode {
              push(.issue(zapper: zapper.id, issue: code))
            }
          }
          .swipeActions {
            if !event.acknowledged {
              Button("Mark as read", systemImage: "checkmark") {
                Task { await acknowledge(zapper.id, [event.id]) }
              }.tint(AppTheme.accent)
            }
          }
        }
        if newest.count > 5 {
          Button(showsAllReports ? "Show fewer" : "Show all \(newest.count) reports") {
            withAnimation { showsAllReports.toggle() }
          }.font(.subheadline)
        }
      } header: {
        HStack {
          Text("Reports")
          Spacer()
          if !unread.isEmpty {
            Button("Mark all as read") { Task { await acknowledge(zapper.id, unread) } }
              .font(.footnote.weight(.semibold)).textCase(nil).tint(AppTheme.accent)
          }
        }
      }
    }
  }

  // MARK: Threads

  @ViewBuilder private func threads(_ zapper: Zapper) -> some View {
    let ordered = store.orderedThreads(zapper)
    if ordered.isEmpty {
      Section {
        emptyRow(
          "No threads yet", symbol: "bubble.left.and.bubble.right",
          detail: "Ask \(zapper.config.name) to start on something. Its threads show up here.")
      }
    } else {
      Section {
        ForEach(ordered) { worker in threadRow(worker, zapper: zapper) }
      } header: {
        Text("Threads")
      } footer: {
        Text(
          "Up to \(zapper.config.maxConcurrentWorkers) run at once. Swipe or touch and hold a thread to message, stop, release or archive it."
        )
      }
    }
    let finished = zapper.finishedThreads.filter { store.chats[$0]?.archived != true }
    if !finished.isEmpty {
      Section("Finished") {
        ForEach(finished, id: \.self) { chat in
          simpleChatRow(
            chat, title: store.title(of: chat, fallback: "Finished thread"),
            symbol: "checkmark.circle")
        }
      }
    }
    if !zapper.previousCoordinatorChats.isEmpty {
      Section("Earlier conversations") {
        ForEach(zapper.previousCoordinatorChats.reversed(), id: \.self) { chat in
          simpleChatRow(
            chat, title: store.title(of: chat, fallback: zapper.config.name),
            symbol: "clock.arrow.circlepath")
        }
      }
    }
  }

  private func threadRow(_ worker: ZapperWorker, zapper: Zapper) -> some View {
    let rank = store.rank(of: worker)
    let title = store.title(of: worker.chatId, fallback: worker.objective)
    return Button {
      guard !worker.queued else { return }
      push(.chat(threadSession(worker.chatId, fallback: worker.objective)))
    } label: {
      HStack(alignment: .top, spacing: 10) {
        ZapperThreadMark(rank: rank).padding(.top, 2)
        VStack(alignment: .leading, spacing: 2) {
          Text(title).foregroundStyle(.primary).lineLimit(1)
          Text(threadDetail(worker, rank: rank, title: title)).font(.footnote)
            .foregroundStyle(.secondary).lineLimit(2)
        }
        Spacer(minLength: 4)
        if !worker.queued {
          Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
            .foregroundStyle(.tertiary).padding(.top, 3)
        }
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .swipeActions(edge: .trailing) {
      Button("Archive", systemImage: "archivebox") {
        Task {
          await store.perform(.archiveWorker(chatId: worker.chatId), on: zapper, label: "Archive")
        }
      }.tint(.gray)
      Button("Stop", systemImage: "stop.fill") {
        Task { await store.perform(.stopWorker(chatId: worker.chatId), on: zapper, label: "Stop") }
      }.tint(.orange)
    }
    .swipeActions(edge: .leading) {
      Button("Message", systemImage: "arrowshape.turn.up.left.fill") { messaging = worker }
        .tint(AppTheme.accent)
    }
    .contextMenu {
      Button("Message…", systemImage: "arrowshape.turn.up.left") { messaging = worker }
      Button("Stop", systemImage: "stop") {
        Task { await store.perform(.stopWorker(chatId: worker.chatId), on: zapper, label: "Stop") }
      }
      Button("Release from Zapper", systemImage: "arrow.uturn.left") {
        Task {
          await store.perform(.releaseWorker(chatId: worker.chatId), on: zapper, label: "Release")
        }
      }
      Button("Archive", systemImage: "archivebox", role: .destructive) {
        Task {
          await store.perform(.archiveWorker(chatId: worker.chatId), on: zapper, label: "Archive")
        }
      }
    }
  }

  private func threadDetail(_ worker: ZapperWorker, rank: ZapperThreadRank, title: String) -> String
  {
    let state: String
    if let position = worker.queuePosition {
      state = position == 1 ? "Queued · starts next" : "Queued · #\(position) in line"
    } else if rank == .idle, store.chats[worker.chatId]?.lastMessageAt == nil {
      state = "Hasn’t started yet"
    } else {
      state = rank.label
    }
    return title == worker.objective ? state : state + " · " + worker.objective
  }

  private func simpleChatRow(_ chat: String, title: String, symbol: String) -> some View {
    Button {
      push(.chat(threadSession(chat, fallback: title)))
    } label: {
      HStack(spacing: 10) {
        Image(systemName: symbol).foregroundStyle(.secondary).frame(width: 20)
        Text(title).foregroundStyle(.primary).lineLimit(1)
        Spacer(minLength: 4)
        Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
          .foregroundStyle(.tertiary)
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
  }

  // MARK: Issues

  @ViewBuilder private func issues(_ zapper: Zapper) -> some View {
    let board = zapper.issueBoard()
    let open = board.filter { !$0.status.terminal }
    let closed = board.filter { $0.status.terminal }
    if board.isEmpty {
      Section {
        emptyRow(
          "No issues yet", symbol: "checklist",
          detail: "\(zapper.config.name) tracks its work here as it plans.")
      }
    }
    ForEach(open, id: \.status) { group in
      issueGroup(group.status, issues: group.issues, zapper: zapper)
    }
    if !closed.isEmpty {
      let count = closed.reduce(0) { $0 + $1.issues.count }
      Section {
        Button(
          showsClosedIssues
            ? "Hide closed issues" : "Show \(count) closed issue\(count == 1 ? "" : "s")"
        ) {
          withAnimation { showsClosedIssues.toggle() }
        }.font(.subheadline)
      }
      if showsClosedIssues {
        ForEach(closed, id: \.status) { group in
          issueGroup(group.status, issues: group.issues, zapper: zapper)
        }
      }
    }
  }

  private func issueGroup(_ status: ZapperIssueStatus, issues: [ZapperIssue], zapper: Zapper)
    -> some View
  {
    Section {
      ForEach(issues) { issue in
        Button {
          push(.issue(zapper: zapper.id, issue: issue.id))
        } label: {
          IssueListRow(issue: issue, zapper: zapper, account: workspace.account)
        }
        .buttonStyle(.plain)
      }
    } header: {
      HStack(spacing: 6) {
        IssueStatusIcon(status: status, size: 13)
        Text(status.label)
        Text("\(issues.count)").monospacedDigit().foregroundStyle(.tertiary)
      }
    }
  }

  // MARK: Drive

  @ViewBuilder private func drive(_ zapper: Zapper) -> some View {
    let documents = files.filter(ZapperDrive.isDocument)
    if documents.isEmpty {
      Section {
        emptyRow(
          "Nothing in Drive yet", symbol: "folder",
          detail:
            "Notes, the digest and thread rules appear here once \(zapper.config.name) writes them."
        )
      }
    } else {
      Section {
        ForEach(documents) { file in
          Button {
            push(.document(zapper: zapper.id, path: file.path))
          } label: {
            HStack(alignment: .top, spacing: 12) {
              Image(systemName: "doc.text").font(.system(size: 17)).foregroundStyle(.secondary)
                .frame(width: 22).padding(.top, 1)
              VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                  Text(ZapperDrive.title(file.path)).foregroundStyle(.primary)
                  Text("v\(file.version)").font(.caption).monospacedDigit()
                    .foregroundStyle(.tertiary)
                }
                Text(ZapperDrive.preview(file.content)).font(.footnote)
                  .foregroundStyle(.secondary).lineLimit(2)
              }
              Spacer(minLength: 4)
              Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.tertiary).padding(.top, 3)
            }
            .contentShape(Rectangle())
          }
          .buttonStyle(.plain)
        }
      } footer: {
        Text("Every edit keeps a version you can restore.")
      }
    }
  }

  // MARK: Manage

  private func manageMenu(_ zapper: Zapper) -> some View {
    Menu {
      Section {
        if zapper.status == .active {
          Button("Pause", systemImage: "pause") {
            Task { await store.perform(.setStatus(.paused), on: zapper, label: "Pause") }
          }
        } else {
          Button("Resume", systemImage: "play") {
            Task { await store.perform(.setStatus(.active), on: zapper, label: "Resume") }
          }
        }
        Button("Start a fresh conversation", systemImage: "arrow.counterclockwise") {
          confirmation = .fresh
        }
      }
      Section {
        if zapper.status != .stopped && zapper.status != .archived {
          Button("Stop Zapper", systemImage: "stop.circle") { confirmation = .stop }
        }
        if zapper.status != .archived {
          Button("Archive Zapper and threads", systemImage: "archivebox") {
            confirmation = .archive
          }
        }
        Button("Delete Zapper…", systemImage: "trash", role: .destructive) {
          confirmation = .delete
        }
      }
    } label: {
      Label("Manage", systemImage: "ellipsis")
    }
    .tint(.primary)
    .disabled(store.busy[zapper.id] != nil)
    .accessibilityIdentifier("zapper-manage")
  }

  private var confirmationPresented: Binding<Bool> {
    Binding(get: { confirmation != nil }, set: { if !$0 { confirmation = nil } })
  }
  private func confirmationTitle(_ zapper: Zapper) -> String {
    switch confirmation {
    case .stop: return "Stop \(zapper.config.name)?"
    case .archive: return "Archive \(zapper.config.name)?"
    case .delete: return "Delete \(zapper.config.name)?"
    case .fresh: return "Start a fresh conversation?"
    case nil: return ""
    }
  }
  private func confirmationButton(_ action: ManageAction) -> String {
    switch action {
    case .stop: return "Stop Zapper"
    case .archive: return "Archive Zapper and threads"
    case .delete: return "Delete Zapper"
    case .fresh: return "Start fresh"
    }
  }
  private func confirmationMessage(_ action: ManageAction, zapper: Zapper) -> String {
    switch action {
    case .stop:
      return
        "Messages waiting to send are cancelled and running threads are interrupted. You can resume it later."
    case .archive:
      return "Its threads are archived too. You can resume the Zapper later."
    case .delete:
      return "Its threads and documents are kept."
    case .fresh:
      return
        "\(zapper.config.name) keeps its threads, documents and decisions. The current conversation moves to Earlier conversations."
    }
  }
  private func manage(_ action: ManageAction, zapper: Zapper) async {
    switch action {
    case .stop: await store.perform(.setStatus(.stopped), on: zapper, label: "Stop")
    case .archive: await store.perform(.setStatus(.archived), on: zapper, label: "Archive")
    case .fresh:
      await store.perform(.freshConversation, on: zapper, label: "Fresh conversation")
    case .delete:
      if await store.perform(.delete, on: zapper, label: "Delete") { close() }
    }
  }
  private func failurePresented(_ id: String) -> Binding<Bool> {
    Binding(
      get: { store.failure[id] != nil }, set: { if !$0 { store.failure.removeValue(forKey: id) } })
  }

  // MARK: Helpers

  private func reload(_ id: String) async {
    async let loadedEvents = try? store.events(id)
    async let loadedFiles = try? store.files(id)
    let (nextEvents, nextFiles) = await (loadedEvents, loadedFiles)
    if let nextEvents { events = nextEvents }
    if let nextFiles { files = nextFiles }
  }
  private func acknowledge(_ id: String, _ ids: [String]) async {
    await store.acknowledge(id, events: ids)
    events = events.map {
      var event = $0
      if ids.contains(event.id) { event.acknowledged = true }
      return event
    }
  }
  /// Who a report is from: its thread, or the issue it is about.
  private func reportSource(_ event: ZapperEvent, zapper: Zapper) -> String {
    guard !event.workerChatId.isEmpty else { return event.issueCode ?? zapper.config.name }
    return store.title(
      of: event.workerChatId,
      fallback: zapper.workers.first { $0.chatId == event.workerChatId }?.objective ?? "Thread")
  }
  private func projectName(_ spaceID: String) -> String {
    workspace.projects.first { $0.id == spaceID }?.name ?? "Repository"
  }
  private func modelLabel(_ model: ZapperModel) -> String {
    let harness = zapperHarnessName(model.harness)
    guard let name = model.model, !name.isEmpty else { return harness }
    return harness + " · " + name
  }
  private func emptyRow(_ title: String, symbol: String, detail: String) -> some View {
    VStack(spacing: 8) {
      Image(systemName: symbol).font(.system(size: 26)).foregroundStyle(.tertiary)
      Text(title).font(.headline)
      Text(detail).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
    }
    .frame(maxWidth: .infinity).padding(.vertical, 20)
  }

  private func applyLaunchPreview() {
    #if DEBUG
      let arguments = ProcessInfo.processInfo.arguments
      if let index = arguments.firstIndex(of: "--zapper-section"), index + 1 < arguments.count,
        let chosen = ZapperSection(rawValue: arguments[index + 1])
      {
        section = chosen
      }
    #endif
  }
}

/// A page's title and trailing actions, left off where another page owns the header.
private struct PageHeader<Actions: View>: ViewModifier {
  let title: String
  let enabled: Bool
  @ViewBuilder let actions: Actions
  func body(content: Content) -> some View {
    if enabled {
      content
        .navigationTitle(title)
        .compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .primaryAction) { actions }
        }
    } else {
      content
    }
  }
}

extension ZapperNeedsYouItem {
  fileprivate var isDecision: Bool {
    if case .decision = self { return true }
    return false
  }
}

private struct ZapperHero: View {
  let zapper: Zapper
  let status: String
  let busy: String?
  @Binding var expandedPurpose: Bool
  let toggle: (ZapperStatus) -> Void
  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack(spacing: 14) {
        ZapperTile(zapper: zapper, size: 56)
        VStack(alignment: .leading, spacing: 3) {
          Text(zapper.config.name).font(.title3.weight(.bold)).lineLimit(2)
          HStack(spacing: 6) {
            Circle().fill(zapper.status.tint).frame(width: 7, height: 7)
            Text(status).font(.subheadline).foregroundStyle(.secondary)
          }
        }
        Spacer(minLength: 8)
        if zapper.status == .active || zapper.status == .paused {
          let next: ZapperStatus = zapper.status == .active ? .paused : .active
          Button {
            toggle(next)
          } label: {
            Group {
              if busy == "Pause" || busy == "Resume" {
                ProgressView().controlSize(.small)
              } else {
                Label(
                  next == .paused ? "Pause" : "Resume",
                  systemImage: next == .paused ? "pause.fill" : "play.fill")
              }
            }
            .font(.footnote.weight(.semibold)).foregroundStyle(.primary).frame(minWidth: 64)
          }
          .buttonStyle(.bordered).buttonBorderShape(.capsule).controlSize(.small).tint(.secondary)
          .disabled(busy != nil)
          .accessibilityIdentifier("zapper-pause")
        }
      }
      let purpose = zapper.config.purpose.trimmingCharacters(in: .whitespacesAndNewlines)
      if !purpose.isEmpty {
        Text(purpose).font(.subheadline).foregroundStyle(.secondary)
          .lineLimit(expandedPurpose ? nil : 3)
          .onTapGesture { withAnimation(.snappy) { expandedPurpose.toggle() } }
          .accessibilityHint(expandedPurpose ? "" : "Shows the full purpose")
      }
    }
  }
}

private struct ReportRow: View {
  let event: ZapperEvent
  let source: String
  let open: () -> Void
  var body: some View {
    Button(action: open) {
      HStack(alignment: .top, spacing: 10) {
        Circle().fill(event.acknowledged ? Color.clear : AppTheme.accent)
          .frame(width: 8, height: 8).padding(.top, 6)
        VStack(alignment: .leading, spacing: 3) {
          HStack(spacing: 6) {
            Image(systemName: symbol).font(.caption.weight(.semibold)).foregroundStyle(tint)
            Text(event.title).font(.subheadline.weight(event.acknowledged ? .regular : .semibold))
              .foregroundStyle(.primary)
          }
          Text(source).font(.caption).foregroundStyle(.secondary).lineLimit(1)
          let summary =
            (event.issueCode.map { String(event.summary.dropFirst($0.count + 1)) } ?? event.summary)
            .trimmingCharacters(in: .whitespacesAndNewlines)
          if !summary.isEmpty {
            Text(summary).font(.footnote).foregroundStyle(.secondary).lineLimit(3)
              .padding(.top, 1)
          }
        }
        Spacer(minLength: 0)
      }
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityElement(children: .combine)
    .accessibilityAddTraits(event.acknowledged ? [] : .isSelected)
  }
  private var symbol: String {
    switch event.kind {
    case "settled": return "checkmark.circle.fill"
    case "needsInput": return "questionmark.circle.fill"
    case "failed": return "xmark.octagon.fill"
    case "interrupted": return "pause.circle.fill"
    default: return "info.circle.fill"
    }
  }
  private var tint: Color {
    switch event.kind {
    case "settled": return .green
    case "needsInput": return .orange
    case "failed": return .red
    default: return .secondary
    }
  }
}

private struct UnconfirmedRow: View {
  let store: ZapperStore
  let pending: UnconfirmedZapperCommand
  var body: some View {
    HStack(alignment: .top, spacing: 10) {
      Image(systemName: "exclamationmark.arrow.triangle.2.circlepath").foregroundStyle(.orange)
      VStack(alignment: .leading, spacing: 4) {
        Text("Couldn’t confirm “\(pending.label)”").font(.subheadline.weight(.semibold))
        Text(
          "Your computer may already have applied it. Retrying is safe: it sends the same request."
        )
        .font(.footnote).foregroundStyle(.secondary)
        HStack(spacing: 16) {
          Button("Retry") { Task { await store.retry(pending) } }.fontWeight(.semibold)
            .disabled(store.busy[pending.command.zapperId] != nil)
          Button("Dismiss") { store.dismiss(pending) }.foregroundStyle(.secondary)
        }
        .buttonStyle(.borderless).font(.subheadline).padding(.top, 2)
      }
    }
  }
}

/// A single text answer: decisions, thread messages and issue replies.
struct ZapperAnswerSheet: View {
  let title: String
  let prompt: String
  let placeholder: String
  let action: String
  var footnote: String? = nil
  let submit: (String) async -> Bool
  @Environment(\.dismiss) private var dismiss
  @State private var text = ""
  @State private var sending = false
  @FocusState private var focused: Bool
  var body: some View {
    NavigationStack {
      Form {
        Section {
          Text(prompt).font(.body).textSelection(.enabled)
        }
        Section {
          TextField(placeholder, text: $text, axis: .vertical).lineLimit(3...10)
            .focused($focused)
        } footer: {
          if let footnote { Text(footnote) }
        }
      }
      .formStyle(.grouped)
      .navigationTitle(title)
      .compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
        ToolbarItem(placement: .confirmationAction) {
          Button(action) {
            sending = true
            Task {
              let sent = await submit(text.trimmingCharacters(in: .whitespacesAndNewlines))
              sending = false
              if sent { dismiss() }
            }
          }
          .disabled(sending || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
      }
      .onAppear { focused = true }
    }
    .presentationDetents([.medium, .large])
  }
}

extension View {
  /// The grouped list Zapper pages use.
  @ViewBuilder func zapperList() -> some View {
    #if os(iOS)
      listStyle(.insetGrouped).listSectionSpacing(.compact)
        .scrollContentBackground(.hidden).background(AppTheme.surface)
    #else
      listStyle(.inset).scrollContentBackground(.hidden).background(AppTheme.surface)
    #endif
  }
}
