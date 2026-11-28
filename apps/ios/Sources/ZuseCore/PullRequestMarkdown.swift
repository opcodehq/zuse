import Foundation

/// GitHub comment bodies mix Markdown with HTML (bots especially). The native renderer reads
/// Markdown only, so this turns the common HTML into its Markdown equivalent and drops the rest
/// instead of showing raw tags.
public enum PullRequestMarkdown {
  public static func clean(_ body: String) -> String {
    // Code blocks are left exactly as written; only prose between them is converted.
    var output: [String] = []
    var prose: [String] = []
    var fence: String?
    for line in body.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n") {
      let trimmed = line.trimmingCharacters(in: .whitespaces)
      if let open = fence {
        output.append(line)
        if trimmed.hasPrefix(open) { fence = nil }
      } else if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
        output += [convert(prose.joined(separator: "\n"))].filter { !$0.isEmpty }
        prose = []
        output.append(line)
        fence = String(trimmed.prefix(3))
      } else {
        prose.append(line)
      }
    }
    output += [convert(prose.joined(separator: "\n"))].filter { !$0.isEmpty }
    let joined = output.joined(separator: "\n")
    return replace(#"\n{3,}"#, in: joined, with: "\n\n")
      .trimmingCharacters(in: .whitespacesAndNewlines)
  }

  private static func convert(_ prose: String) -> String {
    var text = prose
    // Hidden notes and machine state.
    text = replace(#"<!--[\s\S]*?-->"#, in: text, with: "")
    // Collapsible sections read as a bold heading followed by their content.
    text = replace(
      #"<summary[^>]*>\s*([\s\S]*?)\s*</summary>"#, in: text, with: "\n**$1**\n\n",
      options: [.caseInsensitive])
    text = replace(#"</?details[^>]*>"#, in: text, with: "\n", options: [.caseInsensitive])
    text = replace(#"<br\s*/?>"#, in: text, with: "\n", options: [.caseInsensitive])
    // Links keep their text and target; images keep their description.
    text = replace(
      #"<a\s[^>]*href\s*=\s*"([^"]+)"[^>]*>([\s\S]*?)</a>"#, in: text, with: "[$2]($1)",
      options: [.caseInsensitive])
    text = replace(
      #"<img\s[^>]*alt\s*=\s*"([^"]+)"[^>]*/?>"#, in: text, with: "$1",
      options: [.caseInsensitive])
    text = replace(
      #"</?(b|strong)>"#, in: text, with: "**", options: [.caseInsensitive])
    text = replace(#"</?(i|em)>"#, in: text, with: "_", options: [.caseInsensitive])
    text = replace(#"</?code>"#, in: text, with: "`", options: [.caseInsensitive])
    text = replace(
      #"<h([1-6])[^>]*>([\s\S]*?)</h\1>"#, in: text, with: "\n**$2**\n",
      options: [.caseInsensitive])
    text = replace(
      #"</?(p|div|span|sub|sup|picture|source|table|thead|tbody|tr|td|th|blockquote|hr|img|kbd|g-emoji|ul|ol|li)[^>]*>"#,
      in: text, with: "", options: [.caseInsensitive])
    // GitHub alerts: `> [!WARNING]` becomes a bold label inside the quote.
    text = replace(
      #"(?m)^(\s*>\s*)\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*$"#, in: text,
      with: "$1**$2**\\\\")
    for (upper, title) in [
      ("NOTE", "Note"), ("TIP", "Tip"), ("IMPORTANT", "Important"), ("WARNING", "Warning"),
      ("CAUTION", "Caution"),
    ] {
      text = text.replacingOccurrences(of: "**\(upper)**", with: "**\(title)**")
    }
    for (entity, character) in [
      ("&nbsp;", " "), ("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"),
      ("&amp;", "&"),
    ] {
      text = text.replacingOccurrences(of: entity, with: character)
    }
    return text.trimmingCharacters(in: .newlines)
  }

  /// The first meaningful line, for previews.
  public static func firstLine(_ body: String) -> String {
    clean(body).split(separator: "\n").map {
      $0.trimmingCharacters(in: CharacterSet(charactersIn: "#>*_` ").union(.whitespaces))
    }.first { !$0.isEmpty } ?? ""
  }

  private static func replace(
    _ pattern: String, in text: String, with template: String,
    options: NSRegularExpression.Options = []
  ) -> String {
    guard let expression = try? NSRegularExpression(pattern: pattern, options: options) else {
      return text
    }
    let range = NSRange(text.startIndex..., in: text)
    return expression.stringByReplacingMatches(
      in: text, range: range, withTemplate: template)
  }
}
