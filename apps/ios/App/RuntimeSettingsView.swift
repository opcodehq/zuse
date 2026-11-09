import SwiftUI
import ZuseCore

struct RuntimeSettingsView: View {
  enum Target {
    case computer(RPCClient)
    case cloud(CloudClient)
    var isCloud: Bool {
      if case .cloud = self { return true }
      return false
    }
    var title: String { isCloud ? "Cloud preferences" : "Computer settings" }
  }
  let target: Target
  init(rpc: RPCClient) { target = .computer(rpc) }
  init(cloud: CloudClient) { target = .cloud(cloud) }
  @Environment(\.dismiss) private var dismiss
  @State private var revision = 0
  @State private var original: JSONValue = .null
  @State private var edited: JSONValue = .null
  @State private var schema: SettingsSchema?
  @State private var failure: String?
  @State private var saving = false
  @State private var loading = true
  @State private var search = ""
  private var patch: [String: JSONValue] {
    SettingsSchema.patch(original: original, edited: edited)
  }
  var body: some View {
    NavigationStack {
      Form {
        if let failure {
          Section {
            Text(failure).foregroundStyle(.red)
            Button("Reload settings") { Task { await load() } }
          }
        }
        if loading { ProgressView("Loading settings…") }
        if let schema, original != .null {
          let properties = schema.properties
          Section {
            Text(
              target.isCloud
                ? "Defaults for your cloud workspaces. Use a setting's context menu to restore its inherited default."
                : "Preferences for the connected computer. Device appearance is set separately in Settings."
            )
            .font(.caption).foregroundStyle(.secondary)
          }
          ForEach(
            properties.keys.sorted().filter {
              search.isEmpty || settingLabel($0).localizedCaseInsensitiveContains(search)
            }, id: \.self
          ) { name in
            SettingField(
              name: settingLabel(name), value: field(name), definition: properties[name] ?? .null,
              schema: schema
            )
            .contextMenu {
              if target.isCloud, edited[name] != .null {
                Button("Use inherited default") {
                  edited = edited.removing(name)
                }
              }
            }
          }
        }
      }.settingsList().searchable(text: $search, prompt: "Find a setting")
        .navigationTitle(target.title).compactNavigationTitle()
        .toolbar {
          ToolbarItem(placement: .sheetClose) { SheetCloseButton { dismiss() } }
          ToolbarItem(placement: .confirmationAction) {
            Button(saving ? "Saving…" : "Save") { Task { await save() } }
              .disabled(saving || edited == original || loading)
          }
        }
        .disabled(saving).interactiveDismissDisabled(saving)
        .task { await load() }
    }
  }
  private func field(_ name: String) -> Binding<JSONValue> {
    Binding(get: { edited[name] }, set: { edited = edited.setting(name, to: $0) })
  }
  private func load() async {
    loading = true
    failure = nil
    defer { loading = false }
    do {
      guard let url = Bundle.main.url(forResource: "SettingsSchema", withExtension: "json") else {
        throw ClientError("The settings schema is missing. Rebuild the native app.")
      }
      let document = try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
      schema = SettingsSchema(document: target.isCloud ? document["workspaceSettings"] : document)
      let response: JSONValue
      switch target {
      case .computer(let rpc): response = try await rpc.call("settings.get")
      case .cloud(let client): response = try await client.request("/v1/cloud/settings")
      }
      let values = target.isCloud ? response["values"] : response
      guard case .object = values else {
        throw ClientError("The server returned invalid settings. Reconnect and try again.")
      }
      original = values
      revision = response["revision"].int ?? 0
      edited = original
    } catch { failure = error.localizedDescription }
  }
  private func save() async {
    guard !saving, !loading, edited != original else { return }
    saving = true
    failure = nil
    defer { saving = false }
    let changes = patch
    do {
      switch target {
      case .computer(let rpc):
        // Nested maps are replacements in the shared API: check touched fields before writing.
        let latest = try await rpc.call("settings.get")
        for key in changes.keys where latest[key] != original[key] && latest[key] != changes[key] {
          throw ClientError(
            "\(settingLabel(key)) changed on another client. Reload settings before saving.")
        }
        original = try await rpc.call("settings.update", ["patch": .object(changes)])
      case .cloud(let client):
        let response = try await client.request(
          "/v1/cloud/settings", method: "PUT",
          body: .object([
            "expectedRevision": .number(Double(revision)), "values": edited,
          ]))
        original = response["values"]
        revision = response["revision"].int ?? revision
      }
      edited = original
    } catch { failure = error.localizedDescription }
  }
}

private func settingLabel(_ key: String) -> String {
  key.replacingOccurrences(of: "([a-z0-9])([A-Z])", with: "$1 $2", options: .regularExpression)
    .replacingOccurrences(of: "_", with: " ").capitalized
}

/// A native field for every shape in the shared SettingsPatch schema.
private struct SettingField: View {
  let name: String
  @Binding var value: JSONValue
  let definition: JSONValue
  let schema: SettingsSchema
  private var resolved: JSONValue { schema.resolve(definition, value: value) }
  var body: some View {
    if value == .null {
      Button("Configure \(name)") { value = schema.initialValue(definition) }.frame(height: 28)
    } else if case .bool = value {
      Toggle(name, isOn: Binding(get: { value.bool }, set: { value = .bool($0) })).frame(
        height: 28)
    } else if case .string = value {
      let choices = schema.stringChoices(definition)
      if !choices.isEmpty && schema.allowsCustomString(definition) {
        HStack {
          TextField(name, text: Binding(get: { value.string ?? "" }, set: { value = .string($0) }))
            .verbatimInput().autocorrectionDisabled().frame(height: 28)
          Menu("Choose") {
            ForEach(choices, id: \.self) { choice in
              Button(choice) { value = .string(choice) }
            }
          }.frame(height: 28)
        }
      } else if !choices.isEmpty {
        Picker(name, selection: Binding(get: { value.string ?? "" }, set: { value = .string($0) }))
        {
          ForEach(Array(Set(choices + [value.string ?? ""])).sorted(), id: \.self) {
            Text($0).tag($0)
          }
        }.frame(height: 28)
      } else {
        TextField(name, text: Binding(get: { value.string ?? "" }, set: { value = .string($0) }))
          .verbatimInput().autocorrectionDisabled().frame(height: 28)
      }
    } else if case .number(let number) = value {
      TextField(
        name, value: Binding(get: { number }, set: { value = .number($0) }), format: .number
      )
      .frame(height: 28)
    } else {
      NavigationLink {
        AnyView(SettingContainer(name: name, value: $value, definition: resolved, schema: schema))
      } label: {
        LabeledContent(name, value: summary)
      }
    }
  }
  private var summary: String {
    switch value {
    case .object(let fields): return "\(fields.count) settings"
    case .array(let items): return "\(items.count) items"
    default: return ""
    }
  }
}

private struct SettingContainer: View {
  let name: String
  @Binding var value: JSONValue
  let definition: JSONValue
  let schema: SettingsSchema
  @State private var newKey = ""
  var body: some View {
    Form {
      if case .object(let fields) = value {
        let properties = object(definition["properties"])
        ForEach(Set(fields.keys).union(properties.keys).sorted(), id: \.self) { key in
          SettingField(
            name: settingLabel(key), value: child(key),
            definition: schema.objectField(key, in: definition) ?? .null, schema: schema
          )
          .contextMenu {
            if fields[key] != nil, schema.canRemoveField(key, in: definition) {
              Button("Remove override", role: .destructive) { value = value.removing(key) }
            }
          }
        }
        if schema.allowsNewObjectKeys(definition) {
          HStack {
            TextField("Name", text: $newKey).verbatimInput().frame(height: 28)
            Button("Add") {
              guard let field = schema.objectField(newKey, in: definition) else { return }
              value = value.setting(newKey, to: schema.initialValue(field))
              newKey = ""
            }.disabled(
              newKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                || fields[newKey] != nil || schema.objectField(newKey, in: definition) == nil
            ).frame(height: 28)
          }
        }
      } else if case .array(let items) = value {
        ForEach(items.indices, id: \.self) { index in
          SettingField(
            name: "Item \(index + 1)", value: element(index), definition: definition["items"],
            schema: schema)
        }.onDelete { indices in
          var updated = items
          for index in indices.sorted(by: >) { updated.remove(at: index) }
          value = .array(updated)
        }
        Button("Add item", systemImage: "plus") {
          value = .array(items + [schema.initialValue(definition["items"])])
        }.frame(height: 28)
      }
    }.settingsList().navigationTitle(name).compactNavigationTitle()
  }
  private func object(_ value: JSONValue) -> [String: JSONValue] {
    if case .object(let fields) = value { return fields }
    return [:]
  }
  private func child(_ key: String) -> Binding<JSONValue> {
    Binding(get: { value[key] }, set: { value = value.setting(key, to: $0) })
  }
  private func element(_ index: Int) -> Binding<JSONValue> {
    Binding(
      get: { value.array.indices.contains(index) ? value.array[index] : .null },
      set: { newValue in
        var items = value.array
        guard items.indices.contains(index) else { return }
        items[index] = newValue
        value = .array(items)
      })
  }
}
