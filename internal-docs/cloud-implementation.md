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
| B: real cloud chat | Boxd transport and durable prepared-image allocation worker; encrypted enrollment/renewal; leased Rust RPC gateway; protected guest storage; canonical launch, encrypted text/native-command recovery, retained queue/model/Stop recovery and native steering/input delivery. Remaining: authenticated HTTP creation/admission, image selection/builds, provider grants, context/attachment delivery, and UI flows | Deterministic-provider and real PostgreSQL/HTTP/WebSocket/guest-process acceptance passed; live-provider acceptance pending |
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
- `POST /v1/cloud/workspaces/:id/runtime/storage/enroll` (prepare/commit activation barrier).
- `POST /v1/cloud/workspaces/:id/ready` (`agent-started` receipt for Rust guests).
- `POST /v1/cloud/workspaces/:id/gateway/ticket`.
- `GET /v1/cloud/workspaces/:id/gateway` (authenticated WebSocket upgrade).
- `POST /v1/cloud/workspaces/:id/commands`, `GET .../commands/:commandId`,
  `DELETE .../commands/:commandId`, `GET .../commands/watch?afterRevision=...`.
- `GET /v1/cloud/workspaces/:id/data-key` (authorized editor only).
- `POST /v1/cloud/workspaces/:id/runtime/commands/lease` and `.../ack`.
  Mailbox enqueue/lease additionally require explicit per-workspace
  `request_config.rustCommandMailboxEnabled=true`; leave off until guest delivery
  and billing capacity/reservation integration pass acceptance.

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
| `API_ISSUER` | Runtime ticket/gateway origin, defaults `https://api-rust.zuse.sh`; plaintext HTTP is allowed only for loopback acceptance |
| `CLOUD_WORKSPACE_IDLE_TIMEOUT_MS` | Existing idle policy; defaults ten minutes, preserves earlier runtime-operation deadlines |

Organization access rechecks membership and separates content/admin/billing roles.
Only allowlist organizations after their existing rollout/GitHub-enrollment policy has
been assessed. Porting those additional policy gates remains required before broad
organization rollout. Personal records are never a fallback for an organization request.

## Guest executable

Build `cargo build -p zuse-cloud-guest`. The executable is `zuse-cloud-runtime`.
A launcher supplies these values through flags or the launcher environment:

| Variable | Purpose |
| --- | --- |
| `ZUSE_CLOUD_WORKSPACE_ID`, `ZUSE_API_URL` | Registered workspace and control-plane origin |
| `ZUSE_USER_DATA`, `ZUSE_CLOUD_WORKSPACE_ROOT` | Authoritative data directory and prepared checkout |
| `ZUSE_STORAGE_INCARNATION_ID`, `ZUSE_RUNTIME_GENERATION` | Control-plane-approved storage and runtime generation |
| `ZUSE_RUNTIME_BOOT_TOKEN_FILE` | Initial boot credential file; restart uses persisted identity |
| `ZUSE_WORKSPACE_RUNTIME_LOCK_FD` | Optional original launcher-held lifetime lock descriptor |

The guest serves agent MCP tools over its bound loopback IPC port and the authenticated
cloud gateway. It does not expose a public unauthenticated engine listener. Native
providers come from the shared engine registry; `--test-harness` selects only the
deterministic acceptance provider and cannot call live models. Initial repository
preparation and provider grants must complete in the allocation/image workflow.

## Coordination and archive requirements

`crates/cloud/migrations/0001_control_plane.sql`, `0002_gateway_leases.sql` and
`0003_command_mailbox.sql`
add only Rust coordination tables.
They neither replace original `api_*` tables nor assign any resource to Rust.
`0004_allocations.sql` additionally stores initial allocation intents, encrypted
short-lived boot grants and provider receipts; it does not transfer ownership.
Apply them explicitly to an isolated test database first. Jobs require a matching
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
cargo test -p zuse-protocol cloud
ZUSE_CLOUD_TEST_DATABASE_URL=postgres://user@127.0.0.1:5432/test \
  ZUSE_CLOUD_TEST_GUEST_BIN="$PWD/target/debug/zuse-cloud-runtime" \
  cargo test -p zuse-cloud --features runtime-acceptance --test postgres rust_cloud -- --ignored
cargo test -p zuse-cloud-runtime -- --include-ignored
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

Verified on October 22, 2026: five cloud unit/HTTP fixture tests, six
PostgreSQL/HTTP/WebSocket/engine/process scenarios and four guest identity/storage
checks pass (15 total). JWT acceptance uses generated RSA keys and an actual JWKS
HTTP endpoint; the catalog scenario uses PostgreSQL 15, original column names and
real Axum handlers. The deployed read-only Store reports its actual PostgreSQL
read-only setting. These fixtures establish local behavior, not live WorkOS accounts,
provider execution, billing, migration or deployment.

The built executable also passed local `/healthz`, `/readyz`, unauthenticated API
rejection and `inspect` smoke checks against a separate seeded localhost database.
Crate-scoped strict Clippy for cloud API/guest/runtime packages and formatting pass. Dependency-inclusive strict Clippy
finds existing unrelated warnings in `protocol/presentation.rs` and `plugin_state.rs`.
Strict Clippy for the RPC crate additionally reports seven existing warnings in
`rpc/client.rs`, `managed.rs` and `device_room.rs` (`manual_map`, `collapsible_if`,
`useless_conversion`); none point to the new cloud transport. These unrelated
files were not changed.
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
provider launch grants, resolve Git identity or start an agent.
Production-key and historical-data acceptance remain pending.

The gateway now mints existing-format one-minute client admission tickets and verifies
workspace/owner/protocol/generation/epoch bindings. It multiplexes opaque binary/text
payloads across one retained runtime socket, carries ticket-derived actor/permission
in `client.open`, supports v2 binary and v1 text/base64 adapters, and preserves v3
pending/available/reset behavior without queuing or replaying application requests.
A separate database lease admits one gateway process per workspace. Heartbeats check
lifecycle ownership; a conservative monotonic deadline stops forwarding if ownership
cannot be renewed. Socket queues and total buffered bytes are bounded.

Acceptance uses actual TCP WebSockets with two clients and a retained-runtime
replacement. It verifies identity-header spoofing has no effect, cross-workspace and
expired tickets/unknown origins fail, competing ownership fails, bytes remain exact,
responses stay on their originating connection, pending requests are not replayed,
v1/v2 adapters interoperate, and a generation handoff closes active sockets. Ticket
expiry governs admission, matching existing long-lived connection behavior; lifecycle
fences govern established sockets. No provider is allocated or resumed by the gateway.

Gateway organization admission remains explicitly disabled until runtime access
callbacks and ongoing collaboration authorization are ported. Deployment initially
requires one gateway instance or workspace-sticky routing: competing API processes
return `workspace_gateway_owned_elsewhere` instead of creating separate rooms.
A gateway backplane/routing rollout and live guest acceptance remain pending. The
Rust engine's distinct RPC schema uses the explicitly negotiated
`zuse-workspace-rust-v1` WebSocket subprotocol and `zuse-rpc-v1` inner RPC. It uses
the same routing frames, preserving legacy adapters separately. The cached workspace summary exposes the negotiated RPC mode without connecting to
compute. A workspace configured
with `runtimeRpcProtocol: "zuse-rpc-v1"` rejects legacy bootstrap/tickets and requires
the new protocol. Retained workspaces default to the original Effect RPC. A storage
format change still requires verified migration and a generation handoff.

`zuse_rpc::cloud` now connects a client and serves an existing engine over one cloud
gateway socket. Client close removes only its RPC subscriptions. Relayed command
admission is owned by the engine's tracked workers, so canceling a transport cannot
abandon admission after its durable claim. Network recovery opens a new connection;
application frames are never resent by this transport. Gateway generation fencing is
reported distinctly so the eventual guest supervisor can retire stale execution.

The `runtime-acceptance` scenario runs actual PostgreSQL, Axum/TCP WebSockets and
the existing Rust engine with a deterministic provider. It exercises bootstrap/ACK,
protocol selection, original thread/initial-conversation IDs, the sealed initial
command/turn identity, execution after client closure, transcript reopen, a complete
engine shutdown/reopen, retained native session ID and file changes, and duplicate
command receipts without a second provider execution. The dedicated feature keeps
engine/native dependencies out of normal control-plane builds. The cloud CI workflow
now includes this acceptance command; hosted execution remains blocked by repository
billing as noted below. This establishes the engine/transport workflow locally,
not live provider allocation, credential grants or populated-workspace migration.

`apps/cloud-runtime` packages the existing engine as `zuse-cloud-runtime`.
`crates/cloud-runtime` preserves original identity-file fields and JWKs, decrypts the
wrapped transcript key and persists enrollment before ACK. Renewals save their request
ID before sending, so a lost response is recovered with the same credential lineage.
A renewed current credential can finish an interrupted bootstrap ACK. One gateway
supervisor reconnects with bounded backoff while accepted engine work remains alive;
definitive authority/generation fences retire the engine.

Startup holds the original `.workspace-runtime.lock`, including a validated inherited
launcher descriptor, and the engine's own instance lock. The control plane must set
`request_config.rustRuntimeStorage` to `{ "format": "zuse-rust-engine-v1",
"storageIncarnationId": "..." }` before Rust enrollment. The guest additionally binds
workspace, account, incarnation and canonical database path in an owner-only local
marker and inside the existing engine SQLite database. An additional persisted
control-plane prepare/commit barrier binds initialization to the same machine,
recipient/signing keys and storage incarnation. Once committed, that authority never
reissues fresh-storage admission for an empty directory; losing both local marker and
database requires recovery. Retained storage is revalidated before opening the engine.
Existing legacy files require verified migration. A missing database after enrollment fails without creating a
replacement. No automatic legacy import or ownership transfer is performed.

The guest adopts the original thread, initial conversation, model, command and turn
IDs. It completes the initial launch receipt only after engine command admission.
Archived/fenced workspaces cannot complete that callback, and repeat receipts are
idempotent. Nonempty legacy permission lists, edit/bash auto-approval modes and
unimported forks currently fail explicitly; native credential grants, complete mode
translation and populated-workspace import remain required. The guest does not
advertise GitHub/device/mailbox capabilities that are not implemented.

The base container is `infra/cloud/runtime.Dockerfile`; prepared images must install
the selected native agent CLIs. Docker is unavailable here, so a container build is
not claimed. Local executable and process acceptance are recorded below.

Packaged guest acceptance starts the actual executable, enrolls and automatically
adopts the sealed initial launch, then stops and restarts without a boot token. It
verifies original history/device/key identity, one initial execution, refusal to
replace a missing database, refusal to create storage after both local marker and
database disappear, and rejection of late ready callbacks after archive. A separate
HTTP/PG scenario loses a committed renewal response and local ACK marker, then
recovers the same credentials and keys. Independent Node WebCrypto imports the
persisted RSA/Ed25519 JWKs; lock/storage checks preserve unrelated bytes and reject
legacy, cross-account and mismatched database ownership. Integrity scanning is kept
out of ordinary retained reopen; initial installation and archive verification own
that work.

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

## Durable mailbox port (in progress)

Additive migration `0003_command_mailbox.sql` persists original v3 encrypted
command envelopes and result ciphertext in PostgreSQL. Shared Rust types preserve
reference protocol, AAD field ordering, fingerprints, lease fields and terminal
states. The control plane never decrypts command bodies. Each accepted command commits a
generation-fenced `mailbox-wake` job in the same transaction; duplicate acceptance
reuses its job and scheduler failure rolls back acceptance. Provider reconciliation
of those jobs is still pending. Original Worker mailbox
imports are still pending; changing the writer without importing pending commands
is not supported.

Workspace/owner row locks serialize enqueue, lease, acknowledgment and lifecycle
admission. Enqueue preserves client command IDs, rejects different fingerprints,
strips client-supplied actor metadata, caps nonterminal count/bytes and orders
commands by session. Leases are thirty seconds. Expiry requires a proven later
runtime generation on the same storage before redelivery; changed storage marks
previously attempted commands outcome-unknown. Late definitive acknowledgments
remain accepted until a newer lease replaces them. Only never-leased work may be
cancelled. Archived history/status and authorized encryption-key reads do not contact
providers or wake compute. First key issuance installs the original wrapped format
under the workspace lock, so commands can be encrypted before a guest is enrolled.

This is not complete mailbox parity yet: desktop/browser encrypted client
transport, rich-context translation, provider wake reconciliation, billing capacity/reservations,
original mailbox import and archival/deletion cleanup remain required before
activation. The API is deliberately gated rather than advertising an unavailable
runtime capability.

Mailbox checks: `mailbox_durable_ordering_receipts_and_uncertain_lease_fences`
passes against isolated PostgreSQL 15 with actual Axum handlers. Covers durable
retries/conflicts, account isolation, author stripping, editor key access before guest enrollment and during archive,
per-conversation ordering, atomic wake-job responsibility/rollback, cancellation,
receipt redelivery, expiry barriers,
stale leases, storage replacement and archive/delete admission. Bounded HTTP
watch/lease waits use commit notifications with a one-second cross-process repair
poll; no database connection or workspace lock remains held while waiting.

Validation for this chunk: 12 cloud checks passed (five unit checks plus seven
isolated PostgreSQL/HTTP/WebSocket/guest-process scenarios), including the new
mailbox scenario. After the wake-job and key-issuance additions, the focused mailbox
scenario passed again. Four shared cloud protocol checks passed. Scoped strict
Clippy for `zuse-cloud-api`/`zuse-cloud`, package formatting, protocol formatting and
`git diff --check` passed. No production migrations, owner handoffs or provider
operations were performed. Live-provider, guest mailbox and billing acceptance
remain outstanding; the mailbox activation flag stays off by default.

### Guest delivery and canonical queue admission

The guest consumes original AES-GCM/HMAC v3 text-message envelopes using the
workspace transcript key. The engine admits the original command/message IDs to
its existing `SessionDoc` command queue, synchronously saves the canonical snapshot
and records durable discovery responsibility before returning admission. It reuses
the engine's normal drain, model settings, queued-turn handling, execution journal
and processed-command ledger. A retry preserves the first queued request/settings.
The hosted profile enables SQLite FULL synchronization before engine workers are
assembled, including command queue snapshots and processed claims. Local profiles
retain their existing policy. No direct tool replay or separate history database
is added.

Encrypted envelope bindings and acknowledgment receipts live in additive
`zuse_cloud_command_receipts` inside that same verified engine SQLite database.
Receipts are committed before HTTP acknowledgment. Restart first checks unfinished
receipt bindings against the canonical engine command entry; an existing bound
entry proves prior admission without a fresh tool call. Without that proof, the
guest requires a fresh valid lease. Lost ACK responses replay only receipts. Guest
leases bind the original workspace, sandbox, runtime generation and storage
incarnation. One supervisor renews credentials and publishes updates to the
independent long-polling consumer; gateway reconnect does not cancel admitted work.
Gateway command messages are notifications, never execution grants.

The consumer is enabled only by the explicitly gated bootstrap
`commandMailboxEnabled` flag. Ordinary runtime startup stays functional with the
flag off. Organization execution remains disabled. Text and empty-reference
composer inputs are supported; nonempty file/skill/annotation inputs require the
remaining context adapter, and unsupported content is rejected rather than lost.
Original v3 attachment/goal restrictions remain explicit. Full attachment, background-task and UI delivery still require their remaining inventory workflows. Cloud queue recovery now persists held run settings and explicit pause decisions;
long-running restart/Stop/Send now acceptance has passed. Full background-task, attachment and UI workflows still require their inventory acceptance.

Focused guest acceptance now proves encrypted commands accepted before startup,
original IDs/history retained through restart, a committed API ACK with its response
withheld, and a failed local receipt commit after canonical queue admission. The
latter is recovered from the existing command entry, without a new engine claim.
Independent Node WebCrypto checks verify AES-GCM/HMAC compatibility in both
directions, including rejection of modified routing metadata or keyed identity.
Archive/restore/deletion must carry or purge the new mailbox rows/jobs and local
receipt table as part of their existing ownership scope; those lifecycle workflows
remain pending.

Current Rust enrollment proofs now bind runtime generation/epoch before raw-bearer
gateway attachment, mailbox delivery, storage enrollment, launch readiness and
renewal. Committed storage additionally binds the current generation, sandbox,
incarnation and key thumbprints. A generation change with an unchanged old bearer
is rejected; fresh authorized enrollment is required. Legacy protocol adapters
retain their existing contract.

Validation: the final hosted guest builds; 18 cloud/guest checks and one hosted WAL
durability check passed locally. The cloud suite passed again after the enrollment
fence changes, and the independent crypto test additionally verifies omission of
original optional payload fields. Scoped strict Clippy and formatting passed.
No production owner transfer, live-provider execution, billing export, deployment
or full milestone completion is claimed by this chunk.

### Hosted queue continuation

Hosted profiles now journal local Loro updates to the existing SQLite outbox even
without an Edge connection. Reopen replays those committed updates before exposing
the document. This reuses the original journal and processed-command ledger.
Network sync remains separately controlled; no connection is started by that local
journal.

Held messages persist their original run request beside the queue row, preserving
model, reasoning, options, cwd and approval configuration through restart. Queue
pause decisions are persisted: explicit Stop and archive remain held, while a
runtime shutdown's temporary barrier does not revoke previously accepted work.
Hosted reopening follows the persisted policy; ordinary local profiles retain the
existing conservative reopen behavior. Snapshot and thin-document rebuilds carry
queued rows, their run settings and the pause decision together.

Queue verification: a real hosted engine is stopped and reopened twice while a
turn-boundary provider is active. Held and explicitly queued messages retain
selected model/options after subsequent settings changes. Explicit Stop remains
paused after reopen, and Send now delivers the retained row once with its original
ID. A separate snapshot/epoch-rebuild test preserves queue rows, pause state and
run settings. The rebuilt guest passed the full cloud regression suite. A separate normal-profile
reopen test confirms local queues still require an explicit send after restart.

Broader `zuse-doc` strict Clippy currently reports four existing diagnostics outside
these changes: `commands.rs` large enum variant, two collapsible conditionals in
`registry/sidebar_pins.rs`, and one in `registry/sidebar_sections.rs`. No unrelated
refactor is included. Cloud-package lint, focused checks and formatting remain the
verification gate for this chunk. Offline journal compaction/export and full cloud
UI/control integration remain required before production activation.

Final queue-chunk checks: 19 cloud/guest checks passed, plus the snapshot/rebuild
case and the local conservative-reopen case (21 checks total). Scoped cloud-package
strict Clippy and formatting passed. Broader Sync strict Clippy additionally reports
six existing diagnostics in `chat_client.rs` and `dial.rs`; none is in the changed
store code. The task branch remains separate from main. Full milestones and live
provider, migration, billing and UI acceptance remain pending in the ledger above.


### Native cloud command delivery

The encrypted mailbox accepts explicit versioned `zuse.session.run`,
`zuse.session.steer`, `zuse.session.interrupt` and `zuse.session.respond-input`
commands. They share the engine's existing session command payload types, preserving
local wire serialization and public Doc re-exports. The authenticated outside kind
must match the decrypted payload; a Run cannot be mislabeled as a control. Every
session remains FIFO at durable admission, preserving prompt/Stop ordering. Applied
means durably admitted; execution state still comes from the canonical transcript.

Native admission is separate from the prompt dispatch lock. Provider startup can
remain pending while Stop and input controls enter the durable queue. A temporary
admission barrier prevents workers from executing a command before its snapshot
is committed, then wakes both prompt and control drains. Production and acceptance
use the same EngineExecutor adapter. Default cloud command expiry is unbounded;
explicit expiry still applies. Archived accepted work must not silently expire
through the ordinary local 24-hour command default.

The initial launch uses this durable queue too, retaining original command/turn
IDs and allowing gateway readiness before provider startup completes. A successful
ready receipt clears the cached launch intent. Existing processed launches are
recognized only with the original user-message identity and text. Retained boots
open owned active conversation documents so background work resumes without a
viewer. Organization execution and rich context remain separately gated.

`data-key.sessionCommandVersion` advertises native support. Fresh bootstrap advertises
`rust-session-commands-v1`; retained upgrades refresh these known capabilities at
the authenticated storage commit barrier. Legacy storage commit requests remain
valid and clear native command support, so the authority does not hand a new command
to a guest that has not declared its decoder. Prepared-image allocation may explicitly set `rustSessionCommandVersion=1`
to admit work before boot, but leasing waits for the actual guest capability. That
flag is reserved for authority-approved images, not inferred from legacy protocols.
The allocator and browser/desktop client wiring remain pending.


Native-command verification: 33 local checks passed (five cloud fixture tests,
11 PostgreSQL/HTTP/gateway/engine/process scenarios, five guest crypto/storage
checks, four protocol cases and eight canonical command evaluator cases). The final
rebuilt executable passed all 11 PostgreSQL scenarios again after the decoder layout
and capability-selection changes. The native scenario interrupts a provider stalled
before stream creation, delivers steering and question answers once despite repeated
submission, rejects mismatched outside/payload kinds, and refreshes capabilities
from an older cached bootstrap through storage commit. Guest-process acceptance
also verifies the completed launch intent is absent from retained identity storage.
Scoped strict Clippy and formatting pass. The previously existing large payload
enum warning now resides in `protocol/session_commands.rs` after the unchanged
payload layout moved there; dependency-wide lint limitations remain as recorded.

This chunk does not enable production writes or charges. Allocation/image preparation,
provider grants, attachment/context import, billing, workspace lifecycle/archives,
client integration, full GPUI parity and migration still require their listed work.


### Prepared-image allocation worker

The authority can now admit initial allocation for an already Rust-owned, queued
personal workspace with an existing sealed launch intent and an approved prepared
Boxd snapshot. This operator path is deliberately separate from the pending public
creation endpoint: it does not bypass the future endpoint's account/image access,
entitlement, allowance and spend-reservation checks. Connected-provider accounts,
organizations and retained/recovery storage are rejected by this initial path.

Admission atomically records migration `0004_allocations.sql`'s operation, sealed
30-minute boot grant, original workspace generation, fresh storage incarnation,
negotiated Rust protocols and durable allocation job. Retries retain the same image,
operation, machine name and boot grant. No provider credential or raw boot token
enters the job payload or public projection. An image change for an existing
operation is rejected. The worker finds a deterministic machine name before create;
Boxd's existing name lookup/uniqueness supports recovery of an uncertain create.
After a provider ID is recorded, a missing machine never triggers another create.

Provider allocation receipts are retained even if deletion or ownership fencing
wins before attachment. Each workspace projection rechecks owner generation,
operation and live job lease. Allocation authorizes the boot grant and machine
before launch; the installed launcher starts the existing guest whose lifetime data
lock serializes repeated starts. Readiness requires the actual enrolled storage and
original launch receipt, not successful process creation. A completed allocation
purges its sealed boot token. Provider errors remain visible as status codes without
overwriting an already-online runtime or a concurrent deletion. No automatic
machine destruction runs here.

Build the API binary, explicitly apply migration 0004 to the intended database,
and keep write activation separately controlled. Operator commands are:

```sh
# Requires ZUSE_CLOUD_RUNTIME_WRITES=true and the original encryption/mint configuration.
# The workspace must already be a newly queued Rust-owned row, with a sealed launch intent.
zuse-cloud-api allocate --workspace-id <new-workspace-id> --snapshot <approved-prepared-snapshot>
# Uses server-held BOXD_API_KEY and optional BOXD_ORG; no provider credentials reach the guest.
zuse-cloud-api worker
zuse-cloud-api worker --once
```

`compose.yaml` includes the worker behind the explicit `allocation` profile. API
serving remains independently configurable and never starts a worker implicitly.
`infra/cloud/launch-runtime.sh` is installed into the guest base image. The prepared
image must supply the Rust binary, `zuse` user, prepared checkout at
`/home/zuse/workspace` and selected provider tools; repository preparation, signed
image selection and credential grant provisioning still need their inventory work.
Do not point this operator path at legacy runtime images. The Dockerfiles remain
unbuilt here because this VM has no Docker daemon.

Acceptance: a real guest process launched through the installed shell script enrolls
against Axum/PostgreSQL, streams the original sealed launch's response and publishes
its original command/chat/session identities. The provider fixture loses both create
and launch responses; recovery creates once and completes through the existing
machine. It also removes the assigned provider machine temporarily and proves no
replacement is allocated. A deletion during provider creation preserves its receipt
for cleanup without attaching or launching a guest. The provider itself remains a
local fixture; live Boxd image/runtime/grant acceptance is not claimed.

Archive, deletion and migration must carry or purge allocation records and durable
jobs. In particular, deletion must inspect allocation receipts for machines created
before attachment; `api_cloud_workspaces.provider_sandbox_id` alone is insufficient.
Recovery capture must exclude launcher boot files and reusable provider credentials.
Full pause/resume, archive/restore, billing and public creation integration remain
required before production rollout.


Final allocation-chunk verification: all five cloud fixture/unit tests and all
13 PostgreSQL/HTTP/gateway/engine/process scenarios pass (18 checks). The two
allocation scenarios passed again after the final control-flow cleanup. The API
binary builds and exposes `allocate`/`worker`; cloud/API strict Clippy, Rust
formatting, launcher shell syntax and diff checks pass. No live provider, production
database, charge export, owner transfer or Docker build was performed.
