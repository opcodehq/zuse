# Model connections

`crates/model-accounts` owns model registrations, OAuth, encrypted local records,
and account-scoped storage leases. `crates/engine/src/model_connections.rs`
combines the local and hosted stores under the active Zuse identity. CLI logins
remain independent. The harness broker still needs to consume this service.

Local storage uses a private profile directory, a 0600 key file, and an SQLite
WAL database. AES-256-GCM uses a fresh nonce for every write and authenticates the
provider, record kind, and registration ID. A damaged key is preserved rather
than replaced. The key lives beside the database; this protects a copied database,
not an attacker who can read the entire profile. Platform keychain integration
and packaged-profile acceptance remain pending.

Hosted storage uses the existing `/v1/model-connections/storage` API and its
provider-scoped lease. Every request is bound to the current account subject.
Shared registration JSON preserves the existing optional-property contract so
other agents can read the same records. Cancellation during acquisition or release
schedules best-effort release with the original lease token. Concurrent credentials
requests reread and renew under the lease, so agents share token rotation.

ChatGPT uses dynamic registration, PKCE, a single-use state, a nonce, and a
loopback listener. SuperGrok uses device authorization. Signed identities require
the expected issuer, client audience, expiry, subject, issued-at time, algorithm,
and compatible public-key usage. Disconnect clears local credentials and advances
the authorization generation before attempting provider revocation, preventing
late authorization from restoring a disconnected account.

Native and browser Settings expose storage selection, sign-in, reconnect, rename,
default selection, disconnect, and provider-plan acknowledgement. The browser
can submit a pasted loopback callback address when the engine runs elsewhere.
That fallback parses the address locally, validates the exact destination and
single-use state, and shares the normal PKCE exchange; it never fetches the pasted
URL. Credential reads have no public RPC route.

Run `cargo test -p zuse-model-accounts` for encryption/context tampering, damaged
key preservation, lease cancellation, signed identities, PKCE completion, refresh
rotation across agents, disconnect fencing, metadata privacy, and shared-record
compatibility. These use a local identity-provider fixture. Live authorization,
hosted-storage interoperability, UI workflow acceptance, and harness integration
remain required before calling the complete model-account setup accepted.
