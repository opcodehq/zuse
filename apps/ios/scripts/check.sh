#!/bin/bash
# Lint, project consistency, unit tests and app builds. On Linux only the
# portable subset runs; Apple SDK builds and UI tests need macOS with Xcode.
set -euo pipefail
cd "$(dirname "$0")/../../.."
swift format lint --strict --recursive apps/ios/App apps/ios/Sources apps/ios/Tests apps/ios/UITests apps/ios/MacUITests apps/ios/Package.swift
ruby apps/ios/scripts/check-project.rb
if [[ "$(uname -s)" != "Darwin" ]]; then
  swiftc -frontend -parse -target arm64-apple-macosx15.0 apps/ios/App/*.swift
  swiftc -frontend -parse -target arm64-apple-ios18.0 apps/ios/App/*.swift
  bash apps/ios/scripts/test-portable.sh
  printf '%s\n' 'Apple SDK type checking, WebSocket integration and simulator UI tests need macOS.'
  exit 0
fi
python3 apps/ios/scripts/build-pairing-apple.py
bash apps/ios/scripts/test-rpc.sh
xcodebuild -project apps/ios/Zuse.xcodeproj -scheme ZuseMac \
  -destination 'platform=macOS' -derivedDataPath apps/ios/DerivedData CODE_SIGNING_ALLOWED=NO build
xcodebuild -project apps/ios/Zuse.xcodeproj -scheme Zuse \
  -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath apps/ios/DerivedData CODE_SIGNING_ALLOWED=NO build
