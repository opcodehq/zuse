import Foundation

public struct ModelProvider: Identifiable, Sendable {
  public struct Model: Decodable, Identifiable, Sendable {
    public let id: String
    public let label: String
    public let available: Bool?
    public let defaultModel: Bool?
    public let optionDescriptors: [OptionDescriptor]?
    public var reasoning: OptionDescriptor? {
      optionDescriptors?.first {
        $0.kind == "select" && ($0.id == "reasoning" || $0.id == "effort")
      }
    }
    public struct OptionDescriptor: Decodable, Sendable {
      public let id: String
      public let kind: String
      public let defaultId: String?
      public let options: [Option]?
      public struct Option: Decodable, Identifiable, Sendable {
        public let id: String
        public let label: String
      }
    }
  }
  public let id: String
  public let name: String
  public let defaultModel: String
  public let models: [Model]
  public static func load(_ rpc: RPCClient) async throws -> [ModelProvider] {
    if await rpc.canonical { return try await parse(CanonicalWorkspace(rpc).catalog()) }
    return try parse(await rpc.call("model.catalog"))
  }
  public static func parse(_ value: JSONValue) throws -> [ModelProvider] {
    guard case .object(let entries) = value["providers"] else {
      throw ClientError("Invalid model catalog.")
    }
    return try entries.sorted { $0.key < $1.key }.compactMap { id, entry in
      let models = try entry["models"].decode([Model].self).filter { $0.available != false }
      guard !models.isEmpty else { return nil }
      let preferred = entry["defaultModelId"].string ?? models.first { $0.defaultModel == true }?.id
      return ModelProvider(
        id: id, name: entry["displayName"].string ?? id,
        defaultModel: models.first { $0.id == preferred }?.id ?? models[0].id, models: models)
    }
  }
}
