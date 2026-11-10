# Mobile import

Imported from https://github.com/swarajbachu/zuse:

- `apps/ios` and the React Native baseline `apps/mobile`: default branch commit `c02d624db23b2317c14b7aff90526828b040eb67`.
- `apps/android`: branch `feat/native-android-kotlin`, commit `b7daf3ec22db84d3a3a911453b9c14f1c76c7eb6`.

These are source imports, not verified feature parity. Terminal is excluded from the requested parity work. Existing upstream terminal code has been preserved as part of the baseline import.

The upstream repository is a TypeScript monorepo. This checkout uses a Rust backend and does not contain the upstream `packages` workspace. The React Native workspace dependencies and native asset-generation scripts therefore need integration before their original workspace commands can run here. Committed native assets are included.

## Current implementation

Both native clients now speak the Rust canonical RPC contract alongside the imported legacy transport. Canonical integration includes project/chat discovery, models, chat creation and recovery, transcript resets/deltas, live runtime status, commands and stop, permission/question responses, attachment upload/download, and scoped Git diffs. Android reuses unchanged rendered messages and folds transcript changes off the UI thread. Reconnect retries use bounded jitter and avoid replaying uncertain mutations. These changes have contract coverage; they do not establish complete React Native feature parity.

The shared `crates/mobile-bridge` reuses the host's Noise channel and authenticated heartbeat. Android packages ARM64/x86_64 JNI libraries; pairing invitations are stored separately from display metadata, and authentication failure closes the connection. Apple has Swift transport bindings and an XCFramework build script for device, simulator and Mac. On macOS, run `python3 apps/ios/scripts/build-pairing-apple.py` before resolving the Swift package in Xcode. The Apple check script also builds it automatically. Apple uses direct C references so static linking retains bridge symbols.

## Observed validation (Linux, Android API 35 emulator)

- Android core regression: 28 passed, two opt-in acceptance tests skipped by the default suite. Both acceptance tests passed separately against isolated Rust services.
- Android emulator regression: eight passed, one opt-in real-host test skipped by the default suite. This includes the actual Android Keystore DPoP signer.
- Packaged Android real-host acceptance: one passed separately. The installed app's JNI transport authenticates an invitation, verifies host identity, transfers a multi-chunk UTF-8 attachment through the encrypted channel, and observes revocation. The launcher uses a private app file for temporary credentials and removes it and the ADB reverse route afterwards. Run `python3 apps/android/scripts/test-real-engine.py --device` with JDK21 and Android SDK/NDK configured.
- Portable Swift regression: 103 passed, including actual bridge loading, account renewal/expiry and token-bound proofs. A second run compiled and tested the direct C module bindings against the Linux bridge. This verifies the binding signatures, not Apple SDK linkage or Apple WebSocket behavior.
- Rust bridge cross-compilation to the iPhone ARM64 static library passed on Linux. XCFramework packaging and the final application link still require Xcode.
- Shared Rust bridge: two cryptographic channel tests passed, covering large bidirectional records, heartbeat proof, rejection before authentication and replay failure.
- Rust scoped diff regression passed for staged versus unstaged/untracked changes.
- Apple application syntax parsing passed earlier; full Apple SDK type checking, XCFramework generation, simulator and Mac UI/runtime tests have not run because this machine has no macOS/Xcode.

Ignored local toolchains and test logs are under `.context/android-tools` and `.context/swift-tools`. Imported upstream verification scripts still reference unavailable TypeScript workspace packages and need adaptation before they can serve as complete release gates.

## Remaining acceptance

### Local Rust staging desktop → Android

The staging profile uses `http://127.0.0.1:28700` for the local Rust account API;
it does not use the hosted TypeScript staging service. Direct computer pairing
works independently of that API or account sign-in. Build the browser assets with
`scripts/build-web.sh`, then run `bash scripts/run-staging-desktop.sh` to open the
headed Rust app with an isolated `.context/staging-desktop` profile and IPC port
28654. With JDK21, Android SDK/NDK and one local emulator available, run
`python3 apps/android/scripts/connect-local-desktop.py`.

That command starts the running desktop engine's remote listener on loopback
28655, generates an encrypted invitation, installs the staging Android app, and
tests host identity, remote chat creation and opening the chat in Compose. It
keeps the saved computer and ADB reverse route for manual use. Invitations are
owner-readable files under `.context`, never command arguments or logged bearer
tokens. Port 28700 is reserved for a separately launched Rust account API; this
direct-pairing command does not start or claim to verify account/cloud services.

Observed on the API35 emulator: staging desktop → Android acceptance passed,
including persistent encrypted pairing, host identity, chat creation, opening
the chat in Compose, and a real Codex request/reply (`--agent`). The installed
app was relaunched and restored the connected chat and reply. A separate read
through the headed desktop's IPC confirmed Android's chat mutation. The desktop
and emulator remain visible locally. The local Rust account API on 28700 is not
started by this procedure; hosted TypeScript staging is not used.

Android's account HTTP client now has an explicit loopback opt-in, enabled only
by debug staging ViewModels. Production still requires HTTPS. Tests verify
loopback requests, default rejection, remote HTTP rejection and cross-origin
rejection before credentials are obtained. The core suite and staging Kotlin
compilation passed after this change. Actual signed-in discovery against the
local Rust account API remains unverified; that API still needs to be started.

The desktop launcher explicitly sets `ZUSE_ALLOW_LOOPBACK_ACCOUNT_HTTP=1` for
its local Rust API origin. Without that opt-in, account origins still require
HTTPS. The opt-in only accepts literal loopback IPs; it does not permit HTTP
for LAN addresses or DNS names.
The targeted Rust regression
`cargo test -p zuse-engine --lib account::tests::local_http_requires_opt_in_and_a_literal_loopback_origin`
passed (one test), covering the opt-in, IPv4/IPv6 loopback, remote HTTP rejection,
and rejection of credentials, paths, queries and fragments.

Both native platforms now select Rust's `zuse-rpc-json-v1` account transport only when computer discovery advertises both wire version 1 and the protocol capability. Each registers its native DPoP key, requests a managed grant, authenticates before RPC dispatch, pins the host identity, renews grants, and closes on expiry or failed authorization. It does not fall back to the legacy transport after rejection. DPoP proofs on both native platforms now bind device access tokens through the `ath` claim. Android's actual keystore signer passed emulator checks; portable Swift verifies the bound signature and hash.

The Android core regression after these changes passed 28 tests with two opt-in acceptance tests skipped. Four account transport tests cover authentication/renewal, identity rejection, idle expiry and route/grant validation; a cloud HTTP/WebSocket fixture covers registration and managed bootstrap. The isolated Rust managed-server acceptance gate is `cargo test -p zuse-serve --test managed_rpc android_account_client_acceptance -- --ignored --nocapture` with JDK21 configured. It passed: the real Android JVM client authenticated, renewed past the original grant lifetime, cancelled a canonical stream, and continued making requests. This uses an isolated Rust test authority, not a live cloud account. The existing Rust managed-server suite also passed four tests (the native acceptance is opt-in there).

iOS account transport has four portable tests for authentication/renewal, wrong-host rejection, idle expiry and route/grant validation. Its real Apple WebSocket runtime is not yet verified. Cloud runtimes still need contract integration. Live signed-in account discovery, DPoP registration and authorization with the cloud authority have not been verified. Audit all non-terminal React Native flows against running services, including settings, account management and remote lifecycle. Verify Apple packaging and encrypted transport on macOS. Measure cold connect, reconnect and large threads under 400 kbps up/down and 400 ms RTT, with medians of three runs. Upstream screenshots are reference claims; there is no measured Zuse 3G comparison yet.

### Local Rust API runtime evidence

The dedicated `zuse-rust-local-staging-db` PostgreSQL16 container listens only
on 127.0.0.1:28701. Its empty staging database received all 49 upstream Drizzle
migrations and ten Rust coordination migrations, with a private local migration
ledger. No production database or fixture account rows were used. Credentials
are in the owner-readable-only `.context/android-tools/local-rust-api.env`.
`python3 scripts/run-local-rust-api.py` builds and launches the actual Rust API
on 127.0.0.1:28700. `python3 scripts/check-local-rust-api.py` passed: `/healthz`
200, `/readyz`200 and unauthenticated `/v1/cloud/workspaces`401. This confirms
service/database readiness and unauthenticated rejection, not signed-in account
discovery, managed grants or cloud runtime execution.
