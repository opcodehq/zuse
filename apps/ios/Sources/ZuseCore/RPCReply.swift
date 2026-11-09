import Foundation

/// Decode the shared Effect RPC response envelope independently of the socket platform.
enum RPCReply: Sendable {
  case chunk(String, [JSONValue])
  case success(String, JSONValue)
  case failure(String, ClientError)

  init?(_ frame: JSONValue) {
    guard let id = frame["requestId"].string else { return nil }
    switch frame["_tag"].string {
    case "Chunk": self = .chunk(id, frame["values"].array)
    case "Exit":
      if frame["exit"]["_tag"].string == "Success" {
        self = .success(id, frame["exit"]["value"])
      } else {
        let error = frame["exit"]["cause"].array.first?["error"] ?? .null
        self = .failure(
          id,
          ClientError(
            error["message"].string ?? error["_tag"].string
              ?? "The runtime rejected the request.", code: error["_tag"].string))
      }
    default: return nil
    }
  }
}
