import SwiftUI
import ZuseCore

struct RegisteredDevicesView: View {
  let workspace: WorkspaceModel
  private let client: CloudClient
  private let accountGeneration: UUID
  init(workspace: WorkspaceModel) {
    self.workspace = workspace
    client = workspace.cloud
    accountGeneration = workspace.account.generation
  }
  @State private var clients: [JSONValue] = []
  @State private var busy = false
  @State private var failure: String?
  @State private var revokeID: String?
  var body: some View {
    List {
      if let failure { Text(failure).foregroundStyle(.red) }
      if busy { ProgressView("Loading devices…") }
      if !busy && clients.isEmpty && failure == nil {
        ContentUnavailableView("No registered devices", systemImage: "desktopcomputer.and.iphone")
      }
      ForEach(clients, id: \.["clientId"].string) { client in
        HStack {
          VStack(alignment: .leading, spacing: 3) {
            Label(
              client["platform"].string?.capitalized ?? "Device",
              systemImage: "desktopcomputer.and.iphone")
            if let timestamp = client["lastSeenAt"].int {
              Text(Date(timeIntervalSince1970: Double(timestamp) / 1000), style: .relative)
                .font(.caption).foregroundStyle(.secondary)
            }
          }
          Spacer()
          Button("Revoke", role: .destructive) { revokeID = client["clientId"].string }.frame(
            height: 28)
        }
      }
    }.settingsList().disabled(busy).navigationTitle("Registered devices").compactNavigationTitle()
      .task { await load() }.refreshable { await load() }
      .confirmationDialog(
        "Revoke this device?",
        isPresented: Binding(
          get: { revokeID != nil }, set: { if !$0 { revokeID = nil } })
      ) {
        Button("Revoke access", role: .destructive) {
          if let id = revokeID { Task { await revoke(id) } }
          revokeID = nil
        }
      } message: {
        Text("This removes the device's registered access to your Zuse account.")
      }
  }
  private func load() async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    failure = nil
    defer { busy = false }
    do {
      let result = try await client.request("/v1/clients")
      guard accountGeneration == workspace.account.generation else { return }
      guard case .array(let rows) = result["clients"] else {
        throw ClientError("The device list response was incomplete.")
      }
      clients = rows
    } catch { failure = error.localizedDescription }
  }
  private func revoke(_ id: String) async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    failure = nil
    defer { busy = false }
    do {
      let segment = id.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
      _ = try await client.request("/v1/clients/" + segment, method: "DELETE")
      guard accountGeneration == workspace.account.generation else { return }
      clients.removeAll { $0["clientId"].string == id }
    } catch { failure = error.localizedDescription }
  }
}
