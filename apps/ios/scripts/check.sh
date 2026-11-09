#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/../.."
if [[ "$(uname -s)" == "Darwin" ]]; then
  python3 apps/ios/scripts/build-pairing-apple.py
fi
swift format lint --strict --recursive apps/ios/App apps/ios/Sources apps/ios/Tests apps/ios/UITests apps/ios/MacUITests apps/ios/Package.swift
ruby apps/ios/scripts/check-project.rb
bun apps/ios/scripts/settings-schema.ts --check
bun run --cwd apps/ios check-types
bunx biome check apps/ios/scripts packages/utils/src/generative-ui.ts packages/utils/test/unit/generative-ui.test.ts
if [[ "$(uname -s)" == "Darwin" ]]; then
  bash apps/ios/scripts/test-rpc.sh
  xcodebuild -project apps/ios/Zuse.xcodeproj -scheme ZuseMac \
    -destination 'platform=macOS' -derivedDataPath apps/ios/DerivedData CODE_SIGNING_ALLOWED=NO build
  xcodebuild -project apps/ios/Zuse.xcodeproj -scheme Zuse \
    -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath apps/ios/DerivedData CODE_SIGNING_ALLOWED=NO build
else
  swiftc -frontend -parse -target arm64-apple-macosx15.0 apps/ios/App/*.swift
  swiftc -frontend -parse -target arm64-apple-ios18.0 apps/ios/App/*.swift
  bash apps/ios/scripts/test-portable.sh
  printf '%s\n' 'Apple SDK type checking, WebSocket integration and simulator UI tests remain pending on macOS.'
fi
