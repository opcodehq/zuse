import Foundation

/// Presentation metadata comes from SettingsPatch, generated at build preparation time.
public struct SettingsSchema: Sendable {
  public let document: JSONValue
  public init(document: JSONValue) { self.document = document }
  public var properties: [String: JSONValue] {
    if case .object(let fields) = resolve(document)["properties"] { return fields }
    return [:]
  }
  public func field(_ name: String) -> JSONValue { properties[name] ?? .null }
  public func objectField(_ name: String, in definition: JSONValue) -> JSONValue? {
    let definition = resolve(definition)
    if case .object(let properties) = definition["properties"], let field = properties[name] {
      return field
    }
    if case .object(let patterns) = definition["patternProperties"] {
      for pattern in patterns.keys.sorted()
      where name.range(of: pattern, options: .regularExpression) != nil {
        return patterns[pattern]
      }
    }
    if case .object = definition["additionalProperties"] {
      return definition["additionalProperties"]
    }
    return nil
  }
  public func allowsNewObjectKeys(_ definition: JSONValue) -> Bool {
    let definition = resolve(definition)
    if case .object = definition["additionalProperties"] { return true }
    if case .object(let patterns) = definition["patternProperties"] { return !patterns.isEmpty }
    return false
  }
  public func canRemoveField(_ name: String, in definition: JSONValue) -> Bool {
    !resolve(definition)["required"].array.compactMap(\.string).contains(name)
  }
  private func alternatives(_ definition: JSONValue, depth: Int = 0) -> [JSONValue] {
    guard depth < 20 else { return [] }
    if let target = referenced(definition) {
      return alternatives(target, depth: depth + 1)
    }
    let options = definition["anyOf"].array + definition["oneOf"].array
    if options.isEmpty { return [definition] }
    return options.flatMap { alternatives($0, depth: depth + 1) }
  }
  public func stringChoices(_ definition: JSONValue) -> [String] {
    var seen = Set<String>()
    return alternatives(definition).flatMap { option in
      option["enum"].array.compactMap(\.string) + (option["const"].string.map { [$0] } ?? [])
    }.filter { seen.insert($0).inserted }
  }
  public func allowsCustomString(_ definition: JSONValue) -> Bool {
    alternatives(definition).contains {
      $0["type"].string == "string" && $0["enum"].array.isEmpty && $0["const"] == .null
    }
  }
  private func referenced(_ schema: JSONValue) -> JSONValue? {
    guard let reference = schema["$ref"].string, reference.hasPrefix("#/$defs/") else { return nil }
    let name = String(reference.dropFirst(8)).replacingOccurrences(of: "~1", with: "/")
      .replacingOccurrences(of: "~0", with: "~")
    return document["$defs"][name]
  }
  public func resolve(_ schema: JSONValue, value: JSONValue = .null, depth: Int = 0) -> JSONValue {
    guard depth < 20 else { return .object([:]) }
    if let target = referenced(schema) {
      return resolve(target, value: value, depth: depth + 1)
    }
    let alternatives = schema["anyOf"].array + schema["oneOf"].array
    guard !alternatives.isEmpty else { return schema }
    let options = alternatives.filter { $0["type"].string != "null" }
    let match =
      options.first { option in
        let resolved = resolve(option, value: value, depth: depth + 1)
        switch (value, resolved["type"].string) {
        case (.string, "string"), (.bool, "boolean"), (.number, "number"), (.number, "integer"),
          (.object, "object"), (.array, "array"):
          return true
        default: return false
        }
      } ?? options.first ?? .object([:])
    return resolve(match, value: value, depth: depth + 1)
  }
  public func initialValue(_ schema: JSONValue, depth: Int = 0) -> JSONValue {
    guard depth < 12 else { return .null }
    let schema = resolve(schema)
    if schema["default"] != .null { return schema["default"] }
    if schema["const"] != .null { return schema["const"] }
    if let first = schema["enum"].array.first { return first }
    switch schema["type"].string {
    case "string": return .string("")
    case "boolean": return .bool(false)
    case "integer", "number": return .number(0)
    case "array": return .array([])
    case "object":
      var fields: [String: JSONValue] = [:]
      for name in schema["required"].array.compactMap(\.string) {
        fields[name] = initialValue(schema["properties"][name], depth: depth + 1)
      }
      return .object(fields)
    default: return .string("")
    }
  }
  public static func patch(original: JSONValue, edited: JSONValue) -> [String: JSONValue] {
    guard case .object(let fields) = edited else { return [:] }
    return fields.filter { $0.value != original[$0.key] }
  }
}
