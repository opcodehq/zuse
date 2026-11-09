import Foundation
import Testing

@testable import ZuseCore

@Test func queuedReasoningSurvivesEncodingAndRetry() throws {
  let pending = PendingMessage(sessionID: "chat", text: "Review", modelOptions: ["effort": "high"])
  let restored = try JSONDecoder().decode(PendingMessage.self, from: JSONEncoder().encode(pending))
  #expect(restored.id == pending.id)
  #expect(restored.payload["modelOptions"] == .object(["effort": .string("high")]))
  let creation = ChatCreation(
    projectID: "project", providerID: "agent", model: "model", modelOptions: ["effort": "high"])
  #expect(creation.payload["modelOptions"] == restored.payload["modelOptions"])
}
