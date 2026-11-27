import SwiftUI

/// A system symbol, or one of Zuse's own icons: asset names starting with "zuse." are vector
/// templates drawn to sit beside SF Symbols (the Zapper mascot and a pull request).
struct AppIcon: View {
  static let zapper = "zuse.zapper"
  static let pullRequest = "zuse.pull-request"

  let name: String
  var size: CGFloat = 17
  var weight: Font.Weight = .regular
  var body: some View {
    if name.hasPrefix("zuse.") {
      // Template images ignore the font, so size them like a symbol of the same point size.
      Image(name).renderingMode(.template).resizable().scaledToFit()
        .frame(width: size * 1.2, height: size * 1.2)
    } else {
      Image(systemName: name).font(.system(size: size, weight: weight))
    }
  }
}
