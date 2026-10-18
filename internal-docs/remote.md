# Paired remote access

The shared encrypted channel is implemented in `zuse-remote` for native and WASM. The engine owns persistent pairing grants, and the browser server exposes a binary paired RPC endpoint. Native and browser connection adapters plus pairing settings controls are implemented. Native controls still require desktop runtime acceptance; the operator-managed outbound broker and settings-owned connector are implemented; account-scoped provisioning and production relay acceptance remain pending. Existing device-room relay traffic retains its previous transport.

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

Open Settings → Devices in the browser workspace, enter a device label and create an invitation. Copy its private link to the intended device. Opening it imports the capability, immediately removes the URL fragment and retains access in session storage for that tab. Reloading the tab makes a fresh Noise session. An invalid or revoked pairing never falls back to cookie-authenticated RPC. “Forget this browser’s pairing” removes the saved capability and reloads; save local drafts first.

Paired browser attachments use separate authenticated RPC connections, staging 45,000-byte chunks with sequence numbers and committing only after every acknowledgement. They never use the plaintext HTTP attachment route. Disconnects stop that upload instead of replaying requests. Workspace read subscriptions reopen after reconnect; actions retain their existing delivery uncertainty.

The desktop command accepts `zuse --paired-link-file /private/path/engine-link.txt`. The file contains only the copied private invitation URL. Imports are bounded and made private (0600 on Unix). An explicit paired destination skips local-daemon probing and embedded-engine creation, authenticates the remote host and reads its engine identity. Extra media connections use the same paired target. Reconnection never switches to a local engine or the legacy relay. Native Settings → Devices can create/copy/revoke invitations and open a pasted pairing link in a separate workspace window. Physical desktop runtime acceptance remains pending.

Paired origins require HTTPS; HTTP is accepted only on localhost for development. The native adapter uses the normal trusted-root TLS checks. Keep exported files private. The pairing secret is omitted from socket URLs, target Debug output and connection errors.

## Integration still required

- Persisted target selection and physical acceptance of connection management across desktop windows.
- Encrypted end-to-end heartbeat acceptance across an untrusted broker. Current control-frame heartbeats detect link failure; they do not establish application responsiveness through a broker. Paired mode rejects plaintext RPC and does not fall back after authentication failure.
- Host identity pinning and complete connection-target selection. Pairing secrets stay outside relay URLs, logs and ordinary settings metadata.
- Physical Mac/Linux and browser acceptance, revoked and expired keys, broker outages, profile/account changes and deployment packaging. A trusted application bundle remains necessary for browser-held capabilities.

## Evidence

Six native channel scenarios cover invitation round trips, wrong keys/identities, malformed authenticated framing, bidirectional boundaries through 16 MiB, tampering, replay, reflection, reordering and reconnect keys. A WASM compile check validates the portable primitive selection and browser randomness bindings; it does not establish browser runtime acceptance. The full ten-scenario live-client suite passes, including a real device-room fault that drops an actual successful action reply: one action executes, its caller receives uncertainty, and a separate read query can reconnect and retry. The browser WASM build also passes with the shared method classification.

Local logs: `.context/paired-channel-final-tests.log`, `.context/paired-channel-final-wasm-check.log`, `.context/relay-client-live-tests.log`, and `.context/relay-policy-browser-build.log`.

The host integration passes five authority scenarios, five actual binary WebSocket scenarios and an engine RPC restart/profile-isolation scenario. The existing nine HTTP/TLS/upload tests pass alongside them. Subscription cancellation and revocation drop canonical streams; successful encrypted traffic confirms pending grants. Failure fixtures cover damaged storage, failed confirmation persistence, expired invitations, wrong keys, plaintext requests and foreign host routing. Native authority code is excluded from the passing WASM compile check. Evidence: `.context/pairing-transport-final-tests.log`, `.context/pairing-engine-tests.log` and `.context/pairing-authority-wasm-check.log`.

Application adapter validation passes 29 remote/serve scenarios, including destination validation and a native paired client exercising canonical calls, streams and revocation. The browser runtime imports/copies invitations through the actual UI, removes fragments, retains tab access, transfers exact text/image bytes without HTTP uploads, preserves drafts after a forced disconnect, receives a completed live Claude response, and rejects work after revocation despite an authenticated owner cookie. Existing-tab invitation navigation and explicit forget/reload are exercised separately. Evidence is recorded in `.context/paired-browser-*-acceptance.json`. The browser fixture records only ciphertext metadata; no paired RPC frame was plaintext. Native desktop compilation is a CI gate and physical runtime acceptance remains pending.

## Settings-owned browser access

Native Settings → Devices exposes browser access status, start/stop, listen address, public HTTPS URL and certificate/key paths. The embedded engine owns this listener; RPC service clones cannot extend its ownership. Shutdown cancels listener admission before awaited cleanup and closes upgraded connections. Stopping browser access preserves saved device grants. A foreground server reports its existing listener instead of allowing a duplicate start; its process owns shutdown.

`BrowserAccessStatus`, `StartBrowserAccess` and `StopBrowserAccess` reject foreign targets. Status exposes no access token. The optional start parameters are `listen`, `publicOrigin`, `tlsCert`, `tlsKey` and `assets`. Non-loopback access requires an explicit HTTPS public origin; direct HTTPS also requires a certificate and matching key. Browser assets are discovered from `ZUSE_WEB_ASSETS`, an executable sibling `web`, macOS bundle `Resources/web`, or development `dist/web`. Starting direct browser access is currently explicit per engine session; direct-listener auto-start preferences and complete bundle assets packaging remain pending.

Three real-engine/control scenarios exercise start/stop/restart with a native paired RPC subscription, persisted grants, shutdown/drop ownership, duplicate admission and invalid/foreign settings. Evidence: `.context/native-pairing-browser-access-tests.log` and `.context/settings-engine-final-tests.log`.

## Operator-managed outbound relay

`zuse-relay` serves the browser assets and two rendezvous endpoints. Hosts authenticate `/api/paired-host/{pairId}` with an independent 64-hex-character operator token in an Authorization header. Clients use the existing `/api/paired-rpc/{pairId}` path. The broker forwards binary records and has no Noise secrets or canonical RPC dispatcher. A host must register before a client can connect. Client text frames are rejected. There are at most 256 host sockets and eight idle registrations per pair ID, with RAII route cleanup, bounded WebSocket messages, five-second send deadlines and link heartbeats.

`zuse-server --relay-origin ORIGIN --relay-host-token-file FILE` maintains four outbound sockets per saved grant, within the 32-grant profile limit. Freshly created invitations register automatically. Expiry, revocation and shutdown cancel workers and sessions. Disconnection retries host registration with bounded backoff; each client connection performs fresh Noise authentication and application proof through the same host session as direct access. The existing shared 64-session host admission limit also applies to relayed RPC. Restarting a connector preserves grants and cannot replay actions. Private routing files are bounded and made 0600 on Unix; their contents are never in URLs or status metadata.

The foreground server advertises the broker origin for invitations while its local owner browser remains on the direct origin. Creation returns that public origin to native/browser controls. Browser-held capabilities therefore use the normal paired adapter without a second transport policy.

Three actual-socket relay scenarios cover concurrent native clients, 180 KB payloads, canonical subscriptions, profile routing rejection, revocation, connector process-task replacement, shutdown, unauthorized host registration, wrong pairing keys and rejected plaintext. The full transport suite remains a regression gate for the shared direct/relay session. Evidence: `.context/relay-transport-tests.log`.

Still pending: per-account host credential provisioning/rotation, encrypted end-to-end heartbeats, broker restart/outage acceptance through browser and physical Mac/Linux clients, and production deployment. The global operator token is suitable for an operator-controlled relay; it is not a tenant authorization service. Control-frame pings are not proof that an untrusted broker can reach the execution host.

Connector cancellation is also a canonical RPC lifetime gate. Request tasks now have a connection-owned drop guard, so aborting the dispatcher cancels its subscriptions. Reusing a request ID cancels the previous task. The connector-replacement scenario asserts that its subscription guard is released; it failed before this fix (`.context/relay-cancellation-regression-before.log`) and passes afterward in the transport suite. Shared host writes have a 30-second deadline.

The current connector keeps four sockets per grant. Burst media/uploads can exhaust those slots; queued connection admission remains an acceptance gap alongside end-to-end heartbeats.

## Settings-owned outbound relay

Native and browser Settings → Devices provide relay URL and private host credential file fields, explicit start/stop and transport status refresh. The credential path refers to the engine computer. `RelayAccessStatus`, `StartRelayAccess {origin, hostTokenFile}` and `StopRelayAccess` belong to the connected profile and reject foreign targets. Status exposes only the normalized origin, running flag, broker socket/session counts and a sanitized terminal error, never credential contents or paths. A running connector with zero sockets does not establish broker reachability or encrypted application responsiveness. Grants are registered as they appear; no local HTTP listener or browser asset folder is required for outbound access.

The engine owns the connector. RPC clones cannot extend its lifetime; stop drains workers before admitting a replacement, cancels active relayed sessions and retains pairing grants. Direct and relay sessions share the profile's 64-session admission budget. Invitations advertise the relay origin while its connector is running and fall back to direct browser access after stop. Existing invitations retain their exported origin. The CLI relay flags now start this same connector, so Settings can inspect and stop a foreground server's relay too. Native invitation copying uses the creation receipt's origin instead of a cached browser origin.

Starting relay access saves the validated origin and absolute credential-file path in the connected profile's `relay-access.json`. The file is versioned, bound to this device, atomically replaced and private on Unix. Token contents are never copied into settings. Status reports `configured`, `autoStart` and the origin, without exposing the credential path. Native/browser Devices settings have a Start when Zuse opens switch; `SetRelayAutoStart {enabled}` saves that choice without changing the current connector. Stopping affects the current session and preserves the startup choice. With both form fields blank, Start resumes saved configuration through `ResumeRelayAccess`.

Runtime assembly restores an enabled configuration once after building the RPC service. A malformed/foreign saved record or missing credential leaves the engine running and exposes a sanitized Settings error. Disabling startup still works if the credential has disappeared. Shutdown cancels/drains the connector without changing the preference. Explicit server relay flags replace an automatically restored connector and save the supplied configuration. Settings controls serialize admission; admitted RPC mutations complete after their caller disconnects. Browser uncertain receipts reconcile with status reads instead of replaying a mutation.

Host credentials remain operator-issued private files. Automatic account credential provisioning/rotation, encrypted end-to-end heartbeat, queued media admission and physical outage recovery remain open. Real-broker cold-core and outage recovery checks establish native encrypted reconnection, not physical/browser acceptance or broker trust. Refresh status after reconnecting.
