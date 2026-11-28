import SwiftUI
import ZuseCore

/// What a chat's ··· sheet acts on.
enum ChatActionsTarget: Identifiable {
  case zapper(String)
  case chat(SessionRow)
  var id: String {
    switch self {
    case .zapper(let id): return "zapper:" + id
    case .chat(let session): return "chat:" + session.id
    }
  }
}

/// The chat header's ··· sheet. A Zapper's conversation offers its profile and the Zapper's
/// controls; an ordinary chat offers its changes, its title and archiving.
struct ChatActionsSheet: View {
  let workspace: WorkspaceModel
  let target: ChatActionsTarget
  let openProfile: () -> Void
  let showChanges: () -> Void
  /// Zooms out to every pane beside the chat.
  let showPanes: () -> Void
  /// The chat or Zapper left the screen (archived or deleted).
  let left: () -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var confirmation: Confirmation?
  private var store: ZapperStore { workspace.zappers }

  enum Confirmation: Identifiable {
    case fresh, archive, delete
    var id: Self { self }
  }

  var body: some View {
    NavigationStack {
      List {
        switch target {
        case .zapper(let id):
          if let zapper = store.zapper(id) { zapperActions(zapper) }
        case .chat(let session):
          chatActions(session)
        }
      }
      .settingsList()
      .navigationTitle(title).compactNavigationTitle()
      .toolbar {
        ToolbarItem(placement: .cancellationAction) { SheetCloseButton { dismiss() } }
      }
      .confirmationDialog(
        confirmationTitle, isPresented: confirmationPresented, titleVisibility: .visible,
        presenting: confirmation
      ) { action in
        Button(confirmationButton(action), role: action == .fresh ? nil : .destructive) {
          perform(action)
        }
      } message: { action in
        Text(confirmationMessage(action))
      }
    }
    .presentationDetents([.height(height), .large])
    .nativeSheetSize(width: 400, height: height)
  }

  private var height: CGFloat {
    if case .zapper = target { return 430 }
    return 280
  }

  private var title: String {
    switch target {
    case .zapper(let id): return store.zapper(id)?.config.name ?? "Zapper"
    case .chat(let session): return session.title.isEmpty ? "Chat" : session.title
    }
  }

  @ViewBuilder private func zapperActions(_ zapper: Zapper) -> some View {
    let busy = store.busy[zapper.id] != nil
    Section {
      panesButton
      Button {
        dismiss()
        openProfile()
      } label: {
        ActionRow(title: "Zapper profile", icon: "person", chevron: true) {
          if zapper.needsYouCount > 0 { ZapperNeedsYouBadge(count: zapper.needsYouCount) }
        }
      }
      .accessibilityIdentifier("chat-actions-profile")
    }
    Section {
      if zapper.status == .active || zapper.status == .paused {
        let pausing = zapper.status == .active
        Button {
          Task {
            await store.perform(
              .setStatus(pausing ? .paused : .active), on: zapper,
              label: pausing ? "Pause" : "Resume")
          }
        } label: {
          ActionRow(title: pausing ? "Pause" : "Resume", icon: pausing ? "pause" : "play")
        }
        .disabled(busy)
      }
      Button {
        confirmation = .fresh
      } label: {
        ActionRow(title: "Fresh conversation", icon: "arrow.counterclockwise")
      }
      .disabled(busy)
    }
    Section {
      if zapper.status != .archived {
        Button {
          confirmation = .archive
        } label: {
          ActionRow(title: "Archive", icon: "archivebox")
        }
        .disabled(busy)
      }
      Button {
        confirmation = .delete
      } label: {
        ActionRow(title: "Delete", icon: "trash", destructive: true)
      }
      .disabled(busy)
    }
  }

  private var panesButton: some View {
    Button {
      dismiss()
      showPanes()
    } label: {
      ActionRow(title: "All panes", icon: "square.grid.2x2", chevron: true)
    }
    .accessibilityIdentifier("chat-actions-panes")
  }
  @ViewBuilder private func chatActions(_ session: SessionRow) -> some View {
    Section {
      panesButton
      Button {
        dismiss()
        showChanges()
      } label: {
        ActionRow(title: "Changes", icon: "plusminus", chevron: true)
      }
      .disabled(workspace.connectionState != "Connected")
      Button {
        NativePlatform.copy(session.title)
        dismiss()
      } label: {
        ActionRow(title: "Copy title", icon: "doc.on.doc")
      }
    }
    Section {
      Button {
        Task {
          if await workspace.archiveChat(session) {
            dismiss()
            left()
          }
        }
      } label: {
        ActionRow(title: "Archive", icon: "archivebox")
      }
      .disabled(workspace.restoringChats.contains(session.id))
    }
  }

  private var zapper: Zapper? {
    if case .zapper(let id) = target { return store.zapper(id) }
    return nil
  }
  private var confirmationPresented: Binding<Bool> {
    Binding(get: { confirmation != nil }, set: { if !$0 { confirmation = nil } })
  }
  private var confirmationTitle: String {
    let name = zapper?.config.name ?? "Zapper"
    switch confirmation {
    case .fresh: return "Start a fresh conversation?"
    case .archive: return "Archive \(name)?"
    case .delete: return "Delete \(name)?"
    case nil: return ""
    }
  }
  private func confirmationButton(_ action: Confirmation) -> String {
    switch action {
    case .fresh: return "Start fresh"
    case .archive: return "Archive Zapper and threads"
    case .delete: return "Delete Zapper"
    }
  }
  private func confirmationMessage(_ action: Confirmation) -> String {
    let name = zapper?.config.name ?? "The Zapper"
    switch action {
    case .fresh:
      return
        "\(name) keeps its threads, documents and decisions. This conversation moves to Earlier conversations."
    case .archive: return "Its threads are archived too. You can resume the Zapper later."
    case .delete: return "Its threads and documents are kept."
    }
  }
  private func perform(_ action: Confirmation) {
    guard let zapper else { return }
    Task {
      switch action {
      case .fresh:
        if await store.perform(.freshConversation, on: zapper, label: "Fresh conversation") {
          dismiss()
        }
      case .archive:
        if await store.perform(.setStatus(.archived), on: zapper, label: "Archive") {
          dismiss()
        }
      case .delete:
        if await store.perform(.delete, on: zapper, label: "Delete") {
          dismiss()
          left()
        }
      }
    }
  }
}

/// A row in an actions sheet.
private struct ActionRow<Trailing: View>: View {
  let title: String
  let icon: String
  var destructive = false
  var chevron = false
  @ViewBuilder var trailing: Trailing
  var body: some View {
    HStack(spacing: 14) {
      Image(systemName: icon).font(.system(size: 16))
        .foregroundStyle(destructive ? Color.red : Color.primary.opacity(0.8)).frame(width: 22)
      Text(title).foregroundStyle(destructive ? Color.red : Color.primary)
      Spacer(minLength: 8)
      trailing
      if chevron {
        Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
          .foregroundStyle(.tertiary)
      }
    }
    .contentShape(Rectangle())
  }
}

extension ActionRow where Trailing == EmptyView {
  init(title: String, icon: String, destructive: Bool = false, chevron: Bool = false) {
    self.init(title: title, icon: icon, destructive: destructive, chevron: chevron) {
      EmptyView()
    }
  }
}
