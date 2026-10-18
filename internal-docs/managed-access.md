# Account-managed access client

The engine's `managed_access::AccountClient` uses Zuse's existing account API and `Account` lifetime. One client belongs to each engine core; RPC service clones share its proof-key and credential state. This implements the account-control boundary, not the complete managed tunnel or remote connection UI.

## Available engine operations

- `ListAccountEnvironments` returns the signed-in account's linked computers, including endpoint, runtime, capability and public identity metadata.
- `ListAccountDevices` returns authorized client metadata.
- `RevokeAccountDevice {clientId}` asks the account service to revoke one client. It is a write and is excluded from interrupted-request retries.

These operations belong to the connected profile and reject a foreign `targetDeviceId`. Reads are explicitly classified as retryable; they do not start local agents, connect to advertised endpoints or imply host reachability. Account errors do not close otherwise healthy engine RPC connections.

## Shared account-control implementation

The same client implements environment linking and client-bound connection bootstrap for the forthcoming host owner. Linking obtains an account challenge, signs it with a profile-bound Ed25519 key and sends the public host identity. The advertised local origin is derived from the supplied loopback listener port, rather than an arbitrary caller URL. Receipts validate environment identity, public mint key, endpoint/origin correspondence and the managed tunnel hostname/token. Secret-bearing link and bootstrap receipts deliberately cannot be serialized into ordinary UI metadata.

The host key is created once in `managed-access/identity.json` beneath the current profile. Its versioned record binds the key to the device identity. Directory/file permissions are private on Unix; creation uses a synced temporary file and atomic no-clobber persistence. Restart reuses the public identity. Invalid, foreign and damaged records are reported instead of regenerated. The account API receives only the public JWK and signed proof.

Connection bootstrap uses an ephemeral P-256 proof key for the account lifetime. Proofs bind the method, target URL, fresh nonce and issue time; authenticated proofs also bind the access token hash. Token exchange and client registration share one admission gate. Cached access has monotonic and wall-clock expiry with renewal slack. A definitive authentication rejection invalidates that credential and permits one authentication retry; timeouts, server errors and redirects are never treated as permission to replay an action. The caller must supply the intended wire protocol version; the client does not claim compatibility with an unrelated protocol by default.

Every request captures the account cancellation lifetime and checks its subject before and after I/O. Sign-out, account replacement and engine account retirement cancel in-flight work and retire cached proof credentials on subsequent use. Refreshing an account token does not tear down a healthy application connection. Responses are streamed with a 1 MiB limit; HTTP has bounded connection/request deadlines and follows no redirects. Errors omit tokens, proof bodies, private key material and connector credentials.

## Verification and remaining work

Three focused engine scenarios exercise the actual HTTP client and cryptographic proof verification against a local account-service fixture: linking and stable private host identity; twelve concurrent bootstrap calls with one rejected cached credential; redirect refusal and non-replay of a failed revocation; account-switch cancellation/cache replacement; and connected-profile RPC admission without breaking engine identity reads. The fixture represents the external account boundary, not live account authorization or deployed endpoint acceptance.

Local verification passed: the three focused engine scenarios, the native desktop/CLI/viewport check, and the production WASM build. Workspace evidence is recorded in `.context/managed-access-tests.log`, `.context/managed-access-native-check.log` and `.context/managed-access-web-build.log`. No deployed account, connector or physical macOS acceptance was performed for this milestone.

The remaining managed-access owner must persist account-bound link configuration, supervise the outbound connector, implement host authorization/bootstrap compatibility, reconcile restart/revocation and drive native/browser connection controls and the saved environment/route catalog. The existing operator relay, Noise session, heartbeat and uncertainty policy remain reusable. No user-visible managed-access completion or live account acceptance is established by this client milestone.
