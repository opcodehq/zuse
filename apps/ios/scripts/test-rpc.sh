#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p .build
rm -f .build/rpc-port
bun scripts/rpc-fixture.ts &
fixture_pid=$!
trap 'kill "$fixture_pid" 2>/dev/null || true' EXIT
for attempt in {1..50}; do
  [[ -s .build/rpc-port ]] && break
  sleep 0.1
done
ZUSE_RPC_TEST_URL="ws://127.0.0.1:$(cat .build/rpc-port)" swift test
