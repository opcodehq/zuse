import SwiftUI
import Textual
import WebKit
import ZuseCore

struct ChatMarkdown: View {
  let text: String
  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      ForEach(MarkdownBlock.parse(text)) { block in
        if block.kind == .diagram {
          DiagramView(source: block.content)
        } else {
          StructuredText(markdown: block.content, syntaxExtensions: [.math])
            .font(.subheadline)
            .textual.blockSpacing(.fontScaled(top: 0.45, bottom: 0.45))
            .textual.listItemSpacing(.fontScaled(top: 0.15, bottom: 0.15))
            .textual.textSelection(.enabled)
        }
      }
    }
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
