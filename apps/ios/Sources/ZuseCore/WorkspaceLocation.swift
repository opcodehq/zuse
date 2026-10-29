import Foundation

/// Restoration references the credential catalog rather than making another copy of its tokens.
public struct WorkspaceLocation: Codable, Sendable, Equatable {
  public enum Kind: String, Codable, Sendable { case computer, cloud, environment }
  public let kind: Kind
  public let id: String
  public let account: String?
  public init(kind: Kind, id: String, account: String? = nil) {
    self.kind = kind
    self.id = id
    self.account = account
  }
  public init(from decoder: Decoder) throws {
    let value = try JSONValue(from: decoder)
    account = value["account"].string
    if let name = value["kind"].string, let kind = Kind(rawValue: name), let id = value["id"].string
    {
      self.kind = kind
      self.id = id
      return
    }
    // Accept the earlier Source enum encoding, but never re-encode embedded credentials.
    if value["source"]["remote"]["_0"] != .null {
      kind = .computer
      id = try value["source"]["remote"]["_0"].decode(RemoteConnection.self).id
    } else if let id = value["source"]["cloud"]["_0"].string {
      kind = .cloud
      self.id = id
    } else if let id = value["source"]["environment"]["_0"].string {
      kind = .environment
      self.id = id
    } else {
      throw ClientError("The saved workspace location is invalid.")
    }
  }
}
