import Foundation

/// Zapper pixel mascots, ported from `crates/protocol/src/presentation/zapper_mascots.rs` so
/// every client draws the same face for the same saved avatar. Only still poses are drawn here.
public enum ZapperMascots {
  public struct Eye: Sendable, Hashable {
    public let x, y, w, h: Double
  }
  public struct Mascot: Sendable, Hashable, Identifiable {
    public let id: String
    public let name: String
    let a, b, n, taper, dy: Double
    let eyes: [Eye]
  }
  public struct Color: Sendable, Hashable, Identifiable {
    public let id: String
    /// 0xRRGGBB
    public let body: UInt32
  }
  public enum Cell: Sendable, Hashable { case body, eye }
  /// Eye colour on every body colour.
  public static let eye: UInt32 = 0x18181c

  private static func e(_ x: Double, _ y: Double, _ w: Double, _ h: Double) -> Eye {
    Eye(x: x, y: y, w: w, h: h)
  }
  public static let mascots: [Mascot] = [
    Mascot(
      id: "bolt", name: "Pebble", a: 0.70, b: 0.64, n: 2.4, taper: 0, dy: 0.06,
      eyes: [e(-0.21, -0.06, 0.18, 0.38), e(0.21, -0.06, 0.18, 0.38)]),
    Mascot(
      id: "ace", name: "Comet", a: 0.64, b: 0.72, n: 2.2, taper: 0.38, dy: 0.04,
      eyes: [e(-0.18, 0.06, 0.17, 0.34), e(0.18, 0.06, 0.17, 0.34)]),
    Mascot(
      id: "brawler", name: "Brick", a: 0.76, b: 0.62, n: 3.6, taper: 0, dy: 0.08,
      eyes: [e(-0.22, -0.04, 0.2, 0.3), e(0.22, -0.04, 0.2, 0.3)]),
    Mascot(
      id: "hex", name: "Orbit", a: 0.68, b: 0.68, n: 2, taper: 0, dy: 0.06,
      eyes: [e(-0.15, -0.04, 0.2, 0.24), e(0.15, -0.04, 0.2, 0.24)]),
    Mascot(
      id: "squire", name: "Bean", a: 0.56, b: 0.80, n: 2.5, taper: 0, dy: 0.04,
      eyes: [e(-0.17, -0.16, 0.2, 0.3), e(0.17, -0.16, 0.2, 0.3)]),
    Mascot(
      id: "shade", name: "Slab", a: 0.82, b: 0.54, n: 3.2, taper: 0, dy: 0.10,
      eyes: [e(-0.25, -0.02, 0.26, 0.22), e(0.25, -0.02, 0.26, 0.22)]),
    Mascot(
      id: "rogue", name: "Wink", a: 0.70, b: 0.66, n: 2.6, taper: 0, dy: 0.06,
      eyes: [e(-0.21, -0.06, 0.18, 0.38), e(0.22, 0, 0.22, 0.22)]),
    Mascot(
      id: "viking", name: "Doze", a: 0.80, b: 0.64, n: 2.8, taper: 0, dy: 0.08,
      eyes: [e(-0.23, 0.04, 0.28, 0.12), e(0.23, 0.04, 0.28, 0.12)]),
    Mascot(
      id: "bones", name: "Rosy", a: 0.66, b: 0.70, n: 2.2, taper: 0, dy: 0.06,
      eyes: [e(-0.2, -0.08, 0.2, 0.3), e(0.2, -0.08, 0.2, 0.3)]),
  ]
  public static let colors: [Color] = [
    Color(id: "coral", body: 0xef6b5f), Color(id: "yellow", body: 0xf6c94a),
    Color(id: "green", body: 0x7fd05a), Color(id: "lime", body: 0xc3e45a),
    Color(id: "violet", body: 0xa98bf5), Color(id: "teal", body: 0x5fd0c0),
    Color(id: "blue", body: 0x6ea8ff), Color(id: "pink", body: 0xff8fbf),
  ]

  /// FNV-1a, matching the Rust `hash`, so unsaved avatars look the same everywhere.
  static func hash(_ text: String) -> UInt32 {
    text.utf8.reduce(0x811c_9dc5) { ($0 ^ UInt32($1)) &* 0x0100_0193 }
  }
  public static func defaultAvatar(seed: String) -> ZapperAvatar {
    let value = hash(seed)
    return ZapperAvatar(
      mascot: mascots[Int(value % UInt32(mascots.count))].id,
      color: colors[Int((value >> 8) % UInt32(colors.count))].id)
  }
  /// Unknown ids fall back to the first of each, so newer avatars still draw.
  public static func resolve(_ avatar: ZapperAvatar) -> (Mascot, Color) {
    (
      mascots.first { $0.id == avatar.mascot } ?? mascots[0],
      colors.first { $0.id == avatar.color } ?? colors[0]
    )
  }
  /// Cells across for an avatar `size` points wide.
  public static func grid(for size: Double) -> Int {
    (min(max(Int((size / 1.5).rounded()), 16), 44) / 2) * 2
  }

  private static let zoom = 1.16

  /// `mascot` facing forward on a `grid`×`grid` raster, row by row. `blink` 1 closes the
  /// eyes (resting); `wide` grows them (working or asking).
  public static func raster(_ mascot: Mascot, grid: Int, blink: Double = 0, wide: Double = 0)
    -> [Cell?]
  {
    let g = Double(grid)
    let depth = 0.9 * min(mascot.a, mascot.b)
    let centreY = mascot.dy
    func inside(_ qx: Double, _ qy: Double) -> Bool {
      let along = min(max(qy / mascot.b, -1), 1)
      let half = mascot.a * (1 - mascot.taper * (0.5 - 0.5 * along))
      return pow(abs(qx / half), mascot.n) + pow(abs(qy / mascot.b), mascot.n) <= 1
    }
    var out = [Cell?](repeating: nil, count: grid * grid)
    for y in 0..<grid {
      for x in 0..<grid {
        let px = ((Double(x) + 0.5) / g * 2 - 1) / zoom
        let py = ((Double(y) + 0.5) / g * 2 - 1) / zoom - centreY
        if inside(px, py) { out[y * grid + x] = .body }
      }
    }
    let bodies = out.map { $0 != nil }
    func solid(_ x: Int, _ y: Int) -> Bool {
      (0..<grid).contains(x) && (0..<grid).contains(y) && bodies[y * grid + x]
    }
    let minSide = 2 / g * 1.05 / zoom
    for eye in mascot.eyes {
      let (u, v) = (eye.x / mascot.a, eye.y / mascot.b)
      let z0 = depth * max(1 - u * u - v * v, 0.05).squareRoot()
      if z0 < 0.12 * depth { continue }
      let grow = 1 + wide
      let shut = blink > 0.8
      let slit = eye.h * zoom * g / 2 < 2.5
      let w = max(eye.w * grow * (shut && slit ? 0.7 : 1), minSide)
      let h = max(eye.h * grow * (1 - blink), minSide)
      let cx = (eye.x * zoom + 1) / 2 * g
      var wc = max(Int((w * zoom * g / 2).rounded()), 1)
      let parity = ((Int((cx * 2).rounded()) % 2) + 2) % 2
      if wc % 2 != parity {
        wc += (w * zoom * g / 2) > Double(wc) ? 1 : -1
        wc = max(wc, 1)
      }
      let hc = max(Int((h * zoom * g / 2).rounded()), 1)
      let cy = ((eye.y + centreY) * zoom + 1) / 2 * g
      let x0 = Int((cx - Double(wc) / 2 + 0.5).rounded(.down))
      let y0 = Int((cy - Double(hc) / 2 + 0.5).rounded(.down))
      let round = wc >= 3 && hc >= 3 && wc + hc >= 7
      for yy in y0..<(y0 + hc) {
        for xx in x0..<(x0 + wc) {
          let corner = (xx == x0 || xx == x0 + wc - 1) && (yy == y0 || yy == y0 + hc - 1)
          if (corner && round) || xx < 0 || yy < 0 || xx >= grid || yy >= grid { continue }
          // Keep a ring of body around every eye cell so the silhouette stays whole.
          if [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)].allSatisfy({ solid(xx + $0.0, yy + $0.1) })
          {
            out[yy * grid + xx] = .eye
          }
        }
      }
    }
    return out
  }
}
