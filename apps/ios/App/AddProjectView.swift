import SwiftUI
import ZuseCore

struct AddProjectView: View {
  let workspace: WorkspaceModel
  @Environment(\.dismiss) private var dismiss
  @State private var path = ""
  @State private var listing: JSONValue = .null
  @State private var busy = false
  @State private var error: String?
  @State private var scope: String?
  var body: some View {
    NavigationStack {
      Form {
        Section("Project on your computer") {
          LabeledContent("Computer", value: workspace.connectedComputer ?? "Connected computer")
          HStack {
            TextField("Folder path", text: $path).verbatimInput().autocorrectionDisabled().frame(
              height: 28
            )
            .onSubmit { Task { await browse(path) } }
            Button("Browse") { Task { await browse(path) } }.frame(height: 28)
          }
          if let error { Text(error).foregroundStyle(.red) }
          if busy { ProgressView() }
        }
        if listing != .null {
          Section(listing["path"].string ?? "Folders") {
            if let parent = listing["parent"].string {
              Button("Parent folder", systemImage: "arrow.up") { Task { await browse(parent) } }
            }
            ForEach(
              listing["entries"].array.filter { $0["kind"].string == "directory" },
              id: \.["path"].string
            ) { entry in
              Button(entry["name"].string ?? "Folder", systemImage: "folder") {
                Task { await browse(entry["path"].string ?? "") }
              }
            }
          }
        }
        Button("Add project") {
          Task {
            busy = true
            error = nil
            defer { busy = false }
            do {
              try await workspace.addProject(path: path)
              dismiss()
            } catch { self.error = error.localizedDescription }
          }
        }.disabled(busy || path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
      }.settingsList().disabled(busy)
        .navigationTitle("Add project").compactNavigationTitle()
        .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        .task {
          scope = workspace.operationScope
          await browse("")
        }
        .onChange(of: workspace.operationScope) { _, value in if value != scope { dismiss() } }
    }.nativeSheetSize(width: 440, height: 480)
  }
  private func browse(_ requested: String) async {
    guard let rpc = workspace.rpc else { return }
    busy = true
    error = nil
    defer { busy = false }
    do {
      let result = try await rpc.call("workspace.browseDirectory", ["path": .string(requested)])
      guard scope == workspace.operationScope else { return }
      listing = result
      path = result["path"].string ?? requested
    } catch { self.error = error.localizedDescription }
  }
}
