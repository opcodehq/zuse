#!/bin/bash
set -euo pipefail
if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Apple validation is pending: this command requires macOS and full Xcode.' >&2
  exit 2
fi
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/../.."
for dependency in xcodebuild xcrun swift ruby bun; do
  command -v "$dependency" >/dev/null || { printf 'Missing prerequisite: %s\n' "$dependency" >&2; exit 1; }
done
ruby -e 'require "xcodeproj"' || { printf '%s\n' 'Install the xcodeproj Ruby gem before validation.' >&2; exit 1; }
validation_dir="${ZUSE_APPLE_VALIDATION_DIR:-$PWD/.context/native-apple-validation/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
mkdir -p "$validation_dir"
validation_dir="$(cd "$validation_dir" && pwd)"
run_check() {
  local name="$1"
  shift
  printf 'Running %s…\n' "$name"
  if "$@" > "$validation_dir/$name.log" 2>&1; then
    printf 'Passed %s\n' "$name"
  else
    printf 'Failed %s. See %s/%s.log\n' "$name" "$validation_dir" "$name" >&2
    exit 1
  fi
}
find_destination() {
  xcrun simctl list devices available --json | ZUSE_SIMULATOR_FAMILY="$1" bun -e '
    const catalog = JSON.parse(await Bun.stdin.text());
    const runtimes = Object.entries(catalog.devices).sort(([a], [b]) => b.localeCompare(a, undefined, { numeric: true }));
    for (const [runtime, devices] of runtimes) {
      const major = Number(runtime.match(/iOS-(\d+)/)?.[1] ?? 0);
      if (major < 18) continue;
      const device = devices.find(device => device.isAvailable && device.name.startsWith(process.env.ZUSE_SIMULATOR_FAMILY));
      if (device) { console.log(`platform=iOS Simulator,id=${device.udid}`); process.exit(0); }
    }
    console.error(`Install an available iOS 18+ ${process.env.ZUSE_SIMULATOR_FAMILY} simulator in Xcode, or set the destination override.`);
    process.exit(1);
  '
}
iphone_destination="${ZUSE_IPHONE_DESTINATION:-$(find_destination iPhone)}"
ipad_destination="${ZUSE_IPAD_DESTINATION:-$(find_destination iPad)}"
{
  xcodebuild -version
  swift --version
  git rev-parse HEAD
  git diff HEAD --name-only
  printf 'iPhone destination: %s\niPad destination: %s\n' "$iphone_destination" "$ipad_destination"
} > "$validation_dir/environment.log"
run_check builds-and-core bash apps/ios/scripts/check.sh
run_check mac-ui xcodebuild -project apps/ios/Zuse.xcodeproj -scheme ZuseMac \
  -destination 'platform=macOS' -derivedDataPath apps/ios/DerivedData \
  -resultBundlePath "$validation_dir/macOS.xcresult" CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- test
run_check iphone-ui bash apps/ios/scripts/test-simulator.sh "$iphone_destination" "$validation_dir/iPhone.xcresult"
run_check ipad-ui bash apps/ios/scripts/test-simulator.sh "$ipad_destination" "$validation_dir/iPad.xcresult"
printf 'Automated Apple checks passed. Results: %s\n' "$validation_dir"
printf '%s\n' 'Live account/device, visual comparison, accessibility, and rendering performance checks remain required; see internal-docs/mobile/native-apple-validation.md.'
