#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build
rm -f .build/simulator-rpc-port
ZUSE_FIXTURE_PORT_FILE="$PWD/.build/simulator-rpc-port" ZUSE_FIXTURE_DIAGNOSTICS=1 bun scripts/rpc-fixture.ts > .build/simulator-fixture.log 2>&1 &
fixture_pid=$!
trap 'kill "$fixture_pid" 2>/dev/null || true' EXIT
for _ in {1..100}; do
  [[ -s .build/simulator-rpc-port ]] && break
  sleep 0.1
done
[[ -s .build/simulator-rpc-port ]] || { cat .build/simulator-fixture.log; exit 1; }
export TEST_RUNNER_ZUSE_IOS_FIXTURE_URL="ws://127.0.0.1:$(cat .build/simulator-rpc-port)"
destination="${1:-platform=iOS Simulator,name=iPhone 17 Pro}"
run_tests() {
  xcodebuild -project Zuse.xcodeproj -scheme Zuse \
    -destination "$destination" -derivedDataPath "${ZUSE_APPLE_DERIVED_DATA:-DerivedData}" \
    "$@" CODE_SIGNING_ALLOWED=YES test
}
if [[ -n "${2:-}" ]]; then run_tests -resultBundlePath "$2"; else run_tests; fi
