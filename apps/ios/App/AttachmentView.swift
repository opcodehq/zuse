import SwiftUI
import UniformTypeIdentifiers
import ZuseCore

#if os(macOS)
  import AppKit
  import Quartz
#else
  import QuickLook
#endif

struct AttachmentView: View {
  let reference: JSONValue
  let sessionID: String
  let rpc: RPCClient?
  @State private var preview: PreviewFile?
  @State private var loading = false
  @State private var error: String?
  private struct PreviewFile: Identifiable {
    let id = UUID()
    let url: URL
  }
  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Button {
        Task { await open() }
      } label: {
        HStack {
          if loading {
            ProgressView()
          } else {
            Image(
              systemName: reference["mimeType"].string?.hasPrefix("image/") == true
                ? "photo" : "doc")
          }
          Text(reference["originalName"].string ?? "Attachment").lineLimit(2)
          Spacer()
          Image(systemName: "arrow.up.right")
        }.padding(10).background(.quaternary, in: RoundedRectangle(cornerRadius: 10))
      }.buttonStyle(.plain).disabled(loading || rpc == nil)
      if let error { Text(error).font(.caption).foregroundStyle(.red) }
    }
    .sheet(item: $preview, onDismiss: cleanup) { file in
      NavigationStack {
        FilePreview(url: file.url).ignoresSafeArea(edges: .bottom)
          .navigationTitle(reference["originalName"].string ?? "Attachment")
          .compactNavigationTitle()
          .toolbar {
            ToolbarItem(placement: .primaryAction) { ShareLink(item: file.url) }
            ToolbarItem(placement: .cancellationAction) { Button("Done") { preview = nil } }
          }
      }.nativeSheetSize(width: 640, height: 480)
    }
  }
  @State private var temporaryDirectory: URL?
  private func open() async {
    guard let rpc, let id = reference["id"].string else { return }
    loading = true
    error = nil
    defer { loading = false }
    do {
      let (data, mime, _) = try await Attachments.read(id, sessionID: sessionID, rpc: rpc)
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
        UUID().uuidString)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      let file = directory.appendingPathComponent("Attachment").appendingPathExtension(
        UTType(mimeType: mime)?.preferredFilenameExtension ?? "bin")
      try data.write(to: file, options: [.atomic, .completeFileProtection])
      cleanup()
      temporaryDirectory = directory
      preview = PreviewFile(url: file)
    } catch { self.error = error.localizedDescription }
  }
  private func cleanup() {
    if let temporaryDirectory { try? FileManager.default.removeItem(at: temporaryDirectory) }
    temporaryDirectory = nil
  }
}

#if os(iOS)
  private struct FilePreview: UIViewControllerRepresentable {
    let url: URL
    func makeCoordinator() -> Coordinator { Coordinator(url) }
    func makeUIViewController(context: Context) -> QLPreviewController {
      let controller = QLPreviewController()
      controller.dataSource = context.coordinator
      return controller
    }
    func updateUIViewController(_ controller: QLPreviewController, context: Context) {}
    final class Coordinator: NSObject, QLPreviewControllerDataSource {
      let url: URL
      init(_ url: URL) { self.url = url }
      func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
      func previewController(_ controller: QLPreviewController, previewItemAt index: Int)
        -> QLPreviewItem
      { url as NSURL }
    }
  }

#else
  private struct FilePreview: NSViewRepresentable {
    let url: URL
    func makeNSView(context: Context) -> NSView {
      guard let view = QLPreviewView(frame: .zero, style: .normal) else {
        return NSTextField(
          wrappingLabelWithString: "Preview is unavailable. You can still share this file.")
      }
      view.autostarts = true
      view.previewItem = url as NSURL
      return view
    }
    func updateNSView(_ view: NSView, context: Context) {
      (view as? QLPreviewView)?.previewItem = url as NSURL
    }
    static func dismantleNSView(_ view: NSView, coordinator: ()) {
      (view as? QLPreviewView)?.close()
    }
  }
#endif
