# Rust cloud implementation

Reference: original Zuse `2016cf4f0cc240e6797916dd5cab1d8bebab89ae`.
Target origin: `https://api-rust.zuse.sh`. Work takes place on `swarajbachu/rust-cloud-parity`;
do not publish to `main`. The [feature inventory](cloud-parity-inventory.md) defines parity.
UI acceptance must save before/after screenshots under `.context/` and embed the
completed UI images in the final user-facing report.

## Milestone ledger

| Milestone | Implementation | Acceptance |
| --- | --- | --- |
| A: inventory/service | Pinned inventory; Axum service, WorkOS verifier, personal/org read authorization, existing catalog/entitlements, additive job/ownership schema, read-only inspection | Signed-JWT and PostgreSQL/HTTP acceptance passed; existing-account live verification pending |
| B: real cloud chat | Direct Boxd lifecycle/bootstrap transport, legacy encryption and transactional runtime bootstrap/ACK/renewal added; allocation reconciliation, images, credential grants, gateway/mailbox, Rust guest and UI create/send/reopen pending | Boxd HTTP/protobuf and WebCrypto compatibility fixtures pass; live cloud chat pending |
| C: billing/admin | Pending: Stripe/Polar writes, ledger/export, provider credentials, images, machine administration | Pending |
| D: tools/integrations | Existing local tools; hosted routing/sync/device bridge/org automation pending | Pending |
| E: archive lifecycle | Existing local archive; legacy-compatible cloud manifest/status/retention contracts added; cloud backup/retirement/restore/delete pending | Pending |
| F: complete GPUI parity | Existing partial UI; complete reference comparison pending | Pending |
| G: migration/cutover | Read-only inspection added; ownership handoff, guest conversion, archive migration and live cutover pending | Pending |

Do not mark a milestone complete from compilation alone. Record tested behavior and remaining gates here.

## Current service

`apps/cloud` runs the hosted API; `crates/cloud` owns authentication, authorization,
catalog reads and durable job primitives. Shared archive contracts live in
`zuse_protocol::cloud`. No new history database or alternate local engine is introduced.

The API defaults to **read-only**, including PostgreSQL session enforcement for
catalog reads. Explicitly enabled runtime control uses a separate writable pool and
requires a matching Rust workspace owner/generation in each transaction. It never
wakes machines, transfers ownership, exports charges or reconciles checkouts.
It serves only implemented routes:

- `GET /healthz`, `GET /readyz`.
- `GET /v1/cloud/context` (new: authenticated actor, selected scope and owner).
- `GET /v1/billing/entitlements` (existing persisted entitlements, without checkout repair).
- `GET /v1/cloud/workspaces[?projectId=...]`, `GET /v1/cloud/workspaces/:id`.
- `GET /v1/cloud/workspaces/:id/archive-status` (new: retained machine, deadline,
  backup/retirement/restore phase and verification progress; no compute activation).

With `ZUSE_CLOUD_RUNTIME_WRITES=true`, these routes are additionally registered:

- `POST /v1/cloud/workspaces/:id/runtime/bootstrap`.
- `POST /v1/cloud/workspaces/:id/runtime/bootstrap/ack`.
- `POST /v1/cloud/workspaces/:id/runtime/credentials/renew`.

Other feature routes remain absent until their behavior is implemented. Runtime-derived
native-agent availability is not yet returned by the workspace projection.

## Run

Set `ZUSE_CLOUD_DATABASE_URL` explicitly using a private environment file or secret
manager. The executable never falls back to `DATABASE_URL`, which can point to production.
Use a read-only PostgreSQL role for API rollout. Startup never runs migrations.

```sh
cargo run -p zuse-cloud-api -- serve
cargo run -p zuse-cloud-api -- inspect
```

`inspect` reports aggregate counts and schema readiness only; no tokens, user IDs,
messages or credentials are printed. It makes no ownership changes.

Configuration:

| Variable | Default / purpose |
| --- | --- |
| `ZUSE_CLOUD_DATABASE_URL` | Required explicit PostgreSQL connection |
| `ZUSE_CLOUD_LISTEN` | `127.0.0.1:8080`; containers explicitly set `0.0.0.0:8080` |
| `WORKOS_CLIENT_ID` | Existing production public AuthKit client ID |
| `WORKOS_ISSUER` | `https://api.workos.com` |
| `WORKOS_JWKS_URL` | Existing WorkOS JWKS URL derived from the client ID |
| `WORKOS_API_KEY` | Required for live organization membership reads; remains server-side |
| `ORGANIZATION_WORKSPACES_ENABLED` | Defaults false |
| `ZUSE_CLOUD_ORGANIZATION_IDS` | Explicit migration allowlist; default empty denies all org access |
| `ZUSE_CLOUD_ALLOWED_ORIGINS` | Comma-separated exact HTTPS browser origins; empty denies cross-origin browser access |
| `ZUSE_CLOUD_RUNTIME_WRITES` | Defaults disabled; literal `true` enables owner-fenced bootstrap/ACK/renewal |
| `CLOUD_DATA_ENCRYPTION_KEY` | Existing master key required for runtime writes; legacy fallback `CLOUD_CREDENTIAL_VAULT_KEY` |
| `RELAY_MINT_PRIVATE_JWK` | Existing Ed25519 mint key required for runtime writes |
| `API_ISSUER` | Runtime ticket/gateway origin, defaults `https://api-rust.zuse.sh` |

Organization access rechecks membership and separates content/admin/billing roles.
Only allowlist organizations after their existing rollout/GitHub-enrollment policy has
been assessed. Porting those additional policy gates remains required before broad
organization rollout. Personal records are never a fallback for an organization request.

## Coordination and archive requirements

`crates/cloud/migrations/0001_control_plane.sql` adds only Rust coordination tables.
It neither replaces original `api_*` tables nor assigns any resource to Rust.
Apply it explicitly to an isolated test database first. Jobs require a matching
Rust owner and generation before claim; enqueue belongs to the same transaction as
the eventual lifecycle intent. Reconcilers must additionally fence provider effects
and acknowledge results in the resource transaction before production activation.

Archive expiry is ten days and never implies history deletion. Legacy deadlines
are preserved or shortened to that window. A verified manifest/chunk count and a
persisted retirement phase are required to report backup verification. This status
reader alone cannot authorize machine deletion. Full backup/restore helpers,
verification, signed capabilities, source-machine fencing and explicit-delete purge
are still pending. Recovery stays disabled until provider acceptance passes.

## Focused verification

Use a disposable localhost PostgreSQL database with a separate test URL:

```sh
cargo test -p zuse-cloud
ZUSE_CLOUD_TEST_DATABASE_URL=postgres://user@127.0.0.1:5432/test \
  cargo test -p zuse-cloud --test postgres -- --ignored
cargo test -p zuse-protocol cloud::tests
```

The PostgreSQL scenarios create and remove only generated test schemas. They cover
actual HTTP routing, personal isolation, organization sharing/financial roles,
secret-free catalog projection, archive reads without mutation, idempotent jobs,
competing claims, expired-worker recovery and stale lease rejection. Runtime enrollment
acceptance also covers concurrent bootstrap retries through real HTTP handlers,
verified Ed25519 gateway JWTs, original launch-intent identity and transcript-key
preservation, changed-key rejection, idempotent ACK, ownership-generation fencing
and transaction rollback when initial launch state is unavailable. Renewal acceptance
covers fresh guest proofs, expired-bearer recovery, concurrent/lost-response retries,
stale bearer/request rejection, deletion fences and valid-bearer legacy compatibility.

Verified on October 22, 2026: four cloud unit/HTTP fixture tests and both
PostgreSQL/HTTP acceptance scenarios pass. JWT acceptance uses generated RSA keys and an actual JWKS
HTTP endpoint; the catalog scenario uses PostgreSQL 15, original column names and
real Axum handlers. The deployed read-only Store reports its actual PostgreSQL
read-only setting. These fixtures establish local behavior, not live WorkOS accounts,
provider execution, billing, migration or deployment.

The built executable also passed local `/healthz`, `/readyz`, unauthenticated API
rejection and `inspect` smoke checks against a separate seeded localhost database.
Crate-scoped strict Clippy and formatting pass. Dependency-inclusive strict Clippy
finds existing unrelated warnings in `protocol/presentation.rs` and `plugin_state.rs`.
Container definitions and CI are prepared; Docker is unavailable in this workspace,
so a container build is not claimed. The configured existing database connection
was unavailable during a read-only schema inspection; no production write was attempted.

The direct Boxd adapter now exchanges and caches provider tokens and implements
prepared-snapshot creation, machine inspection, start/hibernate/wake/resume/delete,
idle policy, bounded bootstrap exec and small file upload. Its HTTP fixture compares
request bytes with independently generated official SDK 0.2.14 fixtures, decodes
chunked responses and checks stdout/stderr/exit status, token reuse, provider rejection
and no automatic replay after an ambiguous create response. It does not yet reconcile
allocations, verify runtime boot, perform image operations or connect chat execution.
The provider remains unregistered in the API until that workflow is implemented.

To regenerate wire fixtures, extract the official SDK 0.2.14 package with its
declared dependencies outside tracked source, then run:

```sh
node scripts/fixtures/generate-boxd-wire.mjs /path/to/sdk/package
```

Provider source contracts and remaining operations are recorded in
[provider port notes](cloud-provider-port.md). No live provider acceptance is claimed.

Cloud encryption uses the original AES-256-GCM JSON envelopes/AAD, account/workspace
transcript-key wrapping, HMAC archive/API digests, RFC 7638 RSA thumbprints and
RSA-OAEP-256/A256GCM compact runtime grants. Existing ciphertext and digest vectors
generated with WebCrypto pass without conversion; wrong owner/workspace contexts
are rejected. The synthetic RSA fixture contains only a generated test key.
`scripts/fixtures/generate-cloud-crypto.mjs` regenerates those independent vectors.
Transactional bootstrap now uses these primitives to retain the original wrapped
transcript key and decrypt the existing launch intent. It binds the enrollment receipt
to the boot token, both runtime public keys, generation and gateway epoch. Concurrent
retries return the persisted receipt; ACK clears the boot credential once. Missing
initial launch data aborts the whole transaction. Credential renewal verifies the
registered guest's Ed25519 proof and persists the existing renewal receipt format;
expired bearer recovery requires that proof. Delete intent metadata fences callbacks
even if row state regresses. Runtime control does not yet issue
provider launch grants, resolve Git identity, attach a gateway or start an agent.
Production-key and historical-data acceptance remain pending.

Before enabling writes, the legacy lifecycle writer must also honor ownership or be
fenced/drained. The new ownership table alone does not stop the existing scheduler.
No production migration, owner reassignment or runtime-write activation has occurred.

Hosted CI is externally blocked: run `39848991883` for `604fa76` has no job steps;
GitHub reports failed recent account payments or a spending limit requiring an increase.
Local tests remain the available verification until repository billing is restored.

## Remaining external acceptance

Deployment to `api-rust.zuse.sh`, WorkOS live-account acceptance, provider credentials,
R2 access, Stripe sandbox verification, controlled runtime migration and physical
Mac/Linux UI acceptance are not established by local fixtures. Existing production
data and billing ownership must remain unchanged until the corresponding milestone
passes. The current desktop account URL remains `api.zuse.sh` until cloud compatibility
is implemented and rollout explicitly selects the Rust origin.
