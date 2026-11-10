#!/bin/bash
# Protocol-independent correctness checks on Linux or macOS; Apple UI/crypto tests are separate.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
portable_dir="$root/.build/portable"
mkdir -p "$portable_dir/Sources/ZuseCore" "$portable_dir/Tests/ZuseCoreTests"
# Remove only our generated source links so removed tests cannot linger in a reused build directory.
for source in "$portable_dir"/Sources/ZuseCore/*.swift "$portable_dir"/Tests/ZuseCoreTests/*.swift; do
  [[ ! -L "$source" ]] || rm "$source"
done
for source in CloudGateway AccountTransport PairedTransport NativePairing CanonicalTranscript CanonicalWorkspace JSONValue ModelCatalog Transcript Diff MarkdownBlocks PairingLink ConnectionProfile ChatCreation SettingsSchema WorkspaceCatalog SessionArchive InteractionResponse WorkspaceLocation CloudLifecycle CloudClient RemoteAccess PairingHTTPDelegate RPCReply RPCClient Attachments AccountAuthorization SessionActivity ServeCommand ComputerAccess ServeConfiguration ToolPresentation TranscriptPresentation; do
  ln -sf "$root/Sources/ZuseCore/$source.swift" "$portable_dir/Sources/ZuseCore/$source.swift"
done
for source in CloudGatewayTests AccountTransportTests TranscriptTests MarkdownBlocksTests ConnectionProfileTests ChatCreationTests SettingsSchemaTests RPCReplyTests RPCTransportTests WorkspaceCatalogTests ArchiveTests ConnectionTests CloudClientTests WorkspaceLocationTests AccountAuthorizationTests SessionActivityTests ServeCommandTests ComputerAccessTests ServeConfigurationTests ToolPresentationTests; do
  ln -sf "$root/Tests/ZuseCoreTests/$source.swift" "$portable_dir/Tests/ZuseCoreTests/$source.swift"
done
cat > "$portable_dir/Package.swift" <<'MANIFEST'
// swift-tools-version: 6.0
import PackageDescription
#if os(Linux)
let packages: [Package.Dependency] = [.package(url: "https://github.com/apple/swift-crypto.git", exact: "4.0.0")]
let crypto: [Target.Dependency] = [.product(name: "Crypto", package: "swift-crypto")]
#else
let packages: [Package.Dependency] = []
let crypto: [Target.Dependency] = []
#endif
let package = Package(name: "ZusePortableChecks", platforms: [.macOS(.v15)], dependencies: packages, targets: [
  .target(name: "ZuseCore", dependencies: crypto),
  .testTarget(name: "ZuseCoreTests", dependencies: ["ZuseCore"])
])
MANIFEST
swift test --jobs "${ZUSE_SWIFT_JOBS:-4}" --package-path "$portable_dir"
