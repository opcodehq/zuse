import SwiftUI
import ZuseCore

struct CloudSettingsView: View {
  let workspace: WorkspaceModel
  private let client: CloudClient
  private let accountGeneration: UUID
  private let accountIdentity: String?
  init(workspace: WorkspaceModel) {
    self.workspace = workspace
    client = workspace.cloud
    accountGeneration = workspace.account.generation
    accountIdentity = workspace.account.archiveIdentity
  }
  @Environment(\.dismiss) private var dismiss
  @Environment(\.openURL) private var openURL
  @State private var status: JSONValue = .null
  @State private var statusRequest = UUID()
  @State private var operation: JSONValue = .null
  @State private var loginRevision = UUID()
  @State private var providers: [String] = []
  @State private var methods: [String] = []
  @State private var loginProviders: [String] = []
  @State private var busy = false
  @State private var credential = ""
  @State private var credentialProvider = ""
  @State private var credentialMethod = "api-key"
  @State private var baseURL = ""
  @State private var modelProvider = ""
  @State private var failure: String?
  @State private var disconnectProvider: String?
  private var authorizing: Bool { operation["state"].string == "authorizing" }
  private var operationKey: String? {
    accountIdentity.map { "cloud-agent-login-" + ArchiveStore.storageID($0) }
  }
  var body: some View {
    NavigationStack {
      Form {
        if let failure { Text(failure).foregroundStyle(.red) }
        Section("Cloud agents") {
          ForEach(status["providers"].array, id: \.["providerId"].string) { row in
            HStack {
              LabeledContent(row["providerId"].string?.capitalized ?? "Agent") {
                Text(
                  row["accountLabel"].string ?? row["state"].string?.replacingOccurrences(
                    of: "-", with: " "
                  ).capitalized ?? "Unknown"
                )
                .foregroundStyle(.secondary)
              }
              if let provider = row["providerId"].string, row["state"].string != "disconnected" {
                Menu {
                  Button("Disconnect agent", role: .destructive) { disconnectProvider = provider }
                } label: {
                  Image(systemName: "ellipsis")
                }
                .disabled(busy || authorizing)
              }
            }
          }
          ForEach(loginProviders, id: \.self) { provider in
            Button("Sign in to " + provider.capitalized) { Task { await start(provider) } }
              .disabled(busy || authorizing).frame(height: 28)
          }
          Button("Refresh cloud agents") { Task { await refresh() } }.disabled(busy).frame(
            height: 28)
        }
        Section("Connect with a credential") {
          Picker("Agent", selection: $credentialProvider) {
            ForEach(providers, id: \.self) { Text($0.capitalized).tag($0) }
          }.frame(height: 28)
          Picker("Authentication", selection: $credentialMethod) {
            ForEach(methods, id: \.self) { method in
              Text(
                method == "api-key"
                  ? "API key" : method == "subscription" ? "Subscription token" : "Custom endpoint"
              ).tag(method)
            }
          }.frame(height: 28)
          SecureField("Credential", text: $credential).verbatimInput().autocorrectionDisabled()
            .frame(height: 28)
          if credentialMethod == "custom" {
            TextField("HTTPS base URL", text: $baseURL).verbatimInput().autocorrectionDisabled()
              .frame(height: 28)
            TextField("Model provider (optional)", text: $modelProvider).verbatimInput()
              .autocorrectionDisabled().frame(height: 28)
          }
          Button("Connect agent") { Task { await configure() } }
            .disabled(
              busy || authorizing || credential.isEmpty || credentialProvider.isEmpty
                || (credentialMethod == "custom"
                  && baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            ).frame(height: 28)
          Text("Encrypted on this device for your cloud authentication service.").font(.caption)
            .foregroundStyle(.secondary)
        }
        .disabled(busy)
        if authorizing {
          Section("Finish signing in") {
            if let code = operation["verificationCode"].string {
              LabeledContent("Code", value: code).textSelection(.enabled).frame(height: 28)
              Button("Copy code", systemImage: "doc.on.doc") { NativePlatform.copy(code) }.frame(
                height: 28)
            }
            if let address = operation["verificationUrl"].string, let url = URL(string: address),
              url.scheme == "https"
            {
              Button("Open sign-in page", systemImage: "arrow.up.right.square") { openURL(url) }
                .frame(height: 28)
            }
            ProgressView("Waiting for sign-in…")
            Button("Cancel sign-in", role: .cancel) { Task { await cancel() } }.disabled(busy)
              .frame(height: 28)
          }
        }
        Section {
          Text(
            "Cloud agent connections are shared with your other Zuse clients. Disconnecting an agent removes its cloud credentials from your account."
          )
          .font(.caption).foregroundStyle(.secondary)
        }
      }.settingsList().navigationTitle("Cloud settings").compactNavigationTitle()
        .toolbar { ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } } }
        .task { await load() }
        .onDisappear { credential = "" }
        .task(id: (operation["operationId"].string ?? "") + loginRevision.uuidString) {
          await poll()
        }
        .confirmationDialog(
          "Disconnect this cloud agent?",
          isPresented: Binding(
            get: { disconnectProvider != nil }, set: { if !$0 { disconnectProvider = nil } })
        ) {
          if let provider = disconnectProvider {
            Button("Disconnect " + provider.capitalized, role: .destructive) {
              disconnectProvider = nil
              Task { await disconnect(provider) }
            }
          }
        } message: {
          Text("Your other Zuse clients will also see this agent as disconnected.")
        }
    }
  }
  private func encoded(_ value: String) -> String {
    value.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
  }
  private func report(_ error: Error) {
    workspace.account.recordAuthenticationFailure(error)
    failure = error.localizedDescription
  }
  private func acceptOperation(_ result: JSONValue, key: String) throws {
    guard result["operationId"].string != nil, result["state"].string != nil else {
      throw ClientError("The cloud sign-in response was incomplete.")
    }
    operation = result
    if result["state"].string == "authorizing" {
      try Vault.write(JSONEncoder().encode(result), key: key)
    } else {
      try Vault.write(nil, key: key)
    }
  }
  private func load() async {
    guard accountGeneration == workspace.account.generation else { return }
    busy = true
    defer { busy = false }
    let generation = accountGeneration
    do {
      guard let url = Bundle.main.url(forResource: "SettingsSchema", withExtension: "json") else {
        throw ClientError("The agent configuration schema is missing.")
      }
      let document = try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
      let auth = SettingsSchema(document: document["cloudAuthConfigure"])
      providers = auth.resolve(auth.field("providerId"))["enum"].array.compactMap(
        \.string)
      methods = auth.resolve(auth.field("method"))["enum"].array.compactMap(
        \.string)
      let login = SettingsSchema(document: document["cloudAuthLogin"])
      loginProviders = login.resolve(login.field("providerId"))["enum"].array
        .compactMap(\.string)
      guard !providers.isEmpty, !methods.isEmpty, !loginProviders.isEmpty else {
        throw ClientError(
          "The bundled cloud agent configuration is incomplete. Rebuild the native app.")
      }
      credentialProvider = providers.first ?? ""
      if !methods.contains(credentialMethod) { credentialMethod = methods.first ?? "" }
      await refresh()
      guard generation == workspace.account.generation, !Task.isCancelled else { return }
      if let key = operationKey, let saved = try Vault.read(key),
        let id = try JSONDecoder().decode(JSONValue.self, from: saved)["operationId"].string
      {
        do {
          let result = try await client.request("/v1/cloud/auth/login/" + encoded(id))
          guard generation == workspace.account.generation, !Task.isCancelled else { return }
          try acceptOperation(result, key: key)
        } catch let error as ClientError where error.code == "access-unavailable" {
          try Vault.write(nil, key: key)
          report(error)
        }
      }
    } catch is CancellationError {} catch {
      if generation == workspace.account.generation { report(error) }
    }
  }
  private func refresh() async {
    guard accountGeneration == workspace.account.generation else { return }
    let generation = accountGeneration
    let request = UUID()
    statusRequest = request
    do {
      let result = try await client.request("/v1/cloud/auth")
      guard generation == workspace.account.generation, statusRequest == request, !Task.isCancelled
      else { return }
      status = result
    } catch is CancellationError {} catch {
      if generation == workspace.account.generation, statusRequest == request { report(error) }
    }
  }
  private func readyStatus(generation: UUID, refreshKey: Bool = false) async throws {
    guard generation == workspace.account.generation else { throw CancellationError() }
    if refreshKey || status["authorityState"].string != "ready" {
      statusRequest = UUID()
      let ready = try await client.request(
        "/v1/cloud/auth/provision", method: "POST", body: .object([:]))
      guard generation == workspace.account.generation else { throw CancellationError() }
      statusRequest = UUID()
      status = ready
    }
  }
  private func start(_ provider: String) async {
    guard accountGeneration == workspace.account.generation else { return }
    guard !busy, !authorizing, let key = operationKey else { return }
    busy = true
    failure = nil
    defer { busy = false }
    let generation = accountGeneration
    do {
      try await readyStatus(generation: generation)
      guard generation == workspace.account.generation else { return }
      let result = try await client.request(
        "/v1/cloud/auth/login/start", method: "POST",
        body: .object(["providerId": .string(provider)]))
      guard generation == workspace.account.generation else { return }
      try acceptOperation(result, key: key)
    } catch is CancellationError {} catch {
      if generation == workspace.account.generation { report(error) }
    }
  }
  private func poll() async {
    guard let id = operation["operationId"].string, let key = operationKey else { return }
    let generation = accountGeneration
    let revision = loginRevision
    var retryDelay = 3
    do {
      while operation["state"].string == "authorizing" {
        try await Task.sleep(for: .seconds(retryDelay))
        guard authorizing else { break }
        if busy { continue }
        guard generation == workspace.account.generation, loginRevision == revision,
          operation["operationId"].string == id
        else { return }
        do {
          let result = try await client.request("/v1/cloud/auth/login/" + encoded(id))
          try Task.checkCancellation()
          guard generation == workspace.account.generation, loginRevision == revision,
            operation["operationId"].string == id
          else { return }
          try acceptOperation(result, key: key)
          retryDelay = 3
          failure = nil
        } catch is CancellationError {
          return
        } catch {
          guard generation == workspace.account.generation, loginRevision == revision,
            operation["operationId"].string == id, !Task.isCancelled
          else { return }
          report(error)
          if (error as? ClientError)?.requiresUserAction == true { return }
          retryDelay = min(retryDelay * 2, 30)
        }
      }
      if operation["state"].string == "error" {
        failure = operation["errorCode"].string ?? "Agent sign-in failed."
      }
      await refresh()
    } catch is CancellationError {} catch {
      if generation == workspace.account.generation { report(error) }
    }
  }
  private func configure() async {
    guard accountGeneration == workspace.account.generation else { return }
    guard !busy, !authorizing else { return }
    let generation = accountGeneration
    busy = true
    failure = nil
    defer { busy = false }
    do {
      let address = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
      if credentialMethod == "custom" {
        guard let url = URL(string: address), url.scheme == "https", url.host != nil,
          url.user == nil, url.password == nil, url.fragment == nil
        else {
          throw ClientError("Enter an HTTPS base URL without embedded credentials.")
        }
      }
      try await readyStatus(generation: generation, refreshKey: true)
      guard generation == workspace.account.generation else { return }
      guard let keyID = status["encryptionKeyId"].string,
        let jwk = status["encryptionPublicJwk"].string
      else { throw ClientError("Cloud authentication is still starting. Try again shortly.") }
      let sealed = try CloudSecret.seal(credential, publicJWK: jwk)
      var fields: [String: JSONValue] = [
        "providerId": .string(credentialProvider), "method": .string(credentialMethod),
        "sealedSecret": .object(["keyId": .string(keyID), "ciphertext": .string(sealed)]),
      ]
      if credentialMethod == "custom" {
        fields["baseUrl"] = .string(address)
        let provider = modelProvider.trimmingCharacters(in: .whitespacesAndNewlines)
        if !provider.isEmpty { fields["modelProvider"] = .string(provider) }
      }
      let result = try await client.request(
        "/v1/cloud/auth/configure", method: "POST", body: .object(fields))
      guard generation == workspace.account.generation else { return }
      guard result["state"].string == "connected" else {
        throw ClientError("Agent credential verification failed.")
      }
      credential = ""
      await refresh()
    } catch is CancellationError {} catch {
      if generation == workspace.account.generation { report(error) }
    }
  }
  private func cancel() async {
    guard accountGeneration == workspace.account.generation else { return }
    guard !busy, let id = operation["operationId"].string, let key = operationKey else { return }
    let generation = accountGeneration
    loginRevision = UUID()
    busy = true
    defer { busy = false }
    do {
      let result = try await client.request(
        "/v1/cloud/auth/login/" + encoded(id) + "/cancel", method: "POST")
      guard generation == workspace.account.generation, operation["operationId"].string == id else {
        return
      }
      try acceptOperation(result, key: key)
      await refresh()
    } catch { if generation == workspace.account.generation { report(error) } }
  }
  private func disconnect(_ provider: String) async {
    guard accountGeneration == workspace.account.generation else { return }
    guard !busy, !authorizing else { return }
    let generation = accountGeneration
    busy = true
    failure = nil
    defer { busy = false }
    do {
      _ = try await client.request(
        "/v1/cloud/auth/providers/" + encoded(provider), method: "DELETE")
      guard generation == workspace.account.generation else { return }
      await refresh()
    } catch { if generation == workspace.account.generation { report(error) } }
  }
}
