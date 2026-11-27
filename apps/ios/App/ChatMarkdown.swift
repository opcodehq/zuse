import SwiftUI
import Textual
import WebKit
import ZuseCore

struct ChatMarkdown: View {
  /// `reply` is an agent's answer; `compact` is secondary prose such as reasoning or narration.
  enum Size {
    case standard, reply, compact
    var font: Font {
      switch self {
      case .standard: .subheadline
      case .reply: .system(size: 16)
      case .compact: .system(size: 14)
      }
    }
  }
  let text: String
  var size: Size = .standard
  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      ForEach(MarkdownBlock.parse(text)) { block in
        if block.kind == .diagram {
          DiagramView(source: block.content)
        } else {
          StructuredText(markdown: block.content, syntaxExtensions: [.math])
            .font(size.font)
            // About 16 on 24 for replies: room to breathe without looking double-spaced.
            .textual.lineSpacing(.fontScaled(size == .reply ? 0.36 : 0.26))
            .textual.blockSpacing(.fontScaled(top: 0.55, bottom: 0.55))
            .textual.listItemSpacing(.fontScaled(top: 0.18, bottom: 0.18))
            .textual.unorderedListMarker(ChatListMarker())
            .textual.inlineStyle(
              InlineStyle()
                .code(
                  .monospaced, .fontScale(0.88),
                  .backgroundColor(
                    DynamicColor(light: .black.opacity(0.045), dark: .white.opacity(0.08)))
                )
                .strong(.fontWeight(.semibold))
                .link(.foregroundColor(AppTheme.link))
            )
            .textual.headingStyle(ChatHeadingStyle())
            .textual.codeBlockStyle(ChatCodeBlockStyle())
            .textual.tableStyle(ChatTableStyle())
            .textual.tableCellStyle(ChatTableCellStyle())
            .textual.blockQuoteStyle(ChatBlockQuoteStyle())
            .textual.textSelection(.enabled)
        }
      }
    }
  }
}

/// A small, muted dot: lists read as rhythm, not as a column of heavy bullets.
private struct ChatListMarker: StructuredText.UnorderedListMarker {
  func makeBody(configuration: Configuration) -> some View {
    Circle()
      .fill(Color.secondary.opacity(configuration.indentationLevel > 1 ? 0.45 : 0.75))
      .frame(width: 5, height: 5)
      .textual.frame(minWidth: .fontScaled(1.1), alignment: .center)
  }
}

/// Headings sized for a chat column rather than a document page, set in a calm medium weight.
private struct ChatHeadingStyle: StructuredText.HeadingStyle {
  func makeBody(configuration: Configuration) -> some View {
    let scales: [CGFloat] = [1.25, 1.15, 1.06, 1, 1, 0.94]
    configuration.label
      .textual.fontScale(scales[min(configuration.headingLevel, 6) - 1])
      .textual.blockSpacing(.fontScaled(top: 1.0, bottom: 0.3))
      .fontWeight(.medium)
  }
}

/// A quiet code card: a tinted surface, the language and a copy button, nothing else.
private struct ChatCodeBlockStyle: StructuredText.CodeBlockStyle {
  func makeBody(configuration: Configuration) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 6) {
        Text(configuration.languageHint?.lowercased() ?? "code")
          .font(.system(size: 11.5, weight: .medium, design: .monospaced))
          .foregroundStyle(.tertiary)
        Spacer(minLength: 8)
        CopyButton(label: "Copy code") { configuration.codeBlock.copyToPasteboard() }
      }
      .padding(.leading, 12).padding(.trailing, 2).frame(height: 30)
      Overflow {
        configuration.label
          .textual.lineSpacing(.fontScaled(0.32))
          .textual.fontScale(0.82)
          .fixedSize(horizontal: false, vertical: true)
          .monospaced()
          .padding(.horizontal, 12).padding(.bottom, 12)
      }
    }
    .background(AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    .textual.blockSpacing(.fontScaled(top: 0.7, bottom: 0.7))
  }
}

/// Tables stay light: a ruled header and hairlines between rows, no box or column lines.
private struct ChatTableStyle: StructuredText.TableStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .textual.tableCellSpacing(horizontal: 0, vertical: 1)
      .textual.blockSpacing(.fontScaled(top: 0.7, bottom: 0.7))
      .textual.tableOverlay { layout in
        Canvas { context, _ in
          for (index, divider) in layout.horizontalDividers().enumerated() {
            context.fill(Path(divider), with: .color(.primary.opacity(index == 0 ? 0.16 : 0.08)))
          }
        }
      }
  }
}

private struct ChatTableCellStyle: StructuredText.TableCellStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .fontWeight(configuration.row == 0 ? .semibold : .regular)
      .textual.fontScale(0.92)
      .padding(.vertical, 7)
      .padding(.leading, configuration.column == 0 ? 0 : 10).padding(.trailing, 14)
  }
}

private struct ChatBlockQuoteStyle: StructuredText.BlockQuoteStyle {
  func makeBody(configuration: Configuration) -> some View {
    HStack(spacing: 10) {
      RoundedRectangle(cornerRadius: 1.5).fill(Color.primary.opacity(0.18)).frame(width: 3)
      configuration.label.foregroundStyle(.secondary)
    }
    .fixedSize(horizontal: false, vertical: true)
  }
}

/// A small icon button that confirms with a checkmark.
struct CopyButton: View {
  var label = "Copy"
  var size: CGFloat = 13
  var tone: HierarchicalShapeStyle = .secondary
  let copy: () -> Void
  @State private var copied = false
  var body: some View {
    Button {
      copy()
      copied = true
      Task {
        try? await Task.sleep(for: .seconds(1.5))
        copied = false
      }
    } label: {
      Image(systemName: copied ? "checkmark" : "square.on.square")
        .font(.system(size: size, weight: .medium))
        .foregroundStyle(tone)
        .frame(width: 30, height: 30).contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(copied ? "Copied" : label)
  }
}

private struct DiagramView: View {
  let source: String
  @State private var fullScreen = false
  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      DiagramWebView(source: source).frame(height: 280)
      HStack {
        Button("Expand diagram", systemImage: "arrow.up.left.and.arrow.down.right") {
          fullScreen = true
        }
        Spacer()
        Button("Copy source", systemImage: "doc.on.doc") { NativePlatform.copy(source) }
      }.font(.caption)
    }.sheet(isPresented: $fullScreen) {
      NavigationStack {
        DiagramWebView(source: source).navigationTitle("Diagram").compactNavigationTitle()
          .toolbar {
            ToolbarItem(placement: .confirmationAction) { Button("Done") { fullScreen = false } }
          }
      }.nativeSheetSize(width: 720, height: 540)
    }
  }
}

#if os(macOS)
  private typealias DiagramRepresentable = NSViewRepresentable
#else
  private typealias DiagramRepresentable = UIViewRepresentable
#endif

private struct DiagramWebView: DiagramRepresentable {
  let source: String
  func makeCoordinator() -> Coordinator { Coordinator(source) }
  #if os(macOS)
    func makeNSView(context: Context) -> WKWebView { makeWebView(context: context) }
    func updateNSView(_ view: WKWebView, context: Context) { updateWebView(view, context: context) }
    static func dismantleNSView(_ view: WKWebView, coordinator: Coordinator) { dismantle(view) }
  #else
    func makeUIView(context: Context) -> WKWebView { makeWebView(context: context) }
    func updateUIView(_ view: WKWebView, context: Context) { updateWebView(view, context: context) }
    static func dismantleUIView(_ view: WKWebView, coordinator: Coordinator) { dismantle(view) }
  #endif
  private func makeWebView(context: Context) -> WKWebView {
    let configuration = WKWebViewConfiguration()
    configuration.websiteDataStore = .nonPersistent()
    let view = WKWebView(frame: .zero, configuration: configuration)
    view.navigationDelegate = context.coordinator
    #if os(iOS)
      view.isOpaque = false
      view.backgroundColor = .clear
      view.scrollView.backgroundColor = .clear
    #else
      view.setValue(false, forKey: "drawsBackground")
    #endif
    if let file = Bundle.main.url(forResource: "Diagram", withExtension: "html") {
      view.loadFileURL(file, allowingReadAccessTo: file.deletingLastPathComponent())
    }
    return view
  }
  private func updateWebView(_ view: WKWebView, context: Context) {
    if context.coordinator.source != source {
      context.coordinator.source = source
      context.coordinator.render(view)
    }
  }
  private static func dismantle(_ view: WKWebView) {
    view.stopLoading()
    view.navigationDelegate = nil
  }
  final class Coordinator: NSObject, WKNavigationDelegate {
    var source: String
    init(_ source: String) { self.source = source }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { render(webView) }
    func webView(
      _ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
      decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
    ) {
      decisionHandler(
        navigationAction.navigationType == .other && navigationAction.request.url?.isFileURL == true
          ? .allow : .cancel)
    }
    func render(_ view: WKWebView) {
      let value = source
      Task { @MainActor in
        _ = try? await view.callAsyncJavaScript(
          "try { await renderDiagram(source) } catch { document.body.textContent = source }",
          arguments: ["source": value], in: nil, contentWorld: .page)
      }
    }
  }
}
