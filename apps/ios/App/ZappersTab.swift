import SwiftUI
import ZuseCore

/// The Zappers tab: one row per Zapper, like a message list. Opening one opens its
/// conversation; its threads, issues and docs live on its overview page.
struct ZappersListView: View {
  let workspace: WorkspaceModel
  let open: (String) -> Void
  let connect: () -> Void
  @State private var filter: ArchiveFilter = .active
  @State private var creating = false
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
      if visible.isEmpty { emptyState.zappersRow().padding(.top, 72) }
    }
    .listStyle(.plain)
    .contentMargins(.top, 4, for: .scrollContent)
    .scrollContentBackground(.hidden)
    .background(AppTheme.surface)
    .navigationTitle("Zappers")
    .compactNavigationTitle()
    .toolbar {
      ToolbarItemGroup(placement: .primaryAction) {
        Button("New Zapper", systemImage: "plus") { creating = true }
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
    .sheet(isPresented: $creating) {
      NewZapperSheet(workspace: workspace) { id in
        creating = false
        open(id)
      }
    }
    .refreshable { await workspace.refreshSessionCatalog() }
    .onAppear {
      #if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--new-zapper-preview") { creating = true }
      #endif
    }
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
    } else if !store.loaded && workspace.connectionState == "Connected" && store.available {
      ProgressView().frame(maxWidth: .infinity)
    } else if workspace.connectionState == "Connected" && !store.available && !store.preview {
      ZappersEmptyState(
        symbol: "arrow.down.circle", title: "Update Zuse to use Zappers",
        detail:
          "\(workspace.connectedComputer ?? "This computer") runs a version of Zuse without Zappers."
      )
    } else if store.available || store.preview {
      ZappersEmptyState(
        symbol: AppIcon.zapper, title: "No Zappers yet",
        detail:
          "A Zapper looks after one goal for you. It plans the work, starts threads and asks when it needs you."
      ) {
        Button {
          creating = true
        } label: {
          Label("New Zapper", systemImage: "plus").frame(maxWidth: 260)
        }
        .buttonStyle(.borderedProminent).tint(AppTheme.accent).controlSize(.large)
        .buttonBorderShape(.capsule)
      }
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

/// Name a Zapper, pick its project and model, and give it a face.
struct NewZapperSheet: View {
  let workspace: WorkspaceModel
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
          VStack(spacing: 8) {
            ZapperMascotView(avatar: draft.avatar, size: 64)
              .frame(width: 88, height: 88)
              .background(
                Color(hex: ZapperMascots.resolve(draft.avatar).1.body).opacity(0.16),
                in: RoundedRectangle(cornerRadius: 24, style: .continuous)
              )
              .animation(.snappy, value: draft.avatar)
              .accessibilityHidden(true)
            Button("Change look") {
              withAnimation(.snappy) { draft.nextLook() }
            }
            .font(.subheadline.weight(.medium)).tint(AppTheme.accent).buttonStyle(.borderless)
            .accessibilityHint("Shows a different face for this Zapper")
          }
          .frame(maxWidth: .infinity).padding(.vertical, 4)
          .listRowBackground(Color.clear)
        }
        Section {
          TextField("Name", text: $draft.name)
            .font(.system(size: 17)).focused($nameFocused).submitLabel(.done)
            .accessibilityIdentifier("new-zapper-name")
        }
        Section {
          Picker("Project", selection: $draft.projectID) {
            if draft.projectID == nil { Text("Choose").tag(String?.none) }
            ForEach(projects) { project in Text(project.name).tag(String?.some(project.id)) }
          }
          .tint(.secondary)
          LabeledContent("Runs on") {
            Text(workspace.connectedComputer ?? "This computer").foregroundStyle(.secondary)
          }
          modelPicker.tint(.secondary)
        } footer: {
          Text("The Zapper and its threads work in this project’s repository.")
        }
        Section {
          TextField("What should it look after? (optional)", text: $draft.purpose, axis: .vertical)
            .lineLimit(2...6)
        } header: {
          Text("Purpose")
        } footer: {
          Text("Becomes its instructions. You can change them later from its Drive.")
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
      .task { await loadCatalog() }
      .onAppear {
        if draft.projectID == nil {
          draft.projectID =
            (workspace.selectedProject.flatMap { selected in
              projects.first { $0.id == selected.id }
            } ?? projects.first)?.id
        }
        #if DEBUG
          if ProcessInfo.processInfo.arguments.contains("--new-zapper-preview") {
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
      LabeledContent("Model") {
        if catalogFailed {
          Text("Unavailable").foregroundStyle(.secondary)
        } else {
          ProgressView().controlSize(.small)
        }
      }
    } else {
      Picker("Model", selection: modelKey) {
        ForEach(catalog) { provider in
          Section(provider.name) {
            ForEach(provider.models) { model in
              Text(model.label).tag(provider.id + "\n" + model.id)
            }
          }
        }
      }
    }
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
