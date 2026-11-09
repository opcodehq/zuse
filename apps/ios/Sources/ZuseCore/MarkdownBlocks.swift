import Foundation

public struct MarkdownBlock: Identifiable, Equatable, Sendable {
  public enum Kind: Sendable { case text, diagram }
  public let id: Int
  public let kind: Kind
  public let content: String
  public static func parse(_ markdown: String) -> [MarkdownBlock] {
    var blocks: [MarkdownBlock] = []
    var text: [String] = []
    var diagram: [String] = []
    var fence: (character: Character, length: Int, diagram: Bool)?
    func appendText() {
      if !text.isEmpty {
        blocks.append(
          MarkdownBlock(id: blocks.count, kind: .text, content: text.joined(separator: "\n")))
        text = []
      }
    }
    for line in markdown.components(separatedBy: "\n") {
      let trimmed = line.drop(while: { $0 == " " })
      let indent = line.count - trimmed.count
      let character = trimmed.first ?? " "
      let length = trimmed.prefix(while: { $0 == character }).count
      if let active = fence {
        if indent <= 3, character == active.character, length >= active.length,
          trimmed.dropFirst(length).trimmingCharacters(in: .whitespaces).isEmpty
        {
          if active.diagram {
            appendText()
            blocks.append(
              MarkdownBlock(
                id: blocks.count, kind: .diagram,
                content: diagram.dropFirst().joined(separator: "\n")))
            diagram = []
          } else {
            text.append(line)
          }
          fence = nil
        } else if active.diagram {
          diagram.append(line)
        } else {
          text.append(line)
        }
      } else if indent <= 3, character == "`" || character == "~", length >= 3 {
        let language = trimmed.dropFirst(length).trimmingCharacters(in: .whitespaces)
        let isDiagram = language.lowercased() == "mermaid"
        fence = (character, length, isDiagram)
        if isDiagram { diagram = [line] } else { text.append(line) }
      } else {
        text.append(line)
      }
    }
    if !diagram.isEmpty { text.append(contentsOf: diagram) }
    appendText()
    return blocks
  }
}
