import SwiftUI
import ZuseCore

/// One Drive document: rendered Markdown, its version history, and editing.
struct ZapperDocumentView: View {
  let store: ZapperStore
  let zapperID: String
  let path: String
  /// Shown as a pane beside the chat: history and editing sit above the text instead of in the
  /// navigation bar, which belongs to the chat.
  var embedded = false
  @State private var versions: [ZapperFile] = []
  @State private var shown: Int?
  @State private var problem: String?
  @State private var editing = false
  @State private var restoring: ZapperFile?

  private var latest: ZapperFile? { versions.max { $0.version < $1.version } }
  private var current: ZapperFile? {
    shown.flatMap { version in versions.first { $0.version == version } } ?? latest
  }

  var body: some View {
    let zapper = store.zapper(zapperID)
    ScrollView {
      VStack(alignment: .leading, spacing: 12) {
        if embedded && latest != nil {
          HStack(spacing: 14) {
            Spacer()
            actions(zapper)
          }
          .labelStyle(.titleAndIcon).font(.subheadline)
        }
        if let file = current {
          if let latest, file.version != latest.version {
            HStack(spacing: 10) {
              Image(systemName: "clock.arrow.circlepath").foregroundStyle(.secondary)
              Text("Version \(file.version) of \(latest.version)").font(.subheadline)
              Spacer()
              Button("Restore") { restoring = file }.fontWeight(.semibold)
                .disabled(zapper == nil || store.busy[zapperID] != nil)
              Button("Latest") { shown = nil }
            }
            .buttonStyle(.borderless)
            .padding(12)
            .background(AppTheme.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
          }
          if file.content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            Text("This version is empty.").foregroundStyle(.secondary)
          } else {
            ChatMarkdown(text: file.content)
          }
        } else if let problem {
          ContentUnavailableView(
            "Couldn’t open \(ZapperDrive.title(path))", systemImage: "doc.questionmark",
            description: Text(problem))
        } else {
          ProgressView().frame(maxWidth: .infinity).padding(.top, 60)
        }
      }
      .frame(maxWidth: 720, alignment: .leading).frame(maxWidth: .infinity)
      .padding(.horizontal, 20).padding(.vertical, 16)
    }
    .background(AppTheme.surface)
    .modifier(
      DocumentHeader(title: ZapperDrive.title(path), enabled: !embedded) { actions(zapper) }
    )
    .task(id: "\(zapperID)|\(zapper?.revision ?? 0)") { await load() }
    .sheet(isPresented: $editing) {
      if let latest, let zapper {
        ZapperDocumentEditor(file: latest) { content in
          let saved = await store.perform(
            .writeFile(path: path, content: content, expectedVersion: latest.version), on: zapper,
            label: "Save")
          if saved { await load() }
          return saved
        }
      }
    }
    .confirmationDialog(
      "Restore version \(restoring?.version ?? 0)?",
      isPresented: Binding(get: { restoring != nil }, set: { if !$0 { restoring = nil } }),
      titleVisibility: .visible, presenting: restoring
    ) { file in
      Button("Restore version \(file.version)") {
        guard let zapper, let latest else { return }
        Task {
          if await store.perform(
            .restoreFile(path: path, version: file.version, expectedVersion: latest.version),
            on: zapper, label: "Restore")
          {
            shown = nil
            await load()
          }
        }
      }
    } message: { _ in
      Text("Restoring saves it as a new version. Nothing is lost.")
    }
    .alert(
      "Couldn’t save \(ZapperDrive.title(path))",
      isPresented: Binding(
        get: { store.failure[zapperID] != nil },
        set: { if !$0 { store.failure.removeValue(forKey: zapperID) } })
    ) {
      Button("OK") { store.failure.removeValue(forKey: zapperID) }
    } message: {
      Text(store.failure[zapperID] ?? "")
    }
  }

  @ViewBuilder private func actions(_ zapper: Zapper?) -> some View {
    if versions.count > 1 {
      Menu {
        Picker("Version", selection: $shown) {
          ForEach(versions.sorted { $0.version > $1.version }) { file in
            Text(
              file.version == latest?.version
                ? "Version \(file.version) · Latest" : "Version \(file.version)"
            )
            .tag(file.version == latest?.version ? nil : Optional(file.version))
          }
        }
      } label: {
        Label("History", systemImage: "clock.arrow.circlepath")
      }
      .accessibilityIdentifier("zapper-document-history")
    }
    Button("Edit", systemImage: "square.and.pencil") { editing = true }
      .disabled(latest == nil || zapper == nil)
  }

  private func load() async {
    do {
      versions = try await store.files(zapperID, history: true).filter { $0.path == path }
      problem = versions.isEmpty ? "This document has no saved versions." : nil
    } catch {
      problem = error.localizedDescription
    }
  }
}

/// The document's title and actions in the navigation bar, unless the page is a pane.
private struct DocumentHeader<Actions: View>: ViewModifier {
  let title: String
  let enabled: Bool
  @ViewBuilder let actions: Actions
  func body(content: Content) -> some View {
    if enabled {
      content
        .navigationTitle(title)
        .compactNavigationTitle()
        .toolbar { ToolbarItemGroup(placement: .primaryAction) { actions } }
    } else {
      content
    }
  }
}

private struct ZapperDocumentEditor: View {
  let file: ZapperFile
  let save: (String) async -> Bool
  @Environment(\.dismiss) private var dismiss
  @State private var text: String
  @State private var saving = false
  init(file: ZapperFile, save: @escaping (String) async -> Bool) {
    self.file = file
    self.save = save
    _text = State(initialValue: file.content)
  }
  var body: some View {
    NavigationStack {
      TextEditor(text: $text)
        .font(.system(.body, design: .monospaced))
        .scrollContentBackground(.hidden)
        .padding(.horizontal, 12)
        .background(AppTheme.surface)
        .navigationTitle(ZapperDrive.title(file.path))
        .compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
          ToolbarItem(placement: .confirmationAction) {
            Button("Save") {
              saving = true
              Task {
                let saved = await save(text)
                saving = false
                if saved { dismiss() }
              }
            }
            .disabled(saving || text == file.content)
          }
        }
    }
  }
}
