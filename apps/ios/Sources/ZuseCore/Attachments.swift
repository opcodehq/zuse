import Foundation

public enum Attachments {
  public static let maximumBytes = 100 * 1024 * 1024
  public static let chunkBytes = 2 * 1024 * 1024
  public static func upload(
    _ data: Data, name: String, mimeType: String, sessionID: String, rpc: RPCClient
  ) async throws -> JSONValue {
    if await rpc.canonical {
      guard data.count <= 32 * 1024 * 1024 else {
        throw ClientError("The host supports attachments up to 32 MB.")
      }
      let uploadID = UUID().uuidString
      let bytes = 45 * 1024
      for sequence in 0..<max(1, (data.count + bytes - 1) / bytes) {
        try Task.checkCancellation()
        let start = sequence * bytes
        let end = min(start + bytes, data.count)
        _ = try await rpc.call(
          "UploadChunk",
          [
            "uploadId": .string(uploadID), "seq": .number(Double(sequence)),
            "data": .string(data.subdata(in: start..<end).base64EncodedString()),
          ])
      }
      let result = try await rpc.call(
        "UploadCommit", ["uploadId": .string(uploadID), "fileName": .string(name)])
      guard let path = result["path"].string else {
        throw ClientError("Missing host attachment path.")
      }
      return .object([
        "id": .string(path), "path": .string(path), "uploadId": .string(uploadID),
        "mimeType": .string(mimeType), "originalName": .string(name),
      ])
    }
    guard data.count <= maximumBytes else {
      throw ClientError("Attachments must be 100 MB or smaller.")
    }
    let base: [String: JSONValue] = [
      "sessionId": .string(sessionID), "mimeType": .string(mimeType), "originalName": .string(name),
    ]
    var result: JSONValue = .null
    if data.count <= chunkBytes {
      var payload = base
      payload["bytes"] = .string(data.base64EncodedString())
      result = try await rpc.call("attachments.upload", payload)
    } else {
      let uploadID = UUID().uuidString
      for offset in stride(from: 0, to: data.count, by: chunkBytes) {
        try Task.checkCancellation()
        let end = min(offset + chunkBytes, data.count)
        var payload = base
        payload["bytes"] = .string(data.subdata(in: offset..<end).base64EncodedString())
        payload["uploadId"] = .string(uploadID)
        payload["offset"] = .number(Double(offset))
        payload["totalBytes"] = .number(Double(data.count))
        result = try await rpc.call("attachments.uploadChunk", payload)
        guard (end == data.count) == (result != .null) else {
          throw ClientError("The upload did not complete. Please attach the file again.")
        }
      }
    }
    guard let id = result["id"].string, let mime = result["mimeType"].string else {
      throw ClientError("Invalid attachment response.")
    }
    return .object(["id": .string(id), "mimeType": .string(mime), "originalName": .string(name)])
  }
  public static func read(_ id: String, sessionID: String, rpc: RPCClient) async throws -> (
    Data, String, String
  ) {
    if await rpc.canonical {
      var data = Data()
      var metadata: JSONValue = .null
      var offset = 0
      repeat {
        try Task.checkCancellation()
        metadata = try await rpc.call(
          "ReadAttachmentChunk", ["path": .string(id), "offset": .number(Double(offset))])
        guard let encoded = metadata["data"].string, encoded.utf8.count <= 128 * 1024,
          let chunk = Data(base64Encoded: encoded), let next = metadata["nextOffset"].int,
          next == offset + chunk.count, next <= 32 * 1024 * 1024,
          next > offset || metadata["done"].bool
        else { throw ClientError("Invalid attachment chunk.") }
        data.append(chunk)
        offset = next
      } while !metadata["done"].bool
      return (
        data, metadata["mimeType"].string ?? "application/octet-stream",
        metadata["name"].string ?? "Attachment"
      )
    }
    let result = try await rpc.call(
      "attachments.read", ["id": .string(id), "sessionId": .string(sessionID)])
    guard let encoded = result["bytes"].string, encoded.utf8.count <= maximumBytes * 4 / 3 + 8,
      let data = Data(base64Encoded: encoded), data.count <= maximumBytes
    else { throw ClientError("This attachment could not be read.") }
    return (
      data, result["mimeType"].string ?? "application/octet-stream",
      result["originalName"].string ?? "Attachment"
    )
  }
}
