import CryptoKit
import SwiftUI

/// The account picture, downloaded once and kept in Caches so avatars show straight away and
/// offline. Pictures older than a week are fetched again in the background.
@MainActor @Observable
final class AccountPictureStore {
  static let shared = AccountPictureStore()
  private(set) var images: [URL: NativeImage] = [:]
  /// Circular, full-colour copies for the tab bar, which draws images at their own size.
  private var tabImages: [URL: NativeImage] = [:]
  private var loading: Set<URL> = []
  /// Bumped by `clear()`, so a load that started before sign-out doesn't write its result.
  private var generation = 0
  private static let maximumAge: TimeInterval = 7 * 24 * 60 * 60

  func load(_ url: URL) async {
    guard !loading.contains(url) else { return }
    let file = Self.file(for: url)
    var stale = true
    if images[url] == nil, let data = try? Data(contentsOf: file),
      let image = NativeImage(data: data)
    {
      images[url] = image
    }
    if let modified = try? file.resourceValues(forKeys: [.contentModificationDateKey])
      .contentModificationDate
    {
      stale = Date().timeIntervalSince(modified) > Self.maximumAge
    }
    guard images[url] == nil || stale else { return }
    loading.insert(url)
    defer { loading.remove(url) }
    #if DEBUG
      if url.host == CloudPreview.baseURL.host {
        images[url] = Self.previewPicture()
        tabImages[url] = nil
        return
      }
    #endif
    let started = generation
    var request = URLRequest(url: url)
    request.timeoutInterval = 20
    guard let (data, response) = try? await URLSession.shared.data(for: request),
      let status = (response as? HTTPURLResponse)?.statusCode, (200..<300).contains(status),
      let image = NativeImage(data: data), generation == started
    else { return }
    images[url] = image
    tabImages[url] = nil
    try? FileManager.default.createDirectory(
      at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? data.write(to: file, options: .atomic)
  }

  /// A small circle of the picture drawn in its own colours, for places that tint templates.
  func circularImage(for url: URL, size: CGFloat) -> NativeImage? {
    if let cached = tabImages[url] { return cached }
    guard let image = images[url] else { return nil }
    let renderer = ImageRenderer(
      content: Image(nativeImage: image).resizable().scaledToFill()
        .frame(width: size, height: size).clipShape(Circle())
    )
    renderer.scale = 3
    #if os(macOS)
      let rendered = renderer.nsImage
    #else
      let rendered = renderer.uiImage?.withRenderingMode(.alwaysOriginal)
    #endif
    tabImages[url] = rendered
    return rendered
  }

  /// Signing out forgets the picture along with the account.
  func clear() {
    generation += 1
    images = [:]
    tabImages = [:]
    try? FileManager.default.removeItem(at: Self.directory)
  }

  private static var directory: URL {
    FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
      .appendingPathComponent("AccountPictures", isDirectory: true)
  }
  private static func file(for url: URL) -> URL {
    let name = SHA256.hash(data: Data(url.absoluteString.utf8))
      .map { String(format: "%02x", $0) }.joined()
    return directory.appendingPathComponent(name)
  }

  #if DEBUG
    /// `--cloud-preview`'s account picture, drawn rather than downloaded.
    private static func previewPicture() -> NativeImage? {
      let renderer = ImageRenderer(
        content: ZStack {
          LinearGradient(
            colors: [
              Color(red: 0.98, green: 0.62, blue: 0.36), Color(red: 0.86, green: 0.3, blue: 0.5),
            ],
            startPoint: .topLeading, endPoint: .bottomTrailing)
          Text("🧑🏽‍💻").font(.system(size: 76)).offset(y: 6)
        }
        .frame(width: 120, height: 120)
      )
      renderer.scale = 2
      #if os(macOS)
        return renderer.nsImage
      #else
        return renderer.uiImage
      #endif
    }
  #endif
}

/// The account picture in a circle, falling back to initials on the brand colour, or a person
/// when signed out.
struct AccountAvatar: View {
  let account: Account
  let size: CGFloat
  private var store: AccountPictureStore { .shared }
  private var initials: String? {
    let words =
      (account.displayName ?? account.email)?
      .split(whereSeparator: { " .@".contains($0) }) ?? []
    let value = words.prefix(2).compactMap { $0.first.map { String($0).uppercased() } }.joined()
    return value.isEmpty ? nil : value
  }
  var body: some View {
    let url = account.signedIn ? account.pictureURL : nil
    ZStack {
      if let url, let image = store.images[url] {
        Image(nativeImage: image).resizable().scaledToFill()
          .frame(width: size, height: size).clipShape(Circle())
          .overlay { Circle().strokeBorder(Color.primary.opacity(0.08), lineWidth: 0.5) }
      } else if account.signedIn, let initials {
        Circle().fill(AppTheme.accent)
        Text(initials).font(.system(size: size * 0.38, weight: .semibold)).foregroundStyle(
          AppTheme.onAccent)
      } else {
        Circle().fill(Color.primary.opacity(0.09))
        Image(systemName: "person.fill").font(.system(size: size * 0.42))
          .foregroundStyle(.secondary)
      }
    }
    .frame(width: size, height: size)
    .task(id: url) { if let url { await store.load(url) } }
    .accessibilityHidden(true)
  }
}
