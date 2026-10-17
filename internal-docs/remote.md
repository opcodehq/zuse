# Paired remote access

The shared encrypted channel is implemented in `zuse-remote` for native and WASM. The engine owns persistent pairing grants, and the browser server exposes a binary paired RPC endpoint. Native and browser connection adapters plus browser pairing controls are implemented. The outbound broker and native pairing settings controls remain pending. Existing device-room relay traffic retains its previous transport.

## Channel contract

A pairing capability contains a random 128-bit route ID and a random 256-bit pre-shared secret. Its explicit export is a versioned `#pair=` fragment. The secret wrapper redacts Debug output and zeroizes its key bytes on drop. Invitation parsing bounds input and rejects malformed or unsupported formats.

The fixed suite is `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`, implemented by [Snow](https://docs.rs/snow/0.10.0/snow/). The pair ID and protocol purpose are bound into the handshake prologue. Application data starts after the two-message handshake. Fresh ephemeral keys establish each connection; previous connection records cannot be accepted by a new channel. Cipher state and nonce sequencing come from Noise.

The [Noise specification](https://noiseprotocol.org/noise.html) limits each encrypted record to 65,535 bytes. The application layer fragments messages up to 16 MiB into ordered authenticated records. Receivers grow partial buffers as records arrive, with capacity bounded by the declared message limit. Authentication errors, invalid framing, reflection, replay and reordering close the channel permanently. Recovery requires a fresh handshake; this API has no plaintext fallback.

The native client and browser share the explicit `methods::retryable_read` classification. Only those read operations can redial after an uncertain RPC disconnect. Actions and unknown methods retain uncertainty instead of being repeated. This classification governs replay; execution-host authorization remains separate.

## Host authorization and transport

The canonical engine RPC exposes `CreatePairing {label}`, `ListPairings {}` and `RevokePairing {pairId}`. Creation returns `{peer, fragment}`; listing returns metadata only. A grant permits full control of the connected profile. There is no read-only pairing role. Controls reject a foreign `targetDeviceId`.

Each profile stores `pairing/peers.json` under its own store root. The directory is private (0700), files are private (0600), and an exclusive owner lock prevents competing authorities. Updates use a synced temporary file, atomic rename and directory sync. Failed persistence closes the authority and cancels its connection credentials. Damaged saved data is preserved and reported instead of silently reset.

Up to 32 devices may be saved. Invitations expire after ten minutes. The host makes a grant persistent only after a valid encrypted application request and before dispatching RPC; completing or replaying a handshake alone does not confirm it. Revocation and engine shutdown cancel active connection credentials. Already admitted agent jobs remain owned by the host.

`/api/paired-rpc/{pairId}` accepts binary Noise records and uses the existing canonical RPC dispatcher, including subscriptions and cancellation. The secret is never part of the endpoint URL. No cookie is required: explicit possession of the pairing capability supplies authentication. Text frames, invalid keys and malformed initial requests close the connection. Handshakes have ten seconds and initial application proof has thirty seconds. A shared limit of 64 paired sockets bounds pending and active connections. Explicit requests to another execution host are rejected so this endpoint cannot forward them through the old device-room relay.

## Connecting applications

Open Settings → Paired devices in the browser workspace, enter a device label and create an invitation. Copy its private link to the intended device. Opening it imports the capability, immediately removes the URL fragment and retains access in session storage for that tab. Reloading the tab makes a fresh Noise session. An invalid or revoked pairing never falls back to cookie-authenticated RPC. “Forget this browser’s pairing” removes the saved capability and reloads; save local drafts first.

Paired browser attachments use separate authenticated RPC connections, staging 45,000-byte chunks with sequence numbers and committing only after every acknowledgement. They never use the plaintext HTTP attachment route. Disconnects stop that upload instead of replaying requests. Workspace read subscriptions reopen after reconnect; actions retain their existing delivery uncertainty.

The desktop command accepts `zuse --paired-link-file /private/path/engine-link.txt`. The file contains only the copied private invitation URL. Imports are bounded and made private (0600 on Unix). An explicit paired destination skips local-daemon probing and embedded-engine creation, authenticates the remote host and reads its engine identity. Extra media connections use the same paired target. Reconnection never switches to a local engine or the legacy relay. Native pairing creation/selection controls and physical desktop runtime acceptance remain pending.

Paired origins require HTTPS; HTTP is accepted only on localhost for development. The native adapter uses the normal trusted-root TLS checks. Keep exported files private. The pairing secret is omitted from socket URLs, target Debug output and connection errors.

## Integration still required

- Native pairing/settings controls, saved target selection and user-facing connection management across desktop windows.
- Outbound host connectivity through an opaque broker, bounded handshake/session queues, reconnect and end-to-end liveness. Paired mode must reject legacy plaintext RPC instead of falling back after an authentication failure.
- Host identity pinning and complete connection-target selection. Pairing secrets stay outside relay URLs, logs and ordinary settings metadata.
- Physical Mac/Linux and browser acceptance, revoked and expired keys, broker outages, profile/account changes and deployment packaging. A trusted application bundle remains necessary for browser-held capabilities.

## Evidence

Six native channel scenarios cover invitation round trips, wrong keys/identities, malformed authenticated framing, bidirectional boundaries through 16 MiB, tampering, replay, reflection, reordering and reconnect keys. A WASM compile check validates the portable primitive selection and browser randomness bindings; it does not establish browser runtime acceptance. The full ten-scenario live-client suite passes, including a real device-room fault that drops an actual successful action reply: one action executes, its caller receives uncertainty, and a separate read query can reconnect and retry. The browser WASM build also passes with the shared method classification.

Local logs: `.context/paired-channel-final-tests.log`, `.context/paired-channel-final-wasm-check.log`, `.context/relay-client-live-tests.log`, and `.context/relay-policy-browser-build.log`.

The host integration passes five authority scenarios, five actual binary WebSocket scenarios and an engine RPC restart/profile-isolation scenario. The existing nine HTTP/TLS/upload tests pass alongside them. Subscription cancellation and revocation drop canonical streams; successful encrypted traffic confirms pending grants. Failure fixtures cover damaged storage, failed confirmation persistence, expired invitations, wrong keys, plaintext requests and foreign host routing. Native authority code is excluded from the passing WASM compile check. Evidence: `.context/pairing-transport-final-tests.log`, `.context/pairing-engine-tests.log` and `.context/pairing-authority-wasm-check.log`.

Application adapter validation passes 29 remote/serve scenarios, including destination validation and a native paired client exercising canonical calls, streams and revocation. The browser runtime imports/copies invitations through the actual UI, removes fragments, retains tab access, transfers exact text/image bytes without HTTP uploads, preserves drafts after a forced disconnect, receives a completed live Claude response, and rejects work after revocation despite an authenticated owner cookie. Existing-tab invitation navigation and explicit forget/reload are exercised separately. Evidence is recorded in `.context/paired-browser-*-acceptance.json`. The browser fixture records only ciphertext metadata; no paired RPC frame was plaintext. Native desktop compilation is a CI gate and physical runtime acceptance remains pending.
