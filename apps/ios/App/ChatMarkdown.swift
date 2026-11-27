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
            .textual.lineSpacing(.fontScaled(size == .reply ? 0.3 : 0.2))
            .textual.blockSpacing(.fontScaled(top: 0.5, bottom: 0.5))
            .textual.listItemSpacing(.fontScaled(top: 0.2, bottom: 0.2))
            .textual.inlineStyle(
              InlineStyle()
                .code(
                  .monospaced, .fontScale(0.88),
                  .backgroundColor(
                    DynamicColor(light: .black.opacity(0.06), dark: .white.opacity(0.1)))
                )
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

/// Headings sized for a chat column rather than a document page.
private struct ChatHeadingStyle: StructuredText.HeadingStyle {
  func makeBody(configuration: Configuration) -> some View {
    let scales: [CGFloat] = [1.3, 1.18, 1.08, 1, 1, 1]
    configuration.label
      .textual.fontScale(scales[min(configuration.headingLevel, 6) - 1])
      .textual.blockSpacing(.fontScaled(top: 1.1, bottom: 0.45))
      .fontWeight(.semibold)
  }
}

/// A rounded code card with its language and a copy button.
private struct ChatCodeBlockStyle: StructuredText.CodeBlockStyle {
  func makeBody(configuration: Configuration) -> some View {
    VStack(alignment: .leading, spacing: 0) {
      HStack(spacing: 6) {
        Text(configuration.languageHint?.lowercased() ?? "code")
          .font(.system(size: 12, weight: .medium)).foregroundStyle(.secondary)
        Spacer(minLength: 8)
        CopyButton(label: "Copy code") { configuration.codeBlock.copyToPasteboard() }
      }
      .padding(.leading, 12).padding(.trailing, 4).frame(height: 32)
      Divider().opacity(0.6)
      Overflow {
        configuration.label
          .textual.lineSpacing(.fontScaled(0.3))
          .textual.fontScale(0.84)
          .fixedSize(horizontal: false, vertical: true)
          .monospaced()
          .padding(.horizontal, 12).padding(.vertical, 10)
      }
    }
    .background(AppTheme.codeBackground, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: 12, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.07), lineWidth: 1)
    )
    .textual.blockSpacing(.fontScaled(top: 0.6, bottom: 0.6))
  }
}

/// Tables read as a quiet card: hairline rules and a bold header row.
private struct ChatTableStyle: StructuredText.TableStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .textual.tableCellSpacing(horizontal: 1, vertical: 1)
      .textual.blockSpacing(.fontScaled(top: 0.6, bottom: 0.6))
      .textual.tableBackground { layout in
        Canvas { context, _ in
          if layout.numberOfRows > 0 {
            let header = layout.rowBounds(0)
            context.fill(Path(header.integral), with: .color(.primary.opacity(0.045)))
          }
        }
      }
      .textual.tableOverlay { layout in
        Canvas { context, _ in
          for divider in layout.dividers() {
            context.fill(Path(divider), with: .color(.primary.opacity(0.1)))
          }
        }
      }
      .padding(1)
      .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
      .overlay(
        RoundedRectangle(cornerRadius: 10, style: .continuous)
          .strokeBorder(Color.primary.opacity(0.1), lineWidth: 1)
      )
  }
}

private struct ChatTableCellStyle: StructuredText.TableCellStyle {
  func makeBody(configuration: Configuration) -> some View {
    configuration.label
      .fontWeight(configuration.row == 0 ? .semibold : .regular)
      .textual.fontScale(0.94)
      .padding(.vertical, 7).padding(.horizontal, 11)
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
        .font(.system(size: 13, weight: .medium))
        .foregroundStyle(.secondary)
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
