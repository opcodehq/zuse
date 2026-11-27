import SwiftUI
import UniformTypeIdentifiers
import ZuseCore

#if os(macOS)
  import AppKit
  import Quartz
#else
  import QuickLook
#endif

/// Thumbnails decoded once per attachment, shared by recycled transcript cells.
@MainActor private enum ThumbnailCache {
  static let images = NSCache<NSString, NativeImage>()
}

/// A file in a message: images as a thumbnail, other files as a pill. Tapping opens a preview.
struct AttachmentView: View {
  let attachment: TimelineAttachment
  let sessionID: String
  let rpc: RPCClient?
  /// A lone image gets more room than one in a row of several.
  var large = false
  @State private var preview: PreviewFile?
  @State private var loading = false
  @State private var error: String?
  @State private var thumbnail: NativeImage?
  private struct PreviewFile: Identifiable {
    let id = UUID()
    let url: URL
  }
  private var side: CGFloat { large ? 200 : 104 }
  var body: some View {
    Button {
      Task { await open() }
    } label: {
      if attachment.isImage {
        tile
      } else {
        pill
      }
    }
    .buttonStyle(.plain)
    .disabled(loading || (rpc == nil && thumbnail == nil))
    .accessibilityLabel(attachment.name)
    .accessibilityHint(error ?? "Opens a preview")
    .task(id: attachment.id) { await loadThumbnail() }
    .sheet(item: $preview, onDismiss: cleanup) { file in
      NavigationStack {
        FilePreview(url: file.url).ignoresSafeArea(edges: .bottom)
          .navigationTitle(attachment.name)
          .compactNavigationTitle()
          .toolbar {
            ToolbarItem(placement: .primaryAction) { ShareLink(item: file.url) }
            ToolbarItem(placement: .cancellationAction) { Button("Done") { preview = nil } }
          }
      }.nativeSheetSize(width: 640, height: 480)
    }
  }
  private var tile: some View {
    ZStack {
      RoundedRectangle(cornerRadius: 16, style: .continuous).fill(AppTheme.bubble)
      if let thumbnail {
        Image(nativeImage: thumbnail).resizable().scaledToFill()
      } else {
        VStack(spacing: 6) {
          if loading {
            ProgressView()
          } else {
            Image(systemName: error == nil ? "photo" : "exclamationmark.triangle")
              .font(.system(size: 22)).foregroundStyle(.secondary)
          }
          Text(attachment.name).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            .padding(.horizontal, 8)
        }
      }
    }
    .frame(width: side, height: side)
    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    .overlay(
      RoundedRectangle(cornerRadius: 16, style: .continuous)
        .strokeBorder(Color.primary.opacity(0.08), lineWidth: 1)
    )
    .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
  }
  private var pill: some View {
    HStack(spacing: 8) {
      Group {
        if loading {
          ProgressView().controlSize(.small)
        } else {
          Image(systemName: error == nil ? "doc.text" : "exclamationmark.triangle")
            .font(.system(size: 14))
        }
      }.frame(width: 18).foregroundStyle(.secondary)
      Text(attachment.name).font(.system(size: 14)).lineLimit(1).truncationMode(.middle)
    }
    .padding(.horizontal, 12).frame(height: 36)
    .background(AppTheme.bubble, in: Capsule())
    .overlay(Capsule().strokeBorder(Color.primary.opacity(0.06), lineWidth: 1))
  }
  private func loadThumbnail() async {
    guard attachment.isImage, thumbnail == nil else { return }
    if let cached = ThumbnailCache.images.object(forKey: attachment.id as NSString) {
      thumbnail = cached
      return
    }
    #if DEBUG
      if let sample = PreviewImages.image(for: attachment.id) {
        thumbnail = sample
        return
      }
    #endif
    guard let rpc else { return }
    loading = true
    defer { loading = false }
    do {
      let (data, _, _) = try await Attachments.read(attachment.id, sessionID: sessionID, rpc: rpc)
      guard let image = NativeImage(data: data) else { return }
      ThumbnailCache.images.setObject(image, forKey: attachment.id as NSString)
      thumbnail = image
    } catch {
      self.error = error.localizedDescription
    }
  }
  @State private var temporaryDirectory: URL?
  private func open() async {
    loading = true
    error = nil
    defer { loading = false }
    do {
      var data: Data
      var mime = attachment.mimeType
      if let rpc {
        (data, mime, _) = try await Attachments.read(attachment.id, sessionID: sessionID, rpc: rpc)
      } else if let thumbnail, let png = thumbnail.pngData() {
        data = png
        mime = "image/png"
      } else {
        return
      }
      let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
        UUID().uuidString)
      try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      let file = directory.appendingPathComponent(
        (attachment.name as NSString).deletingPathExtension.isEmpty
          ? "Attachment" : (attachment.name as NSString).deletingPathExtension
      ).appendingPathExtension(UTType(mimeType: mime)?.preferredFilenameExtension ?? "bin")
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

#if os(macOS)
  extension NSImage {
    fileprivate func pngData() -> Data? {
      guard let tiff = tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff) else {
        return nil
      }
      return bitmap.representation(using: .png, properties: [:])
    }
  }
#endif

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
