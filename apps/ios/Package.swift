// swift-tools-version: 6.0
import PackageDescription

var coreDependencies: [Target.Dependency] = []
var nativeTargets: [Target] = []
#if os(macOS)
  // Build with scripts/build-pairing-apple.py before resolving the package in Xcode.
  coreDependencies.append("ZusePairing")
  nativeTargets.append(.binaryTarget(name: "ZusePairing", path: "Native/ZusePairing.xcframework"))
#endif

let package = Package(
  name: "ZuseNative",
  platforms: [.iOS(.v18), .macOS(.v15)],
  products: [.library(name: "ZuseCore", targets: ["ZuseCore"])],
  targets: [
    .target(name: "ZuseCore", dependencies: coreDependencies),
    .testTarget(name: "ZuseCoreTests", dependencies: ["ZuseCore"]),
  ] + nativeTargets
)
