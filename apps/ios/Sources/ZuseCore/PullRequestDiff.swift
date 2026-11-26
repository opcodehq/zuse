import Foundation

/// One file of a pull request's unified diff (`gh pr diff`), split into hunks so the viewer can
/// show "N unmodified lines" between them.
public struct PullRequestDiffFile: Sendable, Identifiable, Hashable {
  public enum Change: Sendable, Hashable { case added, deleted, renamed, modified }
  public var path: String
  public var oldPath: String?
  public var change: Change
  public var binary: Bool
  public var hunks: [PullRequestDiffHunk]
  public var id: String { path }
  public var additions: Int {
    hunks.reduce(0) { $0 + $1.lines.filter { $0.kind == .addition }.count }
  }
  public var deletions: Int {
    hunks.reduce(0) { $0 + $1.lines.filter { $0.kind == .deletion }.count }
  }
  public var fileName: String { path.split(separator: "/").last.map(String.init) ?? path }
  /// The containing folder, empty at the repository root.
  public var directory: String {
    guard let slash = path.lastIndex(of: "/") else { return "" }
    return String(path[..<slash])
  }
  public var fileExtension: String {
    let name = fileName
    guard let dot = name.lastIndex(of: "."), dot != name.startIndex else { return "" }
    return String(name[name.index(after: dot)...]).lowercased()
  }
}

public struct PullRequestDiffHunk: Sendable, Identifiable, Hashable {
  public var id: Int
  public var oldStart: Int
  public var oldCount: Int
  public var newStart: Int
  public var newCount: Int
  /// The function or scope GitHub prints after the second `@@`.
  public var context: String
  public var lines: [DiffLine]
  /// Unchanged lines between the previous hunk (or the top of the file) and this one.
  public var hiddenBefore: Int
}

extension DiffLine: Hashable {
  public func hash(into hasher: inout Hasher) {
    hasher.combine(id)
    hasher.combine(text)
  }
}

public enum PullRequestDiff {
  public static func parse(_ patch: String) -> [PullRequestDiffFile] {
    var files: [PullRequestDiffFile] = []
    var current: PullRequestDiffFile?
    var hunk: PullRequestDiffHunk?
    var old = 0
    var new = 0
    var lineID = 0

    func closeHunk() {
      guard let finished = hunk else { return }
      current?.hunks.append(finished)
      hunk = nil
    }
    func closeFile() {
      closeHunk()
      if let file = current { files.append(file) }
      current = nil
    }

    for raw in patch.split(separator: "\n", omittingEmptySubsequences: false) {
      let text = raw.hasSuffix("\r") ? String(raw.dropLast()) : String(raw)
      if text.hasPrefix("diff --git ") {
        closeFile()
        let (a, b) = headerPaths(String(text.dropFirst("diff --git ".count)))
        current = PullRequestDiffFile(
          path: b ?? a ?? "", oldPath: nil, change: .modified, binary: false, hunks: [])
        if let a, let b, a != b { current?.oldPath = a }
        continue
      }
      guard current != nil else { continue }
      if hunk != nil, !text.hasPrefix("@@ ") {
        // Inside a hunk: body lines until its counts are used up.
        if text.hasPrefix("\\") { continue }
        let kind: DiffLine.Kind
        var oldNumber: Int?
        var newNumber: Int?
        if text.hasPrefix("+") {
          kind = .addition
          newNumber = new
          new += 1
        } else if text.hasPrefix("-") {
          kind = .deletion
          oldNumber = old
          old += 1
        } else {
          kind = .context
          oldNumber = old
          newNumber = new
          old += 1
          new += 1
        }
        lineID += 1
        hunk?.lines.append(
          DiffLine(
            id: lineID, text: String(text.dropFirst(min(1, text.count))), kind: kind,
            oldLine: oldNumber, newLine: newNumber))
        if let range = hunk, old >= range.oldStart + range.oldCount,
          new >= range.newStart + range.newCount
        {
          closeHunk()
        }
        continue
      }
      if text.hasPrefix("@@ ") {
        closeHunk()
        guard let header = HunkHeader(text) else { continue }
        let previousEnd = current?.hunks.last.map { $0.newStart + $0.newCount } ?? 1
        old = header.oldStart
        new = header.newStart
        hunk = PullRequestDiffHunk(
          id: (current?.hunks.count ?? 0), oldStart: header.oldStart, oldCount: header.oldCount,
          newStart: header.newStart, newCount: header.newCount, context: header.context,
          lines: [], hiddenBefore: max(0, header.newStart - previousEnd))
        if header.oldCount == 0 && header.newCount == 0 { closeHunk() }
        continue
      }
      if text.hasPrefix("new file mode") {
        current?.change = .added
      } else if text.hasPrefix("deleted file mode") {
        current?.change = .deleted
      } else if text.hasPrefix("rename from ") {
        current?.oldPath = unquote(String(text.dropFirst("rename from ".count)))
        current?.change = .renamed
      } else if text.hasPrefix("rename to ") {
        current?.path = unquote(String(text.dropFirst("rename to ".count)))
        current?.change = .renamed
      } else if text.hasPrefix("Binary files ") || text == "GIT binary patch" {
        current?.binary = true
      } else if text.hasPrefix("+++ ") {
        let path = String(text.dropFirst(4))
        if path != "/dev/null" { current?.path = stripPrefix(unquote(path)) }
      } else if text.hasPrefix("--- ") {
        let path = String(text.dropFirst(4))
        if path == "/dev/null" { current?.change = .added }
      }
      if text == "+++ /dev/null" { current?.change = .deleted }
    }
    closeFile()
    return files
  }

  private struct HunkHeader {
    let oldStart: Int
    let oldCount: Int
    let newStart: Int
    let newCount: Int
    let context: String
    init?(_ text: String) {
      // @@ -12,7 +12,9 @@ fn name()
      let body = text.dropFirst(3)
      guard let close = body.range(of: " @@") else { return nil }
      let ranges = body[..<close.lowerBound].split(separator: " ")
      guard ranges.count == 2, ranges[0].hasPrefix("-"), ranges[1].hasPrefix("+") else {
        return nil
      }
      func parse(_ part: Substring) -> (Int, Int)? {
        let numbers = part.dropFirst().split(separator: ",", omittingEmptySubsequences: false)
        guard let start = numbers.first.flatMap({ Int($0) }) else { return nil }
        let count = numbers.count > 1 ? Int(numbers[1]) ?? 1 : 1
        return (start, count)
      }
      guard let old = parse(ranges[0]), let new = parse(ranges[1]) else { return nil }
      (oldStart, oldCount) = old
      (newStart, newCount) = new
      context = String(body[close.upperBound...]).trimmingCharacters(in: .whitespaces)
    }
  }

  /// `a/path b/path`, including C-quoted paths with spaces.
  static func headerPaths(_ text: String) -> (String?, String?) {
    var tokens: [String] = []
    var index = text.startIndex
    while index < text.endIndex {
      if text[index] == " " {
        index = text.index(after: index)
        continue
      }
      if text[index] == "\"" {
        var token = "\""
        var cursor = text.index(after: index)
        while cursor < text.endIndex {
          let character = text[cursor]
          token.append(character)
          cursor = text.index(after: cursor)
          if character == "\\", cursor < text.endIndex {
            token.append(text[cursor])
            cursor = text.index(after: cursor)
          } else if character == "\"" {
            break
          }
        }
        tokens.append(token)
        index = cursor
      } else {
        // Unquoted: split on " b/" so paths containing spaces still separate.
        if tokens.isEmpty, let split = text.range(of: " b/", range: index..<text.endIndex) {
          tokens.append(String(text[index..<split.lowerBound]))
          tokens.append(String(text[text.index(after: split.lowerBound)...]))
          break
        }
        let end = text[index...].firstIndex(of: " ") ?? text.endIndex
        tokens.append(String(text[index..<end]))
        index = end
      }
    }
    let paths = tokens.map { stripPrefix(unquote($0)) }
    return (paths.first, paths.count > 1 ? paths[1] : nil)
  }

  private static func stripPrefix(_ path: String) -> String {
    if path.hasPrefix("a/") || path.hasPrefix("b/") { return String(path.dropFirst(2)) }
    return path
  }

  /// Undo Git's C quoting (`"caf\303\251.txt"`).
  static func unquote(_ value: String) -> String {
    guard value.count >= 2, value.hasPrefix("\""), value.hasSuffix("\"") else { return value }
    var bytes: [UInt8] = []
    var iterator = Array(value.dropFirst().dropLast().utf8).makeIterator()
    while let byte = iterator.next() {
      guard byte == UInt8(ascii: "\\"), let next = iterator.next() else {
        bytes.append(byte)
        continue
      }
      switch next {
      case UInt8(ascii: "n"): bytes.append(10)
      case UInt8(ascii: "t"): bytes.append(9)
      case UInt8(ascii: "r"): bytes.append(13)
      case UInt8(ascii: "\""): bytes.append(34)
      case UInt8(ascii: "\\"): bytes.append(92)
      case UInt8(ascii: "0")...UInt8(ascii: "7"):
        var octal = Int(next - UInt8(ascii: "0"))
        for _ in 0..<2 {
          guard let digit = iterator.next() else { break }
          octal = octal * 8 + Int(digit - UInt8(ascii: "0"))
        }
        bytes.append(UInt8(truncatingIfNeeded: octal))
      default: bytes.append(next)
      }
    }
    return String(decoding: bytes, as: UTF8.self)
  }
}
