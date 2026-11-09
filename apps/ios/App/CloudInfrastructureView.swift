import SwiftUI
import ZuseCore

struct CloudInfrastructureView: View {
  let workspace: WorkspaceModel
  private let client: CloudClient
  private let accountGeneration: UUID
  init(workspace: WorkspaceModel) {
    self.workspace = workspace
    client = workspace.cloud
    accountGeneration = workspace.account.generation
  }
  @State private var connections: [JSONValue] = []
  @State private var providers: [String] = []
  @State private var provider = ""
  @State private var key = ""
  @State private var template = ""
  @State private var organization = ""
  @State private var busy = false
  @State private var failure: String?
  @State private var removal: JSONValue?
  var body: some View {
    Form {
      if let failure { Text(failure).foregroundStyle(.red) }
      Section("Connected cloud providers") {
        if connections.filter({ $0["active"].bool }).isEmpty {
          Text("No provider keys connected.").foregroundStyle(.secondary)
        }
        ForEach(connections.filter { $0["active"].bool }, id: \.["connectionId"].string) { row in
          HStack {
            VStack(alignment: .leading, spacing: 2) {
              Text(row["providerId"].string ?? "Cloud provider")
              if let template = row["templateId"].string {
                Text(template).font(.caption).foregroundStyle(.secondary)
              }
            }
            Spacer()
            Button("Disconnect", role: .destructive) { removal = row }.frame(height: 28)
          }
        }
      }
      Section("Connect your provider account") {
        Picker("Provider", selection: $provider) {
          ForEach(providers, id: \.self) { Text($0).tag($0) }
        }.frame(height: 28)
        SecureField("API key", text: $key).verbatimInput().autocorrectionDisabled().frame(
          height: 28)
        TextField("Template or snapshot (optional)", text: $template).verbatimInput()
          .autocorrectionDisabled().frame(height: 28)
        TextField("Organization (optional)", text: $organization).verbatimInput()
          .autocorrectionDisabled().frame(height: 28)
        Button("Connect provider") { Task { await save() } }
          .disabled(provider.isEmpty || key.isEmpty || key.utf8.count > 4096).frame(height: 28)
        Text(
          "Workspace availability and billing follow your account's server policy. Keys are sent over HTTPS and are never saved by this app."
        )
        .font(.caption).foregroundStyle(.secondary)
      }
      if busy { ProgressView() }
    }.settingsList().navigationTitle("Cloud providers").compactNavigationTitle()
      .disabled(busy)
      .task { await load() }
      .onDisappear { key = "" }
      .confirmationDialog(
        "Disconnect this provider account?",
        isPresented: Binding(
          get: { removal != nil }, set: { if !$0 { removal = nil } })
      ) {
        Button("Disconnect", role: .destructive) {
          if let id = removal?["connectionId"].string { Task { await remove(id) } }
          removal = nil
        }
      } message: {
        Text(
          "Existing workspaces keep their provider identity. New workspaces will use your remaining available providers."
        )
      }
  }
  private func acceptConnections(_ result: JSONValue) throws {
    guard accountGeneration == workspace.account.generation else { throw CancellationError() }
    guard case .array(let rows) = result["connections"] else {
      throw ClientError("The cloud provider response was incomplete.")
    }
    connections = rows
  }
  private func load() async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    failure = nil
    defer { busy = false }
    do {
      guard let url = Bundle.main.url(forResource: "SettingsSchema", withExtension: "json") else {
        throw ClientError("The provider configuration schema is missing.")
      }
      let document = try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))[
        "cloudProviderConnection"]
      let schema = SettingsSchema(document: document)
      providers = schema.resolve(schema.field("providerId"))["enum"].array.compactMap(\.string)
      provider = providers.first ?? ""
      let result = try await client.request("/v1/cloud/provider-connections")
      try acceptConnections(result)
    } catch { failure = error.localizedDescription }
  }
  private func save() async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    failure = nil
    defer { busy = false }
    var body: [String: JSONValue] = ["providerId": .string(provider), "apiKey": .string(key)]
    if !template.isEmpty { body["templateId"] = .string(template) }
    if !organization.isEmpty { body["organization"] = .string(organization) }
    do {
      let result = try await client.request(
        "/v1/cloud/provider-connections", method: "POST", body: .object(body))
      try acceptConnections(result)
      key = ""
    } catch {
      key = ""
      failure = error.localizedDescription
    }
  }
  private func remove(_ id: String) async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    failure = nil
    defer { busy = false }
    do {
      let result = try await client.request(
        "/v1/cloud/provider-connections", method: "DELETE",
        body: .object(["connectionId": .string(id)]))
      try acceptConnections(result)
    } catch { failure = error.localizedDescription }
  }
}
