import Foundation
import ZuseCore

/// Keep retry identity across sheet dismissal, app termination, and an uncertain HTTP response.
struct CloudIntent: Codable {
  let body: JSONValue
  let id: String
  static func key(account: String, operation: String) -> String {
    "cloud-intent-" + ArchiveStore.storageID(account + "|" + operation)
  }
  static func load(key: String) throws -> CloudIntent? {
    guard let bytes = try Vault.read(key) else { return nil }
    return try JSONDecoder().decode(CloudIntent.self, from: bytes)
  }
  static func prepare(_ body: JSONValue, key: String) throws -> CloudIntent {
    if let existing = try load(key: key), existing.body == body { return existing }
    let intent = CloudIntent(body: body, id: UUID().uuidString)
    try Vault.write(JSONEncoder().encode(intent), key: key)
    return intent
  }
  func requestBody() -> JSONValue { body.setting("idempotencyKey", to: .string(id)) }
}
