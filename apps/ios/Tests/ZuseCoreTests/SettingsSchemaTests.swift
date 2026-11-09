import Foundation
import Testing

@testable import ZuseCore

@Test func settingsSchemaResolvesOptionalReferencesAndTypedDefaults() throws {
  let document: JSONValue = .object([
    "$defs": .object([
      "Mode": .object([
        "type": .string("string"), "enum": .array([.string("safe"), .string("auto")]),
      ])
    ])
  ])
  let schema = SettingsSchema(document: document)
  let optional: JSONValue = .object([
    "anyOf": .array([
      .object(["$ref": .string("#/$defs/Mode")]), .object(["type": .string("null")]),
    ])
  ])
  #expect(schema.resolve(optional)["enum"].array == [.string("safe"), .string("auto")])
  #expect(schema.initialValue(optional) == .string("safe"))
  let object: JSONValue = .object([
    "type": .string("object"), "required": .array([.string("enabled")]),
    "properties": .object(["enabled": .object(["type": .string("boolean")])]),
  ])
  #expect(schema.initialValue(object) == .object(["enabled": .bool(false)]))
}

@Test func settingsPatchContainsOnlyChangedTopLevelFields() {
  let original: JSONValue = .object([
    "theme": .string("system"), "models": .object(["a": .bool(true)]), "version": .number(1),
  ])
  let edited = original.setting("models", to: .object(["a": .bool(false)]))
  #expect(
    SettingsSchema.patch(original: original, edited: edited) == [
      "models": .object(["a": .bool(false)])
    ])
  #expect(SettingsSchema.patch(original: original, edited: original).isEmpty)
}

@Test func generatedCloudAuthMetadataResolvesClassReferences() throws {
  // Exercise the actual shipped metadata, including Effect's root $ref class encoding.
  let root = URL(fileURLWithPath: #filePath).resolvingSymlinksInPath()
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
  let document = try JSONDecoder().decode(
    JSONValue.self, from: Data(contentsOf: root.appendingPathComponent("App/SettingsSchema.json")))
  let auth = SettingsSchema(document: document["cloudAuthConfigure"])
  #expect(
    auth.resolve(auth.field("providerId"))["enum"].array.compactMap(\.string) == [
      "claude", "codex", "cursor", "grok",
    ])
  #expect(
    auth.resolve(auth.field("method"))["enum"].array.compactMap(\.string) == [
      "subscription", "api-key", "custom",
    ])
  #expect(auth.properties["baseUrl"] != nil)
  #expect(auth.properties["modelProvider"] != nil)
  let login = SettingsSchema(document: document["cloudAuthLogin"])
  #expect(
    login.resolve(login.field("providerId"))["enum"].array.compactMap(\.string) == [
      "codex", "grok",
    ])
}

@Test func generatedProviderSettingsSupportCustomAgentsAndProtectRequiredFields() throws {
  let root = URL(fileURLWithPath: #filePath).resolvingSymlinksInPath()
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
  let document = try JSONDecoder().decode(
    JSONValue.self, from: Data(contentsOf: root.appendingPathComponent("App/SettingsSchema.json")))
  let schema = SettingsSchema(document: document)
  let providers = schema.field("providerEnabled")
  #expect(schema.allowsNewObjectKeys(providers))
  let custom = try #require(schema.objectField("acp-custom-agent", in: providers))
  #expect(schema.initialValue(custom) == .bool(false))
  #expect(schema.objectField("not-a-provider", in: providers) == nil)
  #expect(!schema.canRemoveField("claude", in: providers))
  #expect(schema.canRemoveField("acp-custom-agent", in: providers))
  #expect(schema.allowsCustomString(schema.field("defaultProviderId")))
  #expect(schema.stringChoices(schema.field("defaultProviderId")).contains("claude"))
  #expect(!schema.allowsCustomString(schema.field("defaultRuntimeMode")))
  let original: JSONValue = .object([
    "providerBinaryPaths": .object([
      "claude": .string("/bin/claude"), "codex": .string("/bin/codex"),
    ])
  ])
  let edited = original.setting(
    "providerBinaryPaths", to: original["providerBinaryPaths"].removing("claude"))
  #expect(
    SettingsSchema.patch(original: original, edited: edited) == [
      "providerBinaryPaths": .object(["codex": .string("/bin/codex")])
    ])
}

@Test func referencedStringUnionsKeepEveryChoiceAndTheirCustomInput() {
  let schema = SettingsSchema(
    document: .object([
      "$defs": .object([
        "Provider": .object([
          "anyOf": .array([
            .object(["type": .string("string"), "enum": .array([.string("claude")])]),
            .object(["type": .string("string"), "pattern": .string("^acp-")]),
            .object(["type": .string("null")]),
          ])
        ])
      ])
    ]))
  let definition: JSONValue = .object(["$ref": .string("#/$defs/Provider")])
  #expect(schema.stringChoices(definition) == ["claude"])
  #expect(schema.allowsCustomString(definition))
}
