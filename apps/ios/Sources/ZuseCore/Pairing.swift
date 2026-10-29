import Foundation

public enum PairingClient {
  public static func redeem(
    _ link: PairingLink, deviceID: String, label: String, certificatePin: String? = nil
  ) async throws
    -> JSONValue
  {
    let session =
      try certificatePin.map { try PinnedConnection.session(pin: $0) }
      ?? URLSession(
        configuration: .ephemeral, delegate: PairingHTTPDelegate(), delegateQueue: nil)
    defer { session.invalidateAndCancel() }
    var request = URLRequest(url: link.httpURL.appendingPathComponent("pair"))
    request.httpMethod = "POST"
    request.timeoutInterval = 15
    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
    request.httpBody = try JSONEncoder().encode([
      "code": link.code, "deviceId": deviceID, "deviceLabel": label,
    ])
    let (data, response) = try await session.data(for: request)
    guard let response = response as? HTTPURLResponse else {
      throw ClientError("Invalid pairing response.")
    }
    guard response.statusCode == 200 else {
      throw ClientError(
        response.statusCode == 410
          ? "This pairing code expired. Generate a new code on your computer."
          : "Pairing failed. Check the address and pairing code.")
    }
    let result = try JSONDecoder().decode(JSONValue.self, from: data)
    guard let token = result["token"].string, token.hasPrefix("zt_") else {
      throw ClientError("The computer did not return a valid connection token.")
    }
    return result
  }
}
