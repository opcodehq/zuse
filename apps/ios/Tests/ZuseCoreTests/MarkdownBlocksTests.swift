import Testing

@testable import ZuseCore

@Test func diagramFencesPreserveSurroundingMarkdown() {
  let blocks = MarkdownBlock.parse("Before\n```mermaid\nflowchart LR\n A --> B\n```\nAfter")
  #expect(blocks.count == 3)
  #expect(blocks[0].content == "Before")
  #expect(blocks[1].kind == .diagram)
  #expect(blocks[1].content == "flowchart LR\n A --> B")
  #expect(blocks[2].content == "After")
}
@Test func incompleteAndNestedFencesStayReadableDuringStreaming() {
  let incomplete = "```mermaid\nflowchart LR\nA -->"
  #expect(MarkdownBlock.parse(incomplete).first?.content == incomplete)
  let example = "````markdown\n```mermaid\nA --> B\n```\n````"
  #expect(MarkdownBlock.parse(example).count == 1)
  #expect(MarkdownBlock.parse(example).first?.kind == .text)
  #expect(MarkdownBlock.parse(example).first?.content == example)
}
