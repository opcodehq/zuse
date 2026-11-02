import Foundation

public enum JSONValue: Codable, Sendable, Equatable {
  case object([String: JSONValue])
  case array([JSONValue])
  case string(String)
  case number(Double)
  case bool(Bool)
  case null

  public init(from decoder: Decoder) throws {
    let c = try decoder.singleValueContainer()
    if c.decodeNil() {
      self = .null
    } else if let value = try? c.decode(Bool.self) {
      self = .bool(value)
    } else if let value = try? c.decode(Double.self) {
      self = .number(value)
    } else if let value = try? c.decode(String.self) {
      self = .string(value)
    } else if let value = try? c.decode([JSONValue].self) {
      self = .array(value)
    } else {
      self = .object(try c.decode([String: JSONValue].self))
    }
  }
  public func encode(to encoder: Encoder) throws {
    var c = encoder.singleValueContainer()
    switch self {
    case .object(let v): try c.encode(v)
    case .array(let v): try c.encode(v)
    case .string(let v): try c.encode(v)
    case .number(let v): try c.encode(v)
    case .bool(let v): try c.encode(v)
    case .null: try c.encodeNil()
    }
  }
  public subscript(_ key: String) -> JSONValue {
    if case .object(let v) = self { return v[key] ?? .null }
    return .null
  }
  public var string: String? {
    if case .string(let v) = self { return v }
    return nil
  }
  public var int: Int? {
    if case .number(let v) = self, v.isFinite, v >= Double(Int.min), v < Double(Int.max) {
      return Int(v)
    }
    return nil
  }
  public var array: [JSONValue] {
    if case .array(let v) = self { return v }
    return []
  }
  public var bool: Bool { self == .bool(true) }
  public var pretty: String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    return (try? String(decoding: encoder.encode(self), as: UTF8.self)) ?? ""
  }
  public func decode<T: Decodable>(_ type: T.Type) throws -> T {
    try JSONDecoder().decode(type, from: JSONEncoder().encode(self))
  }
}

public struct ClientError: LocalizedError, Sendable {
  public let message: String
  public let code: String?
  public init(_ message: String, code: String? = nil) {
    self.message = message
    self.code = code
  }
  public var errorDescription: String? { message }
  public var requiresUserAction: Bool {
    [
      "authentication-required", "access-unavailable", "protocol-incompatible", "identity-changed",
      "ConnectAuthError", "cloud_workspace_unavailable", "workspace_runtime_update_required",
      "cloud_image_rebuild_required", "cloud_auth_setup_required", "workspace-paused",
    ].contains(code ?? "")
  }
}

extension JSONValue {
  public func removing(_ key: String) -> JSONValue {
    guard case .object(var fields) = self else { return self }
    fields.removeValue(forKey: key)
    return .object(fields)
  }
  public func setting(_ key: String, to value: JSONValue) -> JSONValue {
    guard case .object(var fields) = self else { return self }
    fields[key] = value
    return .object(fields)
  }
}
