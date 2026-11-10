import Foundation

public struct DiffLine: Identifiable, Sendable, Equatable {
  public enum Kind: Sendable { case header, context, addition, deletion }
  public let id: Int
  public let text: String
  public let kind: Kind
  public let oldLine: Int?
  public let newLine: Int?
  public static func parse(_ patch: String) -> [DiffLine] {
    var old = 0
    var new = 0
    var inHunk = false
    return patch.components(separatedBy: "\n").enumerated().map { index, text in
      var kind = Kind.header
      var oldNumber: Int?
      var newNumber: Int?
      if text.hasPrefix("@@ ") {
        let parts = text.split(separator: " ")
        if parts.count >= 3 {
          old = parts[1].dropFirst().split(separator: ",").first.flatMap { Int($0) } ?? 0
          new = parts[2].dropFirst().split(separator: ",").first.flatMap { Int($0) } ?? 0
          inHunk = true
        }
      } else if text.hasPrefix("diff --git") {
        inHunk = false
      } else if inHunk && text.hasPrefix("+") {
        kind = .addition
        newNumber = new
        new += 1
      } else if inHunk && text.hasPrefix("-") {
        kind = .deletion
        oldNumber = old
        old += 1
      } else if inHunk && text.hasPrefix(" ") {
        kind = .context
        oldNumber = old
        newNumber = new
        old += 1
        new += 1
      }
      return DiffLine(id: index, text: text, kind: kind, oldLine: oldNumber, newLine: newNumber)
    }
  }
}

/// Exact Git header matching, including renamed and C-quoted Unicode paths.
public enum GitPatch {
  private static func quote(_ path: String, unicode: Bool) -> String {
    let needsQuotes = path.utf8.contains {
      $0 < 32 || $0 == 127 || $0 == 34 || $0 == 92 || (unicode && $0 >= 128)
    }
    guard needsQuotes else { return path }
    var value = "\""
    for scalar in path.unicodeScalars {
      switch scalar.value {
      case 34: value += "\\\""
      case 92: value += "\\\\"
      case 9: value += "\\t"
      case 10: value += "\\n"
      case 13: value += "\\r"
      case 7: value += "\\a"
      case 8: value += "\\b"
      case 11: value += "\\v"
      case 12: value += "\\f"
      default:
        if scalar.value < 32 || scalar.value == 127 || (unicode && scalar.value >= 128) {
          for byte in String(scalar).utf8 { value += String(format: "\\%03o", Int(byte)) }
        } else {
          value += String(scalar)
        }
      }
    }
    return value + "\""
  }
  public static func file(_ patch: String, path: String, oldPath: String? = nil) -> String? {
    let headers = Set(
      [true, false].map {
        "diff --git \(quote("a/" + (oldPath ?? path), unicode: $0)) \(quote("b/" + path, unicode: $0))"
      })
    return patch.components(separatedBy: "\ndiff --git ").enumerated().map { index, block in
      index == 0 ? block : "diff --git " + block
    }.first { headers.contains($0.components(separatedBy: "\n").first ?? "") }
  }
}
