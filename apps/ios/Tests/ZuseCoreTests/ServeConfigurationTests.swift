import Foundation
import Testing

@testable import ZuseCore

@Test func startingServePreservesTheExistingRuntimeChoices() {
  let options = ServeConfiguration(
    .object([
      "tailscale": .bool(true), "lan": .bool(false), "host": .string("127.0.0.2"),
      "port": .number(4899), "sshManaged": .bool(true), "noAccount": .bool(true),
    ]))
  #expect(options.tailscale)
  #expect(
    options.startArguments(tailscale: true, lan: false) == [
      "serve", "start", "--tailscale", "--no-account", "--ssh-managed", "--host", "127.0.0.2",
      "--port", "4899",
    ])
  #expect(
    options.startArguments(tailscale: false, lan: true) == [
      "serve", "start", "--lan", "--no-account", "--port", "4899",
    ])
  let shared = ServeConfiguration(
    .object([
      "lan": .bool(true), "host": .string("0.0.0.0"), "port": .number(4899),
    ]))
  #expect(
    shared.startArguments(tailscale: false, lan: false) == ["serve", "start", "--port", "4899"])
  for invalid in [0.0, 65536, 1.5] {
    #expect(ServeConfiguration(.object(["port": .number(invalid)])).port == nil)
  }
}

@Test func runtimeConfigurationReadsNeitherInitializeNorRewriteADataDirectory() throws {
  let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  defer { try? FileManager.default.removeItem(at: directory) }
  #expect(try ServeRuntimeFile.configuration.read(directory: directory.path) == nil)
  #expect(!FileManager.default.fileExists(atPath: directory.path))
  try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  let file = directory.appendingPathComponent("serve-settings.json")
  let original = Data(#"{"lan":true,"port":4899}"#.utf8)
  try original.write(to: file)
  let saved = try ServeRuntimeFile.configuration.read(directory: directory.path)
  let result = try #require(saved)
  #expect(result["port"].int == 4899)
  #expect(try Data(contentsOf: file) == original)
  try Data(repeating: 32, count: 64 * 1024 + 1).write(to: file)
  #expect(throws: ClientError.self) {
    try ServeRuntimeFile.configuration.read(directory: directory.path)
  }
}
