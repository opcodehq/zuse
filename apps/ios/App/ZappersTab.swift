import SwiftUI
import ZuseCore

/// The Zappers tab: one row per Zapper, like a message list. Opening one opens its
/// conversation; its threads, issues and docs live on its overview page.
struct ZappersListView: View {
  let workspace: WorkspaceModel
  let open: (String) -> Void
  let connect: () -> Void
  @State private var filter: ArchiveFilter = .active
  /// The open New Zapper sheet, with the idea it starts from when one was picked.
  @State private var creating: NewZapperRequest?
  private var store: ZapperStore { workspace.zappers }

  private var visible: [Zapper] {
    store.zappers.filter { zapper in
      switch filter {
      case .active: return zapper.status != .archived
      case .archived: return zapper.status == .archived
      case .all: return true
      }
    }
  }

  var body: some View {
    List {
      if filter != .active {
        FilterChip(title: filter == .archived ? "Archived" : "All Zappers", prominent: true)
          .onTapGesture { filter = .active }
          .accessibilityHint("Shows active Zappers again")
          .zappersRow().padding(.vertical, 4)
      }
      ForEach(visible) { zapper in
        let coordinator = store.chats[zapper.coordinatorChatId]
        Button {
          open(zapper.id)
        } label: {
          ZapperListRow(
            zapper: zapper, working: store.workingCount(zapper),
            busy: store.busy[zapper.id] != nil, preview: coordinator?.lastMessagePreview,
            updated: coordinator?.lastMessageAt)
        }
        .buttonStyle(.plain)
        .zappersRow()
        .contextMenu { rowActions(zapper) }
      }
      if visible.isEmpty {
        emptyState.zappersRow().padding(.top, firstRun ? 36 : 72).padding(.bottom, 24)
      }
    }
    .listStyle(.plain)
    .contentMargins(.top, 4, for: .scrollContent)
    .scrollContentBackground(.hidden)
    .background(AppTheme.surface)
    .navigationTitle("Zappers")
    .compactNavigationTitle()
    .toolbar {
      ToolbarItemGroup(placement: .primaryAction) {
        Button("New Zapper", systemImage: "plus") { creating = NewZapperRequest() }
          .disabled(!canCreate)
          .accessibilityIdentifier("new-zapper")
        Menu {
          Picker("Show", selection: $filter) {
            Label("Active", image: AppIcon.zapper).tag(ArchiveFilter.active)
            Label("Archived", systemImage: "archivebox").tag(ArchiveFilter.archived)
            Label("All Zappers", systemImage: "tray.full").tag(ArchiveFilter.all)
          }
        } label: {
          Label("Show", systemImage: "ellipsis")
        }
        .accessibilityIdentifier("zappers-filter")
      }
    }
    .tint(.primary)
    .sheet(item: $creating) { request in
      NewZapperSheet(workspace: workspace, starter: request.starter) { id in
        creating = nil
        open(id)
      }
    }
    .refreshable { await workspace.refreshSessionCatalog() }
    .onAppear {
      #if DEBUG
        let arguments = ProcessInfo.processInfo.arguments
        // `--new-zapper-starter=<0-2>` opens the sheet from one of the first-run ideas.
        if let index = arguments.first(where: { $0.hasPrefix("--new-zapper-starter=") })
          .flatMap({ Int($0.dropFirst(21)) }), ZapperStarter.all.indices.contains(index)
        {
          creating = NewZapperRequest(starter: ZapperStarter.all[index])
        }
        if arguments.contains("--new-zapper-preview") && creating == nil {
          creating = NewZapperRequest()
        }
      #endif
    }
  }

  /// Nothing made yet and nothing in the way of making one.
  private var firstRun: Bool {
    filter != .archived && store.zappers.isEmpty && (store.preview || store.available)
      && (store.loaded || store.preview)
      && (store.preview || workspace.connectionState == "Connected")
  }

  private var canCreate: Bool {
    store.preview || (store.available && workspace.connectionState == "Connected")
  }

  @ViewBuilder private func rowActions(_ zapper: Zapper) -> some View {
    if zapper.status == .active || zapper.status == .paused {
      let pausing = zapper.status == .active
      Button(pausing ? "Pause" : "Resume", systemImage: pausing ? "pause" : "play") {
        Task {
          await store.perform(
            .setStatus(pausing ? .paused : .active), on: zapper,
            label: pausing ? "Pause" : "Resume")
        }
      }
    }
    if zapper.status == .archived {
      Button("Restore", systemImage: "arrow.uturn.backward") {
        Task { await store.perform(.setStatus(.active), on: zapper, label: "Resume") }
      }
    }
  }

  @ViewBuilder private var emptyState: some View {
    if filter == .archived {
      ZappersEmptyState(
        symbol: "archivebox", title: "No archived Zappers",
        detail: "Zappers you archive show up here, ready to resume.")
    } else if store.available && !store.loaded {
      LoadingRows().padding(.top, -60)
    } else if workspace.connectionState == "Connected" && !store.available && !store.preview {
      ZappersEmptyState(
        symbol: "arrow.down.circle", title: "Update Zuse to use Zappers",
        detail:
          "\(workspace.connectedComputer ?? "This computer") runs a version of Zuse without Zappers."
      )
    } else if store.zappers.isEmpty && (store.available || store.preview) {
      ZapperFirstRun { idea in
        creating = NewZapperRequest(starter: idea)
      }
    } else if store.available || store.preview {
      ZappersEmptyState(
        symbol: AppIcon.zapper, title: "No active Zappers",
        detail: "Paused and archived Zappers are under All Zappers."
      )
    } else if workspace.isCloudActive {
      ZappersEmptyState(
        symbol: "desktopcomputer", title: "Zappers run on your computer",
        detail: "Open one of your computers to see its Zappers.")
    } else {
      ZappersEmptyState(
        symbol: "desktopcomputer", title: "Zappers run on your computer",
        detail: "Connect your computer to see its Zappers and start new ones."
      ) {
        Button(action: connect) {
          Label("Add computer", systemImage: "plus").frame(maxWidth: 260)
        }
        .buttonStyle(.borderedProminent).tint(AppTheme.accent).controlSize(.large)
        .buttonBorderShape(.capsule)
      }
    }
  }
}

/// A centred empty state with an optional action.
struct ZappersEmptyState<Action: View>: View {
  let symbol: String
  let title: String
  let detail: String
  @ViewBuilder var action: Action
  var body: some View {
    VStack(spacing: 10) {
      AppIcon(name: symbol, size: 34, weight: .light).foregroundStyle(.tertiary)
        .padding(.bottom, 4)
      Text(title).font(.title3.weight(.semibold)).multilineTextAlignment(.center)
      Text(detail).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
        .frame(maxWidth: 320)
      action.padding(.top, 10)
    }
    .frame(maxWidth: .infinity).padding(.horizontal, 24)
  }
}

extension ZappersEmptyState where Action == EmptyView {
  init(symbol: String, title: String, detail: String) {
    self.init(symbol: symbol, title: title, detail: detail) { EmptyView() }
  }
}

/// A Zapper's face on a soft tile of its own colour, or its picture.
struct ZapperTile: View {
  let zapper: Zapper
  var size: CGFloat = 40
  var body: some View {
    let (_, color) = ZapperMascots.resolve(zapper.avatar)
    let shape = RoundedRectangle(cornerRadius: size * 0.27, style: .continuous)
    ZStack {
      shape.fill(Color(hex: color.body).opacity(0.16))
      if zapper.config.avatarImage != nil {
        ZapperAvatarView(zapper: zapper, size: size).clipShape(shape)
      } else {
        ZapperMascotView(
          avatar: zapper.avatar, resting: zapper.status != .active, size: size * 0.72)
      }
    }
    .frame(width: size, height: size)
    .overlay { shape.strokeBorder(Color.primary.opacity(0.06), lineWidth: 0.5) }
    .accessibilityHidden(true)
  }
}

/// A Zapper as a conversation: face, name and time, then the latest message.
struct ZapperListRow: View {
  let zapper: Zapper
  let working: Int
  let busy: Bool
  let preview: String?
  let updated: Date?
  var body: some View {
    HStack(alignment: .center, spacing: 12) {
      ZapperTile(zapper: zapper, size: 40)
      VStack(alignment: .leading, spacing: 2) {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
          Text(zapper.config.name).font(.system(size: 17, weight: .medium)).lineLimit(1)
            .foregroundStyle(.primary)
          Spacer(minLength: 6)
          if let updated {
            Text(CompactTime.label(updated)).font(.system(size: 13))
              .foregroundStyle(.secondary).monospacedDigit()
          }
        }
        HStack(spacing: 6) {
          Text(detail).font(.system(size: 15)).foregroundStyle(.secondary).lineLimit(1)
          Spacer(minLength: 6)
          if busy {
            ProgressView().controlSize(.mini)
          } else if zapper.needsYouCount > 0 {
            ZapperNeedsYouBadge(count: zapper.needsYouCount)
          }
        }
      }
    }
    .padding(.vertical, 8).contentShape(Rectangle())
    .accessibilityElement(children: .combine)
    .accessibilityLabel(accessibilityLabel)
    .accessibilityHint("Opens this Zapper’s conversation")
  }
  /// Paused or archived first, then the latest message, then what is running.
  private var detail: String {
    if zapper.status != .active { return zapper.status.label }
    let text = preview?.split(whereSeparator: \.isNewline).joined(separator: " ")
      .trimmingCharacters(in: .whitespaces)
    if let text, !text.isEmpty { return text }
    if working > 0 { return working == 1 ? "1 thread working" : "\(working) threads working" }
    return zapper.config.purpose.isEmpty ? "No messages yet" : zapper.config.purpose
  }
  private var accessibilityLabel: String {
    var parts = [zapper.config.name, detail]
    let needs = zapper.needsYouCount
    if needs > 0 {
      parts.append(needs == 1 ? "1 thing waiting for you" : "\(needs) things waiting for you")
    }
    return parts.joined(separator: ", ")
  }
}

extension View {
  /// Row metrics for the Zappers list.
  func zappersRow() -> some View {
    listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
      .listRowSeparator(.hidden).listRowBackground(Color.clear)
  }
}

// MARK: - New Zapper

/// One showing of the New Zapper sheet.
struct NewZapperRequest: Identifiable {
  let id = UUID()
  var starter: ZapperStarter? = nil
}

/// Name a Zapper, pick its project and model, and give it a face.
struct NewZapperSheet: View {
  let workspace: WorkspaceModel
  /// An idea from the first-run screen that fills in the name, purpose and face.
  var starter: ZapperStarter? = nil
  let created: (String) -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var draft = ZapperDraft()
  @State private var catalog: [ModelProvider] = []
  @State private var catalogFailed = false
  @State private var creating = false
  @State private var failure: String?
  @FocusState private var nameFocused: Bool

  private var projects: [ProjectRow] {
    workspace.projects.filter { $0.id != CanonicalWorkspace.projectless }
  }
  private var modelKey: Binding<String> {
    Binding(
      get: { (draft.harness ?? "") + "\n" + (draft.model ?? "") },
      set: { value in
        let parts = value.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
        draft.harness = parts.first.map(String.init)
        draft.model = parts.count > 1 ? String(parts[1]) : nil
      })
  }

  var body: some View {
    NavigationStack {
      Form {
        Section {
          VStack(spacing: 18) {
            ZapperFace(avatar: draft.avatar, size: 92)
              .animation(.snappy, value: draft.avatar)
              .accessibilityHidden(true)
            ZapperLookPicker(avatar: $draft.avatar)
              .padding(.horizontal, -20)
          }
          .frame(maxWidth: .infinity).padding(.top, 4).padding(.bottom, 2)
          .listRowBackground(Color.clear)
          .listRowInsets(EdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 20))
        }
        Section {
          TextField("Name it, like Release lead", text: $draft.name)
            .font(.system(size: 17)).focused($nameFocused).submitLabel(.done)
            .accessibilityIdentifier("new-zapper-name")
        } header: {
          Text("Name")
        }
        Section {
          TextField(
            "Keep main green and ship a release every Friday", text: $draft.purpose,
            axis: .vertical
          )
          .lineLimit(3...6)
          .accessibilityIdentifier("new-zapper-purpose")
        } header: {
          Text("What it looks after")
        } footer: {
          Text("Optional. Becomes its instructions; you can change them later from its Drive.")
        }
        Section {
          Picker(selection: $draft.projectID) {
            if draft.projectID == nil { Text("Choose").tag(String?.none) }
            ForEach(projects) { project in Text(project.name).tag(String?.some(project.id)) }
          } label: {
            FormRowLabel(title: "Project", symbol: "folder")
          }
          .tint(.secondary)
          LabeledContent {
            HStack(spacing: 6) {
              if let state = workspace.computersIndicator {
                ComputerStatusDot(state: state, size: 7)
              }
              Text(workspace.connectedComputer ?? "This computer").foregroundStyle(.secondary)
                .lineLimit(1)
            }
          } label: {
            FormRowLabel(title: "Runs on", symbol: "desktopcomputer")
          }
          modelPicker.tint(.secondary)
        } header: {
          Text("Where it works")
        } footer: {
          Text("The Zapper and its threads work in this project’s repository.")
        }
        if let failure {
          Section {
            Label(failure, systemImage: "exclamationmark.triangle").font(.footnote)
              .foregroundStyle(.red)
          }
        }
      }
      .formStyle(.grouped)
      .scrollContentBackground(.hidden).background(AppTheme.surface)
      .navigationTitle("New Zapper").compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .cancellationAction) { SheetCloseButton { dismiss() } }
        ToolbarItem(placement: .confirmationAction) {
          Button {
            create()
          } label: {
            if creating {
              ProgressView()
            } else {
              Label("Create", systemImage: "checkmark")
            }
          }
          .buttonStyle(.borderedProminent).tint(AppTheme.accent)
          .disabled(!draft.isValid || creating)
          .accessibilityIdentifier("create-zapper")
        }
      }
      .interactiveDismissDisabled(creating)
      // Opened while the computer is still connecting, it fills in once the lists arrive.
      .task(id: workspace.connectionState) { if catalog.isEmpty { await loadCatalog() } }
      .onChange(of: projects.map(\.id)) { chooseDefaultProject() }
      .onAppear {
        chooseDefaultProject()
        if let starter, draft.name.isEmpty {
          draft.name = starter.name
          draft.purpose = starter.purpose
          draft.avatar = starter.avatar
        }
        #if DEBUG
          if ProcessInfo.processInfo.arguments.contains("--new-zapper-preview") && starter == nil {
            draft.name = "Release lead"
          }
        #endif
        nameFocused = draft.name.isEmpty
      }
    }
    .nativeSheetSize(width: 460, height: 620)
  }

  @ViewBuilder private var modelPicker: some View {
    if catalog.isEmpty {
      LabeledContent {
        if catalogFailed {
          Text("Unavailable").foregroundStyle(.secondary)
        } else {
          ProgressView().controlSize(.small)
        }
      } label: {
        FormRowLabel(title: "Model", symbol: "sparkles")
      }
    } else {
      Picker(selection: modelKey) {
        ForEach(catalog) { provider in
          Section(provider.name) {
            ForEach(provider.models) { model in
              Text(model.label).tag(provider.id + "\n" + model.id)
            }
          }
        }
      } label: {
        FormRowLabel(title: "Model", symbol: "sparkles")
      }
    }
  }

  private func chooseDefaultProject() {
    guard draft.projectID == nil else { return }
    draft.projectID =
      (workspace.selectedProject.flatMap { selected in
        projects.first { $0.id == selected.id }
      } ?? projects.first)?.id
  }

  private func loadCatalog() async {
    var providers: [ModelProvider] = []
    #if DEBUG
      if workspace.zappers.preview {
        providers = (try? WorkspaceModel.previewModelCatalog()) ?? []
      }
    #endif
    if providers.isEmpty, let rpc = workspace.rpc {
      providers = (try? await ModelProvider.load(rpc)) ?? []
    }
    catalog = providers
    catalogFailed = providers.isEmpty
    #if DEBUG
      // `--new-zapper-submit=<model>` picks that model and presses Create, to check creating
      // against a paired computer end to end.
      if let model = ProcessInfo.processInfo.arguments.first(where: {
        $0.hasPrefix("--new-zapper-submit=")
      })?.dropFirst(20),
        let provider = providers.first(where: { $0.models.contains { $0.id == model } })
      {
        draft.harness = provider.id
        draft.model = String(model)
        try? await Task.sleep(for: .seconds(2))
        create()
        return
      }
    #endif
    guard draft.harness == nil else { return }
    let preferred =
      providers.first { $0.id.lowercased().contains("claude") } ?? providers.first
    draft.harness = preferred?.id
    draft.model = preferred?.defaultModel
  }

  private func create() {
    failure = nil
    creating = true
    Task {
      defer { creating = false }
      do {
        let id = try await workspace.zappers.create(draft)
        created(id)
      } catch {
        failure = error.localizedDescription
      }
    }
  }
}
