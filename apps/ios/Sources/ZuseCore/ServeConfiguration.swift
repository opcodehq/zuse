import Foundation

/// The existing CLI owns this configuration. Native controls read it without rewriting its files.
public struct ServeConfiguration: Sendable {
  public let tailscale: Bool
  public let lan: Bool
  public let host: String?
  public let port: Int?
  public let sshManaged: Bool
  public let noAccount: Bool
  public init(_ value: JSONValue) {
    tailscale = value["tailscale"].bool
    lan = value["lan"].bool
    host = value["host"].string
    if case .number(let number) = value["port"], number.isFinite, number.rounded() == number,
      (1...65535).contains(number)
    {
      port = Int(number)
    } else {
      port = nil
    }
    sshManaged = value["sshManaged"].bool
    noAccount = value["noAccount"].bool
  }
  public func startArguments(tailscale: Bool, lan: Bool) -> [String] {
    var arguments = ["serve", "start"]
    if tailscale { arguments.append("--tailscale") }
    if lan { arguments.append("--lan") }
    if noAccount { arguments.append("--no-account") }
    // An explicit local-sharing change replaces the old bind choice. Otherwise preserve it.
    if self.lan == lan {
      if sshManaged { arguments.append("--ssh-managed") }
      if let host { arguments += ["--host", host] }
    }
    if let port { arguments += ["--port", String(port)] }
    return arguments
  }
}

public enum ServeRuntimeFile: String, Sendable {
  case configuration = "serve-settings.json"
  case pairing = "pairing.json"
  public func read(directory: String) throws -> JSONValue? {
    let file = URL(fileURLWithPath: directory).appendingPathComponent(rawValue)
    guard FileManager.default.fileExists(atPath: file.path) else { return nil }
    let handle = try FileHandle(forReadingFrom: file)
    defer { try? handle.close() }
    let data = try handle.read(upToCount: 64 * 1024 + 1) ?? Data()
    guard data.count <= 64 * 1024 else {
      throw ClientError("The service configuration file is too large.")
    }
    let value = try JSONDecoder().decode(JSONValue.self, from: data)
    guard case .object = value else {
      throw ClientError("The service configuration file is invalid.")
    }
    return value
  }
}
