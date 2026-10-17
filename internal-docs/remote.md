# Paired remote access

The encrypted channel component is implemented in `zuse-remote` and compiles for native and WASM. It is not yet connected to the existing device-room relay or application pairing controls. Existing relay traffic retains its previous transport until that integration is complete.

## Channel contract

A pairing capability contains a random 128-bit route ID and a random 256-bit pre-shared secret. Its explicit export is a versioned `#pair=` fragment. The secret wrapper redacts Debug output and zeroizes its key bytes on drop. Invitation parsing bounds input and rejects malformed or unsupported formats.

The fixed suite is `Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s`, implemented by [Snow](https://docs.rs/snow/0.10.0/snow/). The pair ID and protocol purpose are bound into the handshake prologue. Application data starts after the two-message handshake. Fresh ephemeral keys establish each connection; previous connection records cannot be accepted by a new channel. Cipher state and nonce sequencing come from Noise.

The [Noise specification](https://noiseprotocol.org/noise.html) limits each encrypted record to 65,535 bytes. The application layer fragments messages up to 16 MiB into ordered authenticated records. Receivers grow partial buffers as records arrive, with capacity bounded by the declared message limit. Authentication errors, invalid framing, reflection, replay and reordering close the channel permanently. Recovery requires a fresh handshake; this API has no plaintext fallback.

The native client and browser share the explicit `methods::retryable_read` classification. Only those read operations can redial after an uncertain RPC disconnect. Actions and unknown methods retain uncertainty instead of being repeated. This classification governs replay; execution-host authorization remains separate.

## Integration still required

- An engine-owned, profile-scoped pairing authority: create/list/revoke, private atomic persistence, pending invitation expiry and active-session revocation.
- Authenticated channel adapters for the canonical RPC byte pumps, including native and WASM clients. Confirm a pairing after a valid encrypted application frame and before admitting RPC work; a handshake replay alone must not make an invitation permanent.
- Outbound host connectivity through an opaque broker, bounded handshake/session queues, reconnect and end-to-end liveness. Paired mode must reject legacy plaintext RPC instead of falling back after an authentication failure.
- Pairing/settings controls and connection-target selection, plus encrypted RPC upload/chunk paths for the browser. Pairing secrets stay outside relay URLs, logs and ordinary settings metadata.
- Physical Mac/Linux and browser acceptance, revoked and expired keys, broker outages, profile/account changes and deployment packaging. A trusted application bundle remains necessary for browser-held capabilities.

## Evidence

Six native channel scenarios cover invitation round trips, wrong keys/identities, malformed authenticated framing, bidirectional boundaries through 16 MiB, tampering, replay, reflection, reordering and reconnect keys. A WASM compile check validates the portable primitive selection and browser randomness bindings; it does not establish browser runtime acceptance. The full ten-scenario live-client suite passes, including a real device-room fault that drops an actual successful action reply: one action executes, its caller receives uncertainty, and a separate read query can reconnect and retry. The browser WASM build also passes with the shared method classification.

Local logs: `.context/paired-channel-final-tests.log`, `.context/paired-channel-final-wasm-check.log`, `.context/relay-client-live-tests.log`, and `.context/relay-policy-browser-build.log`.
