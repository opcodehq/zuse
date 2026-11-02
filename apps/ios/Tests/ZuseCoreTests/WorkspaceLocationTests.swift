import Foundation
import Testing

@testable import ZuseCore

@Test func restorationReferencesProfilesWithoutRetainingCredentials() throws {
  let connection = RemoteConnection(address: "wss://computer.test/rpc", token: "private-token")
  let location = WorkspaceLocation(kind: .computer, id: connection.id)
  let data = try JSONEncoder().encode(location)
  #expect(!String(decoding: data, as: UTF8.self).contains(connection.token))
  #expect(try JSONDecoder().decode(WorkspaceLocation.self, from: data) == location)
  let profiles = ConnectionProfiles.saving(connection, in: [])
  #expect(profiles.first { $0.id == location.id } == connection)
  let forgotten = profiles.filter { $0.id != connection.id }
  #expect(forgotten.first { $0.id == location.id } == nil)
}

@Test func earlierRestorationRecordsMigrateToReferences() throws {
  let data = Data(
    #"{"source":{"remote":{"_0":{"address":"wss://computer.test/rpc","token":"old-secret"}}},"account":"old-account"}"#
      .utf8)
  let location = try JSONDecoder().decode(WorkspaceLocation.self, from: data)
  #expect(location.kind == .computer)
  #expect(location.id == "wss://computer.test/rpc")
  let canonical = try JSONEncoder().encode(location)
  #expect(!String(decoding: canonical, as: UTF8.self).contains("old-secret"))
  let cloud = try JSONDecoder().decode(
    WorkspaceLocation.self,
    from: Data(#"{"source":{"cloud":{"_0":"workspace"}},"account":"account"}"#.utf8))
  #expect(cloud == WorkspaceLocation(kind: .cloud, id: "workspace", account: "account"))
}
