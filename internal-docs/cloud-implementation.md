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
| B: real cloud chat | Authenticated initial creation and Boxd prepared-image allocation worker; encrypted enrollment/renewal; leased Rust RPC gateway; protected guest storage; canonical launch, encrypted text/native-command recovery, retained queue/model/Stop recovery and native steering/input and brokered Claude delivery through the existing account authority. Retained-machine pause/archive/unarchive/resume, durable native transcript publication and an engine-free cached reader with rendered archive views added; remaining: complete image selection/builds, remaining provider adapters and Rust authority migration, app captures/context delivery, remaining creation variants and UI flows. Browser attached destination menu, create/stream/cached reopen and shared guest RPC transport added; native create/stream/explicit reconnect and encrypted first-message file delivery added | Deterministic-provider and real PostgreSQL/HTTP/WebSocket/guest-process acceptance passed; live-provider acceptance pending |
| C: billing/admin | Ledger summary, usage pagination, owned/idempotent cap updates, existing Stripe/Polar portals and checkout, signed cloud subscription lifecycle processing, signed return receipts, prepaid credit purchase/funding/refund/dispute processing, owned financial/telemetry exports with meter reconciliation, completed Boxd report settlement, retained-image storage settlement, owned repository catalog connect/list/remove, shared native/browser repository picker and image readiness/build/rebuild/history panel, guest image preparation, owned account-image build/status/delete APIs, durable Boxd capture/read-back/promotion/retirement/cleanup worker and shared image deletion confirmation, and native/browser Cloud billing controls added. Existing capacity reads and compatible Claude credential delivery ported; remaining: Rust account-authority/login/refresh ownership, other provider adapters, checkout session recovery/persistent subscription writes, remaining provider usage sources, provider credentials, image lifecycle/administration, machine administration | PostgreSQL/HTTP, native Linux and browser usage/cap workflows pass; live subscription/payment acceptance pending |
| D: tools/integrations | Shared hosted RPC and scoped browser uploads added; full cloud tools acceptance, sync/device bridge/org automation pending | Guest text/PNG bytes, image reads and provider consumption pass with fixture runtime; other tool flows pending |
| E: archive lifecycle | Local reclamation; same-machine pause and ten-day retention; asleep unarchive; encrypted cached transcript views; full-state recovery helper; scoped object transport; durable verified retirement; replacement allocation, journaled restoration and fresh-generation enrollment implemented. Permanent deletion, 30-day tombstones and native/browser confirmation, progress and retry controls added. Default-tree mirror cleanup and native/browser retry controls added; remaining: complete live sync and custom-path import, archived files and existing-workspace/archive migration | Retained-machine, browser archive/delete and packaged capture/retirement/replacement-continuation acceptance pass with isolated PostgreSQL and a deterministic machine provider; live-provider and SDK continuation acceptance pending |
| F: complete GPUI parity | Attached browser/native composer destination menu, native startup tray, archived cloud transcript screens and shared Cloud repositories/image controls added; full reference comparison and remaining cloud screens pending | Compiled native/browser views and rendered browser archive/deletion workflows pass; native visual and full-screen audits pending |
| G: migration/cutover | Read-only inspection and original-format archive filesystem import added; populated runtime conversion, ownership handoff, archive migration and live cutover pending | Pending |

Do not mark a milestone complete from compilation alone. Record tested behavior and remaining gates here.

## Browser cloud launch and shared transport

Implemented in this chunk:

- Host-owned `CloudContext`, `CloudCreate`, `CloudEnvironment` and `CloudRpc`;
  access tokens and gateway grants stay outside the viewport. Catalog/history reads
  do not acquire execution transport. Concurrent tools share an account/workspace connection.
- Prepared repository selection, base branch and Claude model settings, encrypted
  first-message launch acceptance, startup progress and an actual guest chat viewport.
  The initial prompt uses the existing durable launch intent contract. The UI does
  not request `mailbox-v1`, whose contract requires a separate encrypted enqueue.
- The destination menu uses the existing attached composer toolbar, menu surfaces,
  Hugeicons and controls: This computer, scoped Cloud entries, selection radio,
  image readiness and the working Add computer route. Removed standalone Local/Cloud buttons.
- Browser closure/reload, checkpoint-backed reopen without a connection, and explicit
  reconnection to the original conversation. Full canonical reply forwarding preserves
  null values, stream acknowledgements, errors, cancellation and uncertain outcomes;
  execution actions are never automatically replayed by the broker.
- Gateway startup queues accommodate the initial subscription burst under the existing
  32 MiB global byte budget, with fair socket reads/writes. New workspaces enable
  encrypted native projections when transcript storage is configured.

Verification: compiled GPUI browser and host, isolated PostgreSQL/API, prepared-image
allocation adapter, packaged Rust guest, real Claude driver and fixture CLI/credential
issuer. The browser accepted a prompt, rendered its streamed reply, reloaded, read the
same encrypted history without `CloudEnvironment`, and explicitly reconnected without
another create or prompt execution. This uses a deterministic provider, not live Boxd
or a paid model. Receipts: `.context/cloud-launch-browser-acceptance.json`;
`.context/cloud-chat-acceptance.log`. UI: `.context/cloud-composer-destination-menu.png`.

Remaining before Milestone B/live rollout: remaining native desktop routing and cached reopen,
app captures/context, complete provider/model catalogs, queued startup messages,
reload-persistent uncertain-create retry state, bounded handling of repeated immediately
closed connections and retry controls, full startup/reconnect trays, pause/resume UI,
and live-provider acceptance. C–G and the remaining archive gates stay open in the ledger.


### First-message cloud files

Native and browser composers can attach files before starting a cloud workspace.
They reuse local staging and send committed upload identities through the host
controller. The host pins the admitting personal account, uploads encrypted assets,
then submits the original idempotent workspace request. Accepted host work survives
client cancellation. Native attachment launch waits for the resolved profile store;
account-only onboarding remains available before engine assembly.

The additive `0008_launch_assets.sql` migration records an opaque future workspace
identity before any R2 write. Files retain the original eight-file / 20 MiB limits
and supported MIME types. Immutable encrypted chunks are read back and hashed before
publishing the manifest. Private filenames stay encrypted in PostgreSQL and R2.
Create verifies the committed references and the prepared image's attested
`rustLaunchAssetsVersion: 1` capability before allocation. Older prepared images
continue to support text launches; attachment launches request a rebuild.

The packaged guest authenticates runtime-only downloads, verifies metadata and bytes,
installs without overwriting changed destinations, and admits the original command
with both prompt references and provider attachment paths. Existing original-format
v1 asset objects remain readable, including large encrypted envelopes. Launch titles
use the existing plain attachment labels rather than displaying chip markup.

Unaccepted uploads expire after 24 hours. The allocation worker purges a bounded
page under the same account lock as create; accepted workspace assets remain intact.
A database failure after object upload leaves a durable reservation for retry and
cleanup. Permanent workspace deletion removes these assets and staging metadata
through the existing workspace purge lifecycle.

Verified:

- PostgreSQL/HTTP: encrypted round trip, no allocation before create, identical and
  conflicting retries, incomplete/corrupt files, runtime ownership fencing, original
  Node-produced v1 compatibility, capability gating, admission identity, database
  failure after object upload and scoped expiry cleanup.
  `.context/cloud-launch-assets-postgres-final.log`.
- Browser: pasted text + PNG, first-turn provider consumption of the original text
  and identical inline image bytes, streamed reply, client reload, cached reopening
  without compute and reconnect with exactly one provider turn.
  `.context/cloud-launch-assets-browser-api.log`,
  `.context/cloud-launch-attachments-acceptance.json`,
  `.context/cloud-launch-browser-acceptance.json`.
- Native Linux: clipboard image through the attached composer, original bytes on the
  guest, real Claude driver image assertion and a completed journal with one turn.
  `.context/cloud-launch-assets-native-acceptance.json`,
  `.context/cloud-launch-assets-native-api.log`.
- Native/browser builds and focused import preservation and existing create/capacity
  checks passed. `.context/cloud-launch-assets-import-test.log`,
  `.context/cloud-launch-assets-creation-regression.log`.

Screenshots: `.context/cloud-launch-assets-browser.png`,
`.context/cloud-launch-assets-native-composer.png`, `.context/cloud-launch-assets-native.png`.
These checks use a deterministic machine/provider fixture. Live provider and Mac
acceptance remain open. App captures/context bundles, mailbox attachment dependencies,
reload-persistent uncertain-create recovery and the remaining Milestone B gates remain
open; no full milestone is completed by this chunk.


### Connected-cloud attachments

Browser paste and the existing file picker now route chunks through the selected
workspace's shared `CloudRpc` connection. They reuse the guest upload store, size
limit, file commit and attachment read contracts. Results retain the admitting
workspace, viewport selection, connection epoch and composer generation; navigation
cancels pending callbacks and late results cannot enter another composer. No additional
host or gateway socket is opened for cloud uploads.

Verified with the compiled browser, PostgreSQL/API and packaged guest: text and PNG
bytes landed under the guest profile, the transcript preview read the guest image,
and an explicit second turn reached the real Claude driver with the text file and
identical inline PNG bytes. Only the two submitted turns executed. Receipt:
`.context/cloud-upload-browser-acceptance.json`; full run:
`.context/cloud-image-acceptance.log`; screenshot: `.context/cloud-upload-conversation.png`.
The native GPUI admission test covers foreign workspace/selection/connection rejection,
cancellation and removal of browser routing metadata before delivery to the child view.

First-launch file delivery is covered above. Cached/archived image projections,
native desktop file selection and recovery of the parent host transport remain open. This completes
connected-browser upload delivery, not all attachment or workspace-tool parity.


### Native desktop cloud launch

The native composer extends its existing device popover with prepared cloud
repositories, readiness, recent-workspace Connect actions and Add computer.
Cloud startup uses the existing attached tray. Launch retains its idempotency key
and accepted workspace for retry; the native permission selection maps to the
runtime mode. Read-only launch is refused explicitly because the guest contract
does not implement it yet. First-launch file delivery is covered above.

A cloud window uses the existing native conversation, files, Git and terminal
controllers through the root host's shared `CloudEnvironment`/`CloudRpc` broker.
Account controls stay on the root authority. The tunnel retains canonical stream
frames and cancellation, checks the original account and connection epoch, and
uses the existing native reconnect supervisor without replaying mutations. Initial
session selection waits for the guest chat list. Window drafts are scoped to the
account/workspace; credentials remain with the root host.

Verified on native Linux: the existing device dropdown loaded the prepared
repository, startup rendered in the attached composer tray, and the guest's original
conversation appeared. Closing its window and using the refreshed Connect row
reopened the same history. The real Axum/PostgreSQL/guest acceptance passed with
one allocation and exactly one provider turn. The provider is deterministic; this
is not live-provider or Mac acceptance. A native guest terminal opened and showed
the checkout prompt; keyboard input was not verified in the headless X11 run.
Fresh profiles can reach the composer through the existing Start without a project
action, now also available on the initial onboarding screen.

The transport test passed: workspace-scoped forwarding retains null stream items,
account controls use the root authority, and account changes cancel access. Full
desktop build passed in the existing Ubuntu environment; Amazon Linux's older
libc/C++ libraries cannot link the packaged voice dependency. Use that Ubuntu
build environment for native execution here.

Receipts: `.context/native-cloud-ui-acceptance.json`,
`.context/native-cloud-acceptance.log`, `.context/native-cloud-ubuntu-scope-tests.log`
and `.context/native-cloud-entry-build.log`. Screenshots:
`.context/native-cloud-destination-menu.png`,
`.context/native-cloud-launch-progress.png`, `.context/native-cloud-reopened.png`.

This does not complete Milestone B or F: cached native reopen, integrated
sidebar/window routing, full model/branch controls, app captures/context,
startup queues and remaining administration remain open.



## Cloud billing usage and cap administration

Implemented the pinned reference `/v1/cloud/billing/summary`, `/usage` and `/cap`
contracts with shared Rust wire types and host-authority RPC. Reads use one
repeatable-read snapshot and existing periods, ledger entries, live reservations,
storage usage and reconciliation receipts. They neither initialize billing periods
nor export charges. Summary math retains the ledger's settled overage amount and
adds reservation charges; it does not reprice corrected historical usage. Snapshot
usage retains the legacy `other` plus `snapshot-storage` public classification.
Usage cursors retain reference entry-ID ordering and bounded page sizes.

Cap changes validate exact safe integer USD micros, reject a limit below incurred
overage, lock the current period, and reuse the original cap-command table for
idempotency. Repeating an old request returns current state without rewinding a
newer cap. Conflicting concurrent retries cannot both apply. No Stripe/Polar
customer or subscription identity, ledger entry or invoice is rewritten.

Writes are disabled by default. `ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS` creates a
separate writer pool for listed account owners; the catalog pool stays read-only.
Each write additionally requires the existing `zuse_rust_control_owners` row with
`resource_kind='billing'`, `resource_id` equal to the owner, and `backend='rust'`.
It holds that row's shared lock until commit. Workspace lifecycle ownership does
not confer billing ownership. Deployment must fence/forward the old billing writer
before transferring this marker; the migration/cutover work remains open.

Native and browser Settings → Cloud now show allowance, invoice estimate, storage,
provisional usage, cap editing, refresh and paginated history using existing settings
components. Requests go through the host authority, including from a cloud window.
Account/organization changes clear state and cancel pending UI work; returned scope
must match the admitted scope. Cap retries retain their idempotency key until the
amount changes or the server confirms success. Monetary input uses exact decimal
conversion rather than floating-point arithmetic. Errors and success are shown
above the controls, before the history list. Billing never acquires a workspace
execution connection.

Verified:

- PostgreSQL/HTTP: personal isolation, organization billing/admin authorization,
  reservations and expiry, storage classification, pagination, settled-charge
  preservation, cap validation, concurrent conflicting retries and unchanged
  subscription IDs/ledger. `.context/cloud-billing-postgres.log`.
- Native Linux: real summary, edit $25 to $31, rejected below-incurred cap,
  visible feedback and another history page. Receipt:
  `.context/cloud-billing-native-acceptance.json`; image:
  `.context/cloud-billing-native-saved.png`.
- Compiled browser → host → API → PostgreSQL: paginated usage, cap $27 then $31,
  below-incurred rejection and saved data after reload. The observed request trace
  contains no `CloudEnvironment`. Receipts:
  `.context/cloud-billing-browser-acceptance.json`,
  `.context/cloud-billing-browser-wire-proof.json`; image:
  `.context/cloud-billing-browser-saved.png`. Integrated fixture:
  `.context/cloud-billing-ui-api.log`; driver:
  `scripts/acceptance/cloud-billing.mjs` with the explicit billing UI fixture env.
- Native build and compiled wasm screens passed; exact decimal cap conversion
  passes `.context/cloud-billing-money-tests.log`.

This closes the usage/cap workflow only. Period creation/renewal, checkout/portal,
subscription/payment-failure processing, prepaid credits, provider settlement,
webhook deduplication, metered exports and the rest of cloud administration still
require implementation. Live Stripe/Polar, original-account and Mac acceptance are
not established by these fixture tests. No production writes were enabled.


## Current service

`apps/cloud` runs the hosted API; `crates/cloud` owns authentication, authorization,
catalog reads and durable job primitives. Shared archive contracts live in
`zuse_protocol::cloud`. No new history database or alternate local engine is introduced.

The API defaults to **read-only**, including PostgreSQL session enforcement for
catalog reads. Explicitly enabled runtime control uses a separate writable pool and
requires a matching Rust workspace owner/generation in each transaction. Catalog reads never
wake machines. The explicitly configured worker allocates or resumes compute only
for durable admitted intent. It does not transfer ownership or export charges.
It serves only implemented routes:

- `GET /healthz`, `GET /readyz`.
- `GET /v1/cloud/context` (new: authenticated actor, selected scope and owner).
- `GET /v1/billing/entitlements` (existing persisted entitlements, without checkout repair).
- `GET /v1/cloud/workspaces[?projectId=...]`, `GET /v1/cloud/workspaces/:id`.
- `GET /v1/cloud/workspaces/:id/archive-status` (new: retained machine, deadline,
  backup/retirement/restore phase and verification progress; no compute activation).

With `ZUSE_CLOUD_RUNTIME_WRITES=true`, these routes are additionally registered:

- `POST /v1/cloud/workspaces` is additionally registered only for an explicit
  `ZUSE_CLOUD_CREATE_ACCOUNTS` allowlist; see the creation gates below.
- `POST /v1/cloud/workspaces/:id/runtime/bootstrap`.
- `POST /v1/cloud/workspaces/:id/runtime/bootstrap/ack`.
- `POST /v1/cloud/workspaces/:id/runtime/credentials/renew`.
- `POST /v1/cloud/workspaces/:id/runtime/providers/:provider/grant` (original
  account-authority ciphertext, including the Codex compatibility envelope).
- `POST /v1/cloud/workspaces/:id/runtime/storage/enroll` (prepare/commit activation barrier).
- `POST /v1/cloud/workspaces/:id/{pause,archive,unarchive,resume}` (original action body/public workspace response).
- `POST /v1/cloud/workspaces/:id/delete` (explicit deletion, durable cleanup, original action body and reconcile header).
- `POST /v1/cloud/workspaces/:id/runtime/lifecycle` (scoped directive and quiescence/resume receipts).
- `POST /v1/cloud/workspaces/:id/ready` (`agent-started` receipt for Rust guests).
- `POST /v1/cloud/workspaces/:id/gateway/ticket`.
- `GET /v1/cloud/workspaces/:id/gateway` (authenticated WebSocket upgrade).
- `POST /v1/cloud/workspaces/:id/commands`, `GET .../commands/:commandId`,
  `DELETE .../commands/:commandId`, `GET .../commands/watch?afterRevision=...`.
- `GET /v1/cloud/workspaces/:id/data-key` (authorized editor only).
- `GET/PUT /v1/cloud/workspaces/:id/archive/:operation/:object` (original operation-scoped capabilities, immutable ciphertext; no user bearer fallback).
- `POST /v1/cloud/workspaces/:id/runtime/transcript-checkpoint` and `.../transcript-message-page` (encrypted projection publication; requires R2 configuration).
- `GET /v1/cloud/workspaces/:id/sessions/:sessionId/transcript-checkpoint` and `.../transcript-message-page` (registered when transcript storage/key configuration is present; available in read-only mode).
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
| `ZUSE_CLOUD_LEGACY_AUTH_URL` | Optional exact HTTPS origin of the existing account authority API; no default, credentials are never redirected |
| `CLOUD_WORKSPACE_RECOVERY_ENABLED` | Defaults false; admits expired enrolled Rust archives only when explicitly true |
| `CLOUD_WORKSPACE_RECOVERY_WORKSPACE_IDS` | Optional comma-separated recovery admission allowlist |
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
`0005_lifecycle.sql` records retained-machine lifecycle operations with immutable
source machine, runtime generation and storage incarnation. Runtime-control
readiness requires this migration; startup still never applies migrations.
Apply them explicitly to an isolated test database first. Jobs require a matching
Rust owner and generation before claim; enqueue belongs to the same transaction as
the eventual lifecycle intent. Reconcilers must additionally fence provider effects
and acknowledge results in the resource transaction before production activation.

Archive expiry is ten days and never implies history deletion. Legacy deadlines
are preserved or shortened to that window. A verified manifest/chunk count and a
persisted retirement phase are required to report backup verification. This status
reader alone cannot authorize machine deletion. Full backup/restore helpers,
privileged verification, signed capabilities and source-machine fencing are implemented
for enrolled Rust guests. Verified replacement enrollment is implemented; explicit-delete
purge and existing-workspace migration remain pending. Recovery stays disabled until provider acceptance passes.

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


### Authenticated initial workspace creation

`POST /v1/cloud/workspaces` now uses the original shared
`CloudWorkspaceCreateRequest` Rust contract, WorkOS personal-account authorization,
original usable-entitlement policy and current billing capacity. Set
`ZUSE_CLOUD_CREATE_ACCOUNTS` to an explicit comma-separated account allowlist, with
runtime writes separately enabled, to register the route. The default is disabled.
Do not enable this for customers before provider-grant, image and billing settlement
acceptance. Organization, connected-provider, custom-layout/fork, local-device,
secret-binding and unsupported permission-mode variants still need their ports;
requests requiring those capabilities are rejected before admission.

The transaction preserves account/subscription IDs and existing billing ownership.
It reads the current period, provider-cost and settled-charge ledger totals, and
live reservations; it never creates periods, changes payment providers, emits a
charge or exports usage. Ended/grace/held periods and exhausted caps block admission.
The same capacity gate runs before initial provider allocation/launch, so a hold
arriving after acceptance blocks those effects. Already-online enrollment can still
complete its receipt cleanup. Settlement, allowance reservations/refresh, payment
webhooks, period repair and runtime billing enforcement remain required in C.

Creation requires an owned ready project and the latest retained ready Boxd build.
The prepared build must explicitly attest these settings:

```json
{
  "runtimeRpcProtocol": "zuse-rpc-v1",
  "rustRuntimeStorageFormat": "zuse-rust-engine-v1",
  "rustSessionCommandVersion": 1,
  "rustRuntimeCheckoutProjectId": "<selected-project-id>"
}
```

These describe the initial prepared single-project checkout contract; account-image
cache selection, build/rebuild publication, custom snapshot layouts, sizing and fresh
remote-ref fetching remain in the image/repository milestones. Legacy images are
never guessed to speak Rust RPC. The image must have the selected repository at
`/home/zuse/workspace` with the requested base ref available. Project/request secrets
cannot be silently omitted; those flows remain gated until delivery is ported.

One transaction commits the new workspace and its fresh Rust owner, original-format
wrapped transcript key, sealed initial launch intent, allocation operation and
durable job. The account/idempotency key binds a keyed canonical request digest.
Concurrent retries retain the original workspace/chat/session/command/turn IDs;
changed requests reject a reused key. Active and archived branches stay reserved,
and conflicts return `cloud_branch_in_use`. Only new resources receive ownership;
this endpoint does not migrate or replace existing workspaces. Responses preserve
201/200 semantics, the workspace/chat/session response shape, reconciliation header
and the optional `mailbox-v1` initial delivery contract.

The authority supplies a repository checkout plan in Rust bootstrap. Under the
existing lifetime data lock, the guest journals the base commit before first branch
selection and writes its completion marker before admitting the original launch.
Reopen with a completed marker preserves advanced branches, tracked modifications
and untracked files. Occupied branches, dirty initial checkouts and mismatched
journal identities fail without resetting files or refs. Retained runtime storage
without a corresponding repository journal requires verified import. Recovery
capture/restore must preserve `rust-repository-ready.json` with the canonical data.

Creation acceptance covers authenticated HTTP, concurrent retries, request/branch
conflicts, private-project isolation, existing ledger/reservation cap enforcement,
expired reservations, post-admission billing holds, entitlement expiry and complete
rollback when allocation enqueue fails. Original subscription IDs and ledger rows
are unchanged. Allocation acceptance now starts a real guest through its requested
Git branch before streaming the original chat. Separate checkout tests preserve
advanced commits and working bytes across reopen and refuse occupied branches.
Live native-provider turns and production rollout remain unverified.


Final creation-chunk verification: 27 local checks pass (six cloud fixture/unit
checks, 14 PostgreSQL/HTTP/gateway/engine/process scenarios and seven guest
crypto/storage/repository checks). The rebuilt API and guest pass the shared
creation contract; the final PostgreSQL suite passed again after persisting the
original permission-mode metadata. Rust formatting and scoped cloud/API/runtime/
guest strict Clippy pass. Production creation stays disabled by default. No billing
owner transfer, payment/export write, live-provider turn or full milestone completion
is claimed. The next substantial backend work remains provider grants, images and
complete billing/lifecycle integration before customer activation.


### Account-authority grants and native Claude delivery

The runtime grant route binds requests to the current Rust workspace owner,
generation, enrolled storage, credential hash/expiry and exact RSA thumbprint.
It supports the original typed provider/Codex grant envelopes and rechecks runtime
and authority incarnation/epoch after issuance. A delayed result cannot pass a
concurrent delete, credential revocation, authority rotation or writer handoff.
Reusable credentials and refresh tokens never enter mailbox commands or chat state.

For **existing legacy-owned account authorities**, the optional
`ZUSE_CLOUD_LEGACY_AUTH_URL` compatibility adapter delegates to their existing
serialized executor. It preserves the existing account, provider login, authority
storage, cache and sole refresh writer. The transport pins an HTTPS origin, refuses
the Rust API's own origin, never follows redirects, bounds responses and returns
only known error categories. It forwards only the enrolled workspace bearer and
public recipient request, never a user session or administrative provider key.
Missing authority ownership is legacy; an explicitly Rust-owned authority is
rejected rather than silently delegated back. This is a migration adapter, **not
completion of the Rust account-authority rewrite**. Native authority provisioning,
configuration/login/logout, refresh/import and ownership transfer remain required
in C/G before the legacy service can be retired. Old schedulers must still be fenced
before any production Rust workspace activation.

The guest opens the original RSA-OAEP/SHA-256 wrapped AES-256-GCM grant, authenticating
the original ordered AAD and binding plaintext to account, workspace, generation,
provider, key and authority incarnation/epoch. Grants need at least one minute of
remaining validity. The broker coalesces concurrent resolves, retains request identity
on transport failure and refreshes its in-memory cache on demand within the original
five-minute margin. Runtime bearer renewal reaches it through the shared credential
watch channel. It does not store plaintext grants or refresh credentials on disk.

The native Claude driver now accepts a host credential resolver for both discovery
and runs. Credential waits in a run honor Stop before spawning a child; a late
successful grant cannot start a cancelled prompt. Secrets go directly into that child command's environment; serialized
RunRequest/session data remain unchanged. Brokered children clear conflicting
inherited Anthropic/OAuth credentials first. Local drivers without a resolver keep
their existing behavior. A prepared Rust build declaring
`providerAuthDeliveryVersion: 1` selects this broker path; creation requires a
configured issuer and currently accepts the Claude adapter only. The brokered guest
registers only supported drivers, preventing an unsupported provider from falling
back to an unrelated local login. Custom endpoints and Codex/Cursor/Grok runtime
adapters, including external-token refresh callbacks, remain unfinished and gated.

Acceptance launches the actual native Claude Rust driver with an explicit fixture
CLI, through Axum/PostgreSQL and an independent Node authority envelope. Both run
and discovery receive only the expected credential; stale inherited accounts are
removed. The executed turn survives guest reopen without replay, and plaintext
credentials are absent from runtime/checkout files, including SQLite/WAL/history.
Acceptance also holds a grant response, stops startup, releases the late response
and proves no cancelled prompt reaches the provider. A later explicit run succeeds.
Deletion during issuance suppresses ciphertext and fences the guest. Independent
Node-generated fixtures prove scope/generation/expiry rejection and authenticated
metadata/ciphertext tampering rejection. A real HTTP transport fixture verifies
request preservation, refusal to redirect the bearer and sanitization of upstream
errors. No live account authority or model-provider turn was exercised.

Verification: 30 local cases pass (seven cloud fixture/unit checks, 15 PostgreSQL/
HTTP/gateway/engine/process scenarios and eight guest crypto/storage/repository
checks). API/guest builds, scoped strict Clippy, Rust formatting and diff checks
pass. The creation allowlist and authority delegation remain off by default.
Complete billing, image preparation, archive/restore/delete, UI parity and live
migration acceptance remain required by the milestone ledger.


### Retained-machine archive and pause/resume

`runtime/lifecycle.rs` implements the original action routes for Rust-owned personal
Boxd workspaces with committed storage and an enrolled
`rust-workspace-lifecycle-v1` capability. Admission records a durable operation,
command identity and job in one transaction. Reusing a command for a different
action conflicts. Source machine, runtime generation and storage incarnation are
fixed; lifecycle never enters initial allocation or creates replacement compute.

Pause/archive fence gateway attachment and renewal and stop mailbox leasing.
The fenced guest persists held queues for the root thread and its child/tab
conversations, marks archived family metadata, and retires the engine. Final
fallible snapshots cover retained conversation handles and workspace registry
before the authenticated quiescence receipt. The worker cannot pause compute
before that receipt. The explicit worker polls provider state, pauses only the
recorded machine and reports archived/paused only after the provider confirms
paused or stopped. Provider failure retains the source and records a retryable
error. Missing quiescence produces `cloud_runtime_quiesce_failed` and retains the
machine; old runtimes are rejected before admission.

Archive records a fresh ten-day retention deadline; re-archive while asleep starts
a new window. Expiry has no destruction path. No recovery flag is enabled by this
implementation. Unarchive clears the archive timestamps and returns Active with
`state=paused, desiredState=paused`; it creates no provider job and never wakes
compute. Explicit interactive Resume checks the existing entitlement and billing
capacity before admission and again before provider effects. The worker wakes or
starts the same source machine, launches with retained identity and no fresh boot
token, and awaits the original family's validated guest receipt before declaring
it online. Held queues require explicit user continuation.

Archive status exposes the persisted operation ID and `lifecyclePhase` independently
of recovery archive phase and verification counts. Retention never implies that an
archive backup was verified or compute retired. Reads do not enqueue work.

Remaining: encrypted cached archived transcript/file/diff projections; automatic
interactive mailbox wake; stopped/uncertain command fencing during runtime
replacement; durable quiescence-receipt replay across a guest crash; recovery
capture and verification; retirement barrier, replacement-machine restore,
permanent deletion, original-archive migration and usage settlement for retained
resources. Live Boxd hibernate/wake/launcher acceptance remains external; this
chunk does not claim those full archive deliverables or UI acceptance.


Verified this chunk with 31 passing checks: seven cloud unit tests, sixteen
isolated PostgreSQL/HTTP/WebSocket/native-guest scenarios and eight runtime
unit/identity checks. The new archive scenario exercises a live PTY, capability
rejection, account isolation, command replay/conflict, pause failure and explicit
retry, ten-day expiry without retirement, asleep unarchive/re-archive, billing
hold, same-machine resume, retained dirty/`.env` bytes, original keys and continued
conversation history. Both service binaries build; strict scoped lint, formatting
and shell syntax checks pass. A launcher smoke check confirms retained identity
is required and no fresh boot token or administrative provider key reaches the
guest. Live provider, recovery, browser and native desktop UI acceptance remain
pending. No UI changed in this chunk, so no new screenshots are claimed.


### Encrypted cached transcript API and R2

The original transcript-checkpoint/message-page upload and read routes are ported.
They preserve `api_cloud_transcript_checkpoints`, original object keys, AES-GCM/gzip
ciphertext envelopes, wrapped transcript keys and public response shapes. Readers
accept legacy workspace ownership and work in read-only service mode. They check
workspace sharing, hide deleting/deleted data, verify head byte counts/digests and
recheck authorization after object I/O before returning the original transcript
key. Matching epoch/version returns null without an object download; absent old
pages return null. These reads never create a job, refresh workspace activity,
allocate compute or use the live gateway.

Runtime publication requires current Rust ownership, enrolled storage and the
current unexpired runtime bearer. The final quiescing runtime can publish while
archive is requested; deletion fences reject publication. Objects commit before
metadata, and immutable collisions reject differing bytes. Head advancement uses
the original generation/epoch/version ordering; an older or duplicate cursor
cannot rewind the current pointer. Failed storage writes leave metadata intact.
PostgreSQL stores pointers and ciphertext metadata, never transcript plaintext.

Configure all four variables, using a bucket scoped to existing transcript data:

| Variable | Purpose |
| --- | --- |
| `CLOUD_TRANSCRIPT_R2_ENDPOINT` | HTTPS S3 origin, normally `https://<account-id>.r2.cloudflarestorage.com` |
| `CLOUD_TRANSCRIPT_R2_BUCKET` | Existing private transcript bucket |
| `CLOUD_TRANSCRIPT_R2_ACCESS_KEY_ID` | Scoped S3 access key identifier |
| `CLOUD_TRANSCRIPT_R2_SECRET_ACCESS_KEY` | Scoped S3 secret, kept server-side |

The existing `CLOUD_DATA_ENCRYPTION_KEY` (or legacy vault-key fallback) is also
required to read original wrapped keys. Partial R2 configuration fails startup;
enabled readers/writers require the existing checkpoint table in readiness.
No schema is created or ownership transferred on startup. No public object URLs
are issued. R2 requests use signed bounded HTTPS with redirects disabled and
conditional immutable PUT; conflicting existing bytes never overwrite objects.
The transport follows the [S3 signing contract](https://docs.aws.amazon.com/AmazonS3/latest/developerguide/sig-v4-header-based-auth.html)
and [R2 conditional-write contract](https://developers.cloudflare.com/r2/api/s3/api/).

Focused acceptance verifies independent SDK signing vectors, an actual HTTP
object adapter, and isolated PostgreSQL/HTTP projection publication/readback.
Synthetic Node AES-GCM/gzip fixtures preserve original message content and older
pages, and the independent reader decrypts both API responses. The archived read
scenario removes Rust writer ownership and runtime control, then verifies
workspace rows and the durable job count stay unchanged.

The native guest publisher and cached read transport are implemented below.
Remaining: rendered client hydration, full chat catalog/change feeds, archived
file/diff projections and GPUI/browser read surfaces. Transcript checkpoints do
not authorize machine retirement; full verified recovery archives remain a
separate deliverable. Live R2 bucket acceptance is still pending.


Verification for this port: 26 passing checks (nine cloud unit tests and seventeen
isolated PostgreSQL/HTTP/WebSocket/native guest scenarios), strict scoped lint,
API build, formatting and diff checks. The original guest binary is unchanged.
The existing cloud-chat, grants, mailbox, queue and retained-machine archive
scenarios continue to pass. Live bucket reads/writes and cached UI screenshots
are not claimed by these fixture/HTTP checks.


### Native guest publication and cached history transport

`zuse-cloud-projection` shares the native transcript frame, historical cutoffs,
chat/model context, queued-message display state and paged history between the
guest and an engine-free reader exported by `zuse-client::CloudHistoryReader`.
Native payloads explicitly negotiate `zuse-rust-transcript-v1`; their immutable
objects use a separate path inside the existing workspace prefix. Unnegotiated
reads return a protocol mismatch. Original encrypted checkpoint/page formats and
paths remain supported. Native cursor conflicts fail rather than silently
acknowledging a stale upload.

Before initial runtime enrollment, opt in a supported workspace with
`request_config.rustNativeTranscriptEnabled=true` and configured transcript
storage. Bootstrap then enables the guest publisher. Archive additionally
requires the enrolled `rust-transcript-projection-v1` capability. Leave this off
for legacy clients/workspaces until their consumer and populated-workspace
migration are ready; toggling the flag on a cached bootstrap is not an upgrade.

The publisher samples changed documents and journals encrypted upload batches in
the original runtime SQLite database with FULL commits. It persists epoch/version,
content digest, exact ciphertext and upload position before network delivery.
Retries and guest reopen reuse those bytes. Final archive publication occurs
after agent/terminal retirement and checked snapshot flush, before the quiescence
receipt permits provider pause. Storage failure retains the outbox and source
machine. Read snapshots never replay commands or initialize writable engine
storage. Their chat flags are historical context: control-plane lifecycle owns
Active/Archived placement and execution availability.

Verification: 36 checks pass across the codec, cloud API/PostgreSQL/native guest,
and runtime identity/storage suites. The guest scenario covers failed storage,
a lost committed upload response, client closure/restart with identical encrypted
bytes, independent conversation tabs, final archive publication, independent Node
decryption, archived reads while compute stays asleep, and same-machine resume.
Codec paging preserves all 150 fixture entries and queue context. Guest/API builds,
strict scoped lint, client compilation and formatting pass.

Remaining: archived files/attachments/
sidecars/diffs; durable quiescence-receipt recovery; full verified machine backup,
retirement, replacement restore and deletion; legacy cursor/import migration and
live provider/R2 acceptance. This is a working native projection path, not full
archive or UI completion.

### Rendered archived cloud history

The host now brokers `CloudWorkspaces`, `CloudHistory` and `CloudUnarchive`
through its existing sign-in authority. Access tokens remain inside the host;
viewports receive account-scoped workspace metadata and decrypted saved history.
Local profiles do not send their synthetic identity to the production cloud API.
Only explicitly negotiated Rust workspaces are offered by this first UI path.

Native Settings → Archived and browser Settings → Archived chats use the shared
archive model. Opening a saved conversation hydrates the existing transcript
renderer in separate state without an execution connection. Native child state
has no engine; browser child state has no live transport and rejects RPC calls.
Unarchive uses the original durable lifecycle action with a stable command ID;
it returns the workspace to Active while compute remains paused. Loading,
archiving, failures and retry are exposed. Sign-in changes clear cached views and
invalidate pending responses. Browser connection loss does not replay Unarchive.

This is not full archive/UI parity. Remaining work includes conversation tab
selection, cached archived files/diffs/attachments, the full active cloud catalog
and composer workflow, organization selection, legacy history import/adapters,
verified backup/retirement/replacement restore/permanent deletion, and complete
reference screen comparison. Standalone CLI WorkOS onboarding is still pending;
production browsers need an existing WorkOS-authenticated host.

Verified in the actual compiled GPUI browser against the isolated PostgreSQL/API
and guest-process fixture: archive publication and guest quiescence completed;
opening saved history preserved the original workspace/session and held queue;
the real Unarchive action returned that same workspace to `paused`, without
provider wake/launch or automatic execution RPCs. The fixture uses encrypted
in-memory object storage and a deterministic provider/model, not live R2/provider
acceptance. Source-machine retention and original identities are checked by the
PostgreSQL fixture after the browser signals completion.

Focused checks passed: two host cloud-origin/development-identity guards, two
archive reducer tests, the protocol read-versus-mutation retry contract, two
ciphertext/paging tests, and a native GPUI test proving a cached viewport cannot
dispatch execution even if marked connected. Native UI compilation and the
complete browser WASM build also passed. The browser and guest acceptance receipt is
`.context/cloud-archive-browser-acceptance.json`. Rendered images were inspected:
`.context/cloud-archived-before.png`, `.context/cloud-archived-list.png` and
`.context/cloud-archived-history.png`. No native Mac visual acceptance or live
provider acceptance is claimed.

To repeat the browser acceptance, run the existing ignored
`runtime_engine::rust_cloud_archive_quiesces_retains_and_lazily_resumes_original_machine`
PostgreSQL test with `--features runtime-acceptance` and an explicit isolated
`ZUSE_CLOUD_TEST_DATABASE_URL`, built `ZUSE_CLOUD_TEST_GUEST_BIN`, and absolute
`ZUSE_CLOUD_UI_FIXTURE_FILE` ready-file path. When that file appears, run
`node scripts/acceptance/cloud-archive.mjs` with the same ready-file variable and
current `dist/web`. The browser drives the real host, saves screenshots/receipt,
and writes the corresponding `.done` file; the fixture verifies paused compute
before removing its isolated schema. Keep this test identity and fixture loopback
only. Normal acceptance does not set the UI fixture variable.


### Packaged full-state recovery helper

`zuse-cloud-runtime recover` now implements the existing helper environment
contract in Rust. It reads `ZUSE_USER_DATA`/`--data-dir` and a private
`ZUSE_ARCHIVE_CONFIG` containing the original operation/workspace/chat/session,
home/repository roots, scoped object URL/capability, archive key, expiry and
`backup` or `restore` mode. Rust data additionally pins `rustStorage.database`
(relative to the data root), `accountId` and `storageIncarnationId`; absent
`rustStorage` selects the original `data/zuse.sqlite` adapter. Neither mode starts
an engine, creates a database, enrolls a runtime, wakes compute or deletes a
machine. CLI success distinguishes `workspace_archive_uploaded` from
`workspace_archive_restored`.

The helper takes the shared runtime lock (including the verified inherited
launcher descriptor) and existing engine instance lock. Capture includes runtime
data, WAL/SHM, repositories/worktrees, Git refs, tracked/deleted/untracked files,
ignored user files, attachments, engine journals and supported Claude/Codex
continuation roots. It checks source metadata for mutation and validates an
extracted copy before publication. Rust validation checks full SQLite integrity,
the original workspace/account/storage identity, registry chat IDs and session
document identity. Legacy validation checks the original chat/session tables and
supported native providers. Unsupported native-agent layouts fail without
publishing a backup.

Reusable login/provider credentials, model vaults, old runtime credentials and
enrollment marker, GitHub broker credentials, account logins, pairing keys,
managed-device identities, relay settings and browser access tokens are excluded
at their device/profile locations. Public device IDs and durable conversation
state remain. Codex capture is restricted to sessions, archived sessions, session
index and native state databases/sidecars. Internal recovery journals are not
recaptured. External symlinks and unsupported entries fail; archive extraction
rejects traversal, duplicate paths, non-directory roots, hardlinks and symlink
parents before installation.

The transport preserves the original version-one manifest, gzip/tar root names,
4 MiB plaintext chunk size, standard-base64 AES-256-GCM envelope and
workspace/operation/chunk-order AAD. Chunks are addressed by SHA-256 of their
encoded ciphertext. A private operation journal retains exact ciphertext through
upload retry; the manifest is uploaded last. The guest's original upload-only
capability is never used for reads. `Recovery::verify` is a separate privileged
read-back operation: it rereads/authenticates every chunk, checks the full archive
digest and validates the reconstructed database. Its local verification receipt
is **not** a control-plane retirement barrier and cannot authorize deletion.

Restore authenticates the manifest and every chunk before staged extraction,
validation and journaled entry installation. It refuses an occupied runtime data
store, including another account/profile's database, before changing repositories.
Existing prepared-image entries are preserved under each root's
`.zuse-archive/<operation>/previous`. Recovery locks stay in place during install.
Interrupted installation can retry without rewinding installed files or newer
work. A completed receipt is written after validation and fsync. Failed capture,
upload, verification or restoration leaves the source/archive/partial target
available; no recovery helper path authorizes retirement.

Verified with all **20 runtime checks passing**, including the packaged binary
against a real loopback HTTP object fixture with separate upload-only and
read-only capabilities. The filesystem round trip covers a 350-message WAL
history, held queue, processed-command claim, uncertain journal, attachment,
random multi-chunk file, ignored `.env`, raw-byte filename, branch/tag refs, a
second Git worktree, tracked modifications/deletion and fresh target identity.
Tests also cover interrupted upload and install, corrupted remote reads, original
Python tar/Node ciphertext import, wrong account/storage identity, occupied target
protection, source locks and inherited launcher lock ownership. Subsequent writes
to the original restored conversation survive an idempotent retry. This is data
continuity evidence, not live provider/SDK agent continuation acceptance.

Reproduce after building the guest:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3 \
  cargo build --locked -p zuse-cloud-guest
ZUSE_CLOUD_TEST_GUEST_BIN="$PWD/target/debug/zuse-cloud-runtime" \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3 \
  cargo test --locked -p zuse-cloud-runtime --features engine -- --include-ignored
```

Remaining before production recovery enablement: complete retained and replacement-resource
billing, complete local sync/custom-path import, source-image publication and live-provider
continuation acceptance. Ordinary storage admission still refuses populated unbound
storage; the separately authorized restore-adoption path is described below. Recovery
remains default-off. Expiry alone never authorizes destruction: explicit recovery
policy and the persisted verified barrier are both required.

### Durable capture, verification and retirement

Migration `0006_recovery.sql` adds pinned recovery operations and the existing
observation/usage-outbox tables if absent. Startup never applies it. Apply it
explicitly before enabling recovery. The default-off policy admits only expired,
quiesced, broker-authenticated Boxd workspaces with the
`rust-full-state-recovery-v1` enrollment capability. Other workspaces retain their
machines. Admission pins source machine, account/chat/session, storage incarnation
and writer generation; it revokes the source runtime bearer and gateway lease
before a maintenance wake. Native retained storage keeps its enrollment generation
during capture; replacement enrollment must acquire a new authorized identity.

A dedicated worker uses durable leased jobs independently of allocation/resume.
The installed recovery launcher isolates one helper process group per operation,
keeps configuration out of argv/logs, and persists cancellation before stopping
the group. Upload grants cannot read. Original archive read grants can only read
the selected restoring operation. R2 archive transport supports the original
6 MiB ciphertext wire limit without raising transcript limits. Manifest publication
is immutable and occurs after chunk upload. The worker Compose service mounts a
private disk-backed scratch volume for full database verification and sets TMPDIR
to it; the API's small request tmpfs is insufficient for workspace extraction.
Provision that volume with capacity for reconstructed workspaces. Container builds
include the native engine's compiler/runtime dependencies. Docker image build and
provider image publication remain unverified in this VM (no Docker daemon).

After capture, the worker stops the helper and pauses the source. A separate
privileged verifier reads back all objects, authenticates/extracts the archive and
validates SQLite and original identities. It renews the job lease during that work.
Only a fresh owner/generation/operation/lease check can commit the verified digest
and retirement phase. Exact-source deletion runs on a later delivery after that
barrier is durable; uncertain deletion responses reconcile by inspecting the same
source. A second shared-machine check blocks retirement if another live workspace
references it. History, encrypted archive and data key remain after retirement.

Unarchive before the barrier revokes the verifier delivery, cancels capture and
returns to Active without starting the engine. Cleanup confirms the helper has
stopped and the retained source is paused before restoring its bearer. Unarchive
after the barrier lets retirement finish. Interactive resume now durably requests
replacement restoration. Its implementation and remaining production gates are
described below; this policy must remain off until live acceptance passes. Delayed work is fenced by deletion state; the permanent-delete backend and
tombstone implementation and Delete UI are described below. Full local sync/custom-path import
remain outstanding.

Backup running observations feed the original idempotent usage outbox as
`billable=false` telemetry. This does not transfer billing/settlement/export
ownership or implement retained-resource/replacement billing.

Isolated acceptance uses the packaged guest, real HTTP object routes and PostgreSQL
with a deterministic machine provider. It covers multi-chunk ciphertext larger
than the transcript limit, write-only capabilities, corrupt read-back retaining
the source, committed-barrier checks inside the provider deletion callback, lost
delete-response reconciliation, cached transcript after retirement and unarchive
during an in-flight verifier. The deletion-race test injects a deletion fence; it
does not claim the permanent Delete workflow exists. All **21 PostgreSQL/HTTP/
packaged-guest regression tests pass**, including the
four recovery scenarios and the existing cloud-chat, mailbox, allocation,
archive/resume and encrypted-transcript workflows. Immediate archive during guest
startup now checks durable lifecycle intent before gateway reconnect, including
when there was no connected socket to fence. Launcher acceptance covers
idempotent capture and cancellation before/during execution. Live Boxd acceptance
and source-image publication remain required.

```sh
python3 infra/cloud/tests/recovery-launcher.py
ZUSE_CLOUD_TEST_DATABASE_URL=postgres://localhost/isolated_cloud_test \
ZUSE_CLOUD_TEST_GUEST_BIN="$PWD/target/debug/zuse-cloud-runtime" \
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=3 \
  cargo test --locked -p zuse-cloud --features runtime-acceptance \
  --test postgres recovery_acceptance -- --ignored --test-threads=1
```

Additional focused checks: **9 cloud unit tests** and **20 runtime checks** pass,
including R2 signing and immutable 5 MiB archive transfers while transcript reads
and writes retain their 4 MiB limit.

### Replacement restoration and fresh runtime enrollment

Migration `0007_restore.sql` preserves the original recovery operation/generation
and records a separate replacement operation. The source remains retired before
replacement admission. An interactive Resume takes the owner lock before the
workspace row, rechecks entitlement/capacity, selects an approved native image for
the same account/project, and atomically advances the writer generation and
gateway epoch. Storage incarnation, workspace/chat/conversation identities,
encrypted mailbox envelopes and execution receipts remain unchanged. Neither
Unarchive nor cached history reads allocate replacement compute.

Demand arriving during committed retirement is persisted for the recovery worker.
Retirement commits independently before replacement admission; an unavailable
image or billing hold cannot roll back its recorded result. A deterministic
replacement name reconciles uncertain create responses. The replacement helper
gets only a read-scoped original archive capability and restores through the
existing authenticated, staged installation journal. Failed or uncertain capture,
allocation, installation or launch keeps the verified archive and recoverable
target; the worker never substitutes an empty runtime.

The helper writes a data-local adoption receipt only after installed database and
original identity validation. That receipt is excluded from subsequent captures.
The authority then grants a fresh bootstrap bound to the verified digest, original
storage binding, new generation and exact replacement machine. Storage admission
requires both receipts to agree, revalidates the installed database, and writes a
new enrollment marker before the engine opens. A restored populated profile
without that authority grant remains refused. Old runtime/account credentials are
not imported; normal broker grants supply fresh provider credentials.

The replacement activates the original archived conversation family and reports a
normal authenticated Resume receipt. Only online, committed storage enrollment and
that matching receipt finish the restore. Original engine journals, command
admission receipts and mailbox lease fencing prevent accepted work from being
reissued as a new turn. Replacement running observations join the existing usage
outbox as nonbillable telemetry, without transferring settlement/export ownership.

Isolated acceptance covers both demand-after-retirement and demand-during-retirement,
with lost create/install/launch responses in each path. It allocates exactly one
replacement, restores the original `.env`, enrolls generation 3 against the original
storage identity and continues the original conversation through the durable
mailbox. A fresh live transcript reset proves one original prompt, one continued
prompt and their two replies. Repeated Unarchive preserves accepted restoration.
Runtime checks additionally reject missing grants, wrong archive digest, machine,
generation and enrollment identities before creating a storage marker.

Production recovery is still default-off. Live Boxd image/build acceptance,
supported agent-native SDK continuation, provider resource settlement, full local sync/custom-path import and existing legacy-workspace/archive migration
remain required.
This implementation does not convert a legacy Python runtime database into a
Rust engine profile.

Final restore verification: **21 integrated PostgreSQL/HTTP/packaged-guest tests**,
**20 runtime checks**, **9 cloud unit checks** and launcher upload/restore acceptance
pass. Archive fixtures now deterministically lose the first successful quiescence
acknowledgement, in addition to their existing lost transcript acknowledgement.
The guest retries final projection publication and the idempotent quiescence
receipt while retaining storage locks, bounded by its authority credential expiry.
A transient final-flush response no longer exits before recording quiescence.

Resume requeues a failed restore delivery against the existing operation and
replacement name. Its fresh boot grant lifetime begins after verified installation,
so image allocation and archive download do not consume the enrollment window.
An expired pre-enrollment grant is renewed only while no runtime bootstrap receipt
exists. Acceptance expires the grant both during allocation and after installation,
then verifies the same replacement enrolls and continues without creating another
machine or replaying the original turn.

### Permanent deletion backend and content-free tombstones

Migration `0008_delete.sql` adds durable deletion operations, original deletion
columns when absent and a provider-create outcome journal. Writable-runtime
readiness requires it. Startup never applies it. Before rolling out this worker,
drain older workers that do not use the execution guard or create journal.
The API remains read-only unless explicitly enabled; this does not enable cloud
recovery or transfer any billing/export ownership.

The original `POST /v1/cloud/workspaces/:id/delete` contract checks authenticated
ownership/edit permission. Current runtime writes remain personal and Boxd-only;
organization and other-provider rollout remain separate gates. Admission commits
an irreversible deleted intent, advances generation/epoch/destruction fence,
revokes bootstrap/runtime credentials and gateway leases, and cancels old workspace
jobs before returning `deleting`. A repeat request requeues failed cleanup for
the same operation. No entitlement is required to delete, and no archive deadline
implicitly deletes history.

The durable worker progresses through provider cleanup, object purge and metadata
cleanup. Public workspace responses include the operation ID/phase and actual
status; `deleted` is written only after all stages finish. It resolves immutable
source, allocation and replacement receipts, preserves a discovered exact machine
ID before destruction, checks other workspace/persistent-machine references, and
reconciles lost delete replies by inspecting that ID. An uncertain create without
a visible receipt remains a retryable error rather than a claim that cleanup
finished. Shared machines are retained with an actionable error.

A shared-volume execution guard covers allocation/restore effects and privileged
verification through temporary-directory destruction. Delete waits for this guard
before purging. A create outcome journal survives transaction rollback and records
machines returned after Delete admission. Old workers cannot attach or launch
against the advanced generation. Privileged verification checks ownership, intent
and its job lease before every object read; cancellation/deletion stops further
chunk downloads. Its temporary data remains guarded even if the outer job future
is cancelled, then is removed before deletion completes.

All workers handling these resources must use the same private TMPDIR-backed
scratch volume (the provided Compose worker volume). Multiple worker hosts require
the same shared filesystem/lock namespace; do not run independent scratch volumes
for a shared workspace ownership pool. Cleanup removes only the hashed workspace's
verification directory. Empty lock metadata remains available for fencing.

R2 purge uses bounded ListObjectsV2 pages and scoped DeleteObject requests, then
requires a fresh empty listing. Every listed key is checked against the exact
`workspaces/<encoded-workspace>/` prefix before any deletion from that page. URL
encoding preserves literal percent/Unicode keys. Malformed XML, redirects,
unsupported purge adapters and out-of-scope listings fail without reporting
completion. The implementation follows [ListObjectsV2](https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html)
and [R2's supported S3 operations](https://developers.cloudflare.com/r2/api/s3/api/).

Metadata purge removes encrypted transcript pointers, launch intents, runtime
summaries, public conversation rows/receipts, mailbox envelopes, encrypted boot
grants and Rust allocation/lifecycle/recovery/restore records. It clears the
workspace key, runtime credentials, Git branch names and request context. The
remaining workspace record contains opaque identity/billing references, dates and
fences, without transcript/files/secrets. Required project/build/workspace IDs
remain because existing financial usage has foreign keys; usage, subscriptions
and provider settlement history are preserved. The original create idempotency key
is replaced with generated metadata; a keyed digest prevents a delayed create
retry during the tombstone window. Tombstones disappear from API reads after
30 days. Content-free billing references are retained rather than deleting usage.

Verified with **25 integrated PostgreSQL/HTTP/packaged-guest tests** and **11 cloud
unit tests**. Focused deletion covers live verification, late allocation receipts,
partial replacement cleanup, failed object purge/retry, lost provider-delete replies,
30-day tombstones, unrelated workspace/object preservation and create replay
fencing. S3 listing/delete signatures match botocore 1.42.97 fixtures. The HTTP
object tests cover multiple pages, partial delete failures, malformed XML,
percent/Unicode names and malicious listing scope. Recovery acceptance also passes
with the additional per-object cancellation checks.

Full local sync/custom-path import, existing-archive migration and live
provider/R2 acceptance remain required before the archive milestone is complete.

### Native and browser deletion controls

Archived cloud rows now offer explicit Delete with a permanent-deletion dialog.
Escape cancels only the dialog; browser keyboard focus starts on Cancel and Tab/Enter
can confirm. Native and browser clients submit through the host credential broker
using `CloudDelete`; this mutation is never automatically replayed after uncertain
delivery. Retry preserves its command identity and the backend deletion operation.

An accepted request keeps the row visible with actual cleanup progress. Metadata
polling updates provider/object/metadata phases and retryable failures without
starting compute. Unarchive and saved-history opening are disabled while deletion
is in progress. A confirmed tombstone removes the archived row; request acceptance
alone does not. Account switches discard pending view state and cached history.

The compiled GPUI browser passed an actual host/API/PostgreSQL/packaged-guest
workflow: Escape cancellation with no mutation, Tab/Enter confirmation, failed
object cleanup with a visible Retry button, retry of the same operation, and
confirmed tombstone removal. The isolated worker additionally verified exact source
machine deletion, absence of workspace objects and zero compute wakes. Its machine
provider and object store are deterministic; this is not live provider acceptance.
Screenshots: `.context/cloud-delete-confirmation.png`,
`.context/cloud-delete-retry.png`, `.context/cloud-delete-completed.png`.
The reusable acceptance driver is `scripts/acceptance/cloud-delete.mjs`; the
PostgreSQL fixture is enabled only with an explicit `ZUSE_CLOUD_UI_FIXTURE_FILE`.
Native Linux UI checking and the compiled browser build pass. Native visual
acceptance on Mac remains pending.

Focused checks also pass for shared archived-state reduction (3 tests), RPC mutation
retry classification (1 test), and host cloud credential/origin guards (2 tests).
The broader engine test command hit sandbox disk capacity while linking unrelated
integration binaries; those generated binaries were removed and the intended
`--lib cloud::tests` check passed. No broader engine-suite pass is claimed.


### Reference local mirror cleanup

`crates/engine/src/cloud/mirror.rs` reads the original `.zuse-sync.json` v1
ownership format, including interrupted `pending` publications. Older markers
without a version establish no file ownership. The host discovers matching markers
under the reference default `~/.zuse/cloud` tree; an explicit
`ZUSE_CLOUD_MIRROR_ROOT` selects an alternative managed root for acceptance or
configuration. It does not search unrelated home folders or accept a renderer
supplied deletion path.

A confirmed `archived`, `deleting` or `deleted` catalog state admits cleanup.
`archiving`, paused, disconnected, disabled or ready state alone does not. A host
startup/login metadata reconciler runs independently of the Settings screen.
Repeated catalog reads report cached local progress instead of restarting cleanup.
A new ready-to-archived cycle permits a new cleanup. Local hashing runs off the
async executor and cannot wake cloud compute.

Cleanup validates ownership and any sibling download cache before removal. It
hashes current regular-file bytes, verifies mode/size and mutation identity, compares
links without following them, and removes only unchanged manifest-owned entries.
Local edits, unowned `.env` files and extra files stay. It prunes only empty parents
of removed entries, updates the ownership marker with a flushed rename, and removes
a fully empty mirror. Matching workspace-owned caches are purged; a missing
checkout can still have its explicitly known cache cleaned. Interrupted publication,
foreign markers and root symlink ancestors preserve data and return an actionable error.
Changed file-parent symlinks are left alone.
Unix descriptor-relative operations anchor file access and removal below the checked
root rather than resolving a symlink into another tree.

Native/browser archived views show pending and failed local cleanup with Retry
cleanup. A cloud tombstone stays visible while local cleanup is pending or failed;
confirmed server deletion alone does not hide an unresolved local error. The retry
RPC rechecks the authenticated catalog state and returns the same workspace with
host-local status. It does not invoke a cloud lifecycle mutation.

Nine filesystem/lifecycle tests, two host cloud credential guards, four shared
archived-state tests and RPC retry classification pass. The compiled browser
workflow ran against the actual host/API/isolated PostgreSQL and packaged guest:
interrupted publication kept all local data; the Retry cleanup action removed an
unchanged synced file and its owned cache, while preserving a newer edit, local-only
`.env`, extra file and local `.git/HEAD`. It then completed the full cloud deletion
confirmation/failure/retry/tombstone workflow. The worker verified exact source
machine retirement, absence of workspace objects and zero compute wakes. Its
provider/object store are deterministic; live-provider acceptance is still pending.
Native Linux UI checking and the browser build pass. Mac visual acceptance remains
pending. Screenshots are `.context/cloud-mirror-cleanup-error.png` and
`.context/cloud-mirror-cleanup-completed.png`; the receipt is
`.context/cloud-delete-browser-acceptance.json`. Enable the extended driver with
`ZUSE_CLOUD_MIRROR_ACCEPTANCE=1` and a fresh isolated UI fixture file.

Cache payload cleanup retains its ownership marker until all payloads are removed.
A failed final directory unlink restores that marker so retry still has ownership
proof. Focused failure injection checks this boundary and subsequent retry. This
sandbox permits directory access despite chmod mode bits, so permission-based fault
injection does not exercise that failure; the test injects the final unlink error
explicitly instead.

This adds cleanup of discoverable reference mirrors. The live Git snapshot/transfer
worker, sync preference/path controls, custom legacy target registration/import,
joining that future transfer worker before cleanup, and orphan cache discovery
without a known checkout remain required. It does not claim full local sync parity.


Next implementation focus: finish first-launch attachments/context, startup queues and
remaining native routing/cached reopen using the shared transport, then cloud administration.
Full live sync and migration remain open; the verified browser create/stream/reopen
workflow and default mirror cleanup do not close those requirements.

### Existing Stripe and Polar billing portals

The Rust `POST /v1/billing/portal` contract selects the account's original provider
from non-ended subscriptions. Multiple providers return `billing_provider_ambiguous`;
accounts without a subscription use `BILLING_DEFAULT_PROVIDER`. Stripe retrieves the
stored `api_stripe_customers.customer_id`, verifies its original `account_id` metadata
and creates a portal session for that same customer. It never creates or relinks a
customer. Polar uses the original external owner identity, including organization
owners. Both adapters validate HTTPS session links, bound response size/time and
sanitize provider failures. Session links and reusable provider keys are not logged.

Portal admission requires billing-role authorization and the same separate billing
writer allowlist/ownership lock as cap changes. This does not transfer writer,
webhook or export ownership. The catalog connection remains read-only. Native and
browser Manage billing actions use the host authority and reject replies after an
account/organization change. Portal creation is never automatically replayed.

Configuration is loaded only when `ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS` enables the
billing writer. Stripe uses `STRIPE_SECRET_KEY`, optional `STRIPE_ENVIRONMENT`
(`sandbox`/`production`, checked against the key), optional
`STRIPE_PORTAL_CONFIGURATION_ID` and `API_ISSUER` as the portal return URL.
Polar uses `POLAR_ACCESS_TOKEN` and `POLAR_ENVIRONMENT` (`sandbox`/`production`).
`BILLING_DEFAULT_PROVIDER` selects the default; existing subscriptions override it.
Missing provider configuration returns an actionable unavailable response; no
production account or ownership marker is enabled by this change. The adapters use
[Stripe's portal session contract](https://docs.stripe.com/api/customer_portal/sessions/create)
and [Polar's customer sessions contract](https://github.com/polarsource/agent-toolkit/blob/main/skills/polar-integration/SKILL.md).

PostgreSQL/HTTP acceptance passes for personal isolation, organization billing-role
authorization, original Stripe/Polar selection and customer IDs, ownership fencing,
ambiguous providers, ended subscriptions and unchanged original subscription IDs.
The Stripe transport test verifies the original customer before session creation;
the Polar transport test verifies external identity and rejects unsafe links.
Receipts: `.context/cloud-portal-postgres.log` and
`.context/cloud-portal-provider-tests.log`. UI acceptance is recorded separately.

Checkout, renewal/cancellation ingestion, payment-failure processing, settlement,
webhook deduplication and metered exports remain open. Live provider and original
account verification remain required before billing ownership cutover.

Native Linux and the compiled browser both opened the selected provider portal
through the actual host/API/PostgreSQL workflow. External URL launch was intercepted
only for the fixture provider host; no live Stripe/Polar session or subscription was
modified. Browser observation confirms one click creates one portal session and no
`CloudEnvironment` compute wake. Native launch reached the system URL opener.
Receipts: `.context/cloud-portal-native-acceptance.json`,
`.context/cloud-portal-browser-acceptance.json` and `.context/cloud-portal-ui-api.log`.
Native/browser builds and the mutation retry-classification test pass. Screenshots:
`.context/cloud-portal-native.png` and `.context/cloud-portal-browser.png`.

### Signed cloud subscription lifecycle events

Rust now serves the original `/v1/billing/webhook` and provider-specific
`/v1/billing/webhook/{provider}` contracts. Stripe signs the exact raw request body
and timestamp; Polar binds the delivery ID, timestamp and raw body. Verification
rejects stale/future signatures, accepts rotated v1 signatures and preserves the
reference Polar literal-secret format alongside Standard Webhooks encoded keys.
Raw delivery bodies and reusable keys are never stored in event receipts.

Signed payloads only identify the subscription. The adapter fetches current
provider state before changing entitlements. Stripe requires one recognized base
price, matching customer/account metadata and the original local customer link.
Polar uses its original customer external identity and product mapping. Unlinked
subscriptions remain deferred without consuming the event. Unknown events are
ignored without starting workspace compute.

Subscription delivery serializes before the canonical fetch. Account writes hold
the independent Rust billing ownership marker, and period renewal shares an
account lock with spend-cap updates. Entitlement changes, event deduplication and
period changes commit together. Existing entitlement IDs/creation times, provider
subscription IDs, period IDs, negotiated price/markup, caps and settled ledger
charges remain intact. New entitlements and periods use the reference deterministic
IDs and policy. Renewal carries the previous cap. Pending/past-due subscriptions
put billing on hold; an ended subscription retains its paid window and only ends
billing when that window has elapsed. A competing provider or subscription cannot
replace an existing period's owner.

Persistent-machine subscription events return a retryable unavailable response
without recording successful processing: their machine provisioning/recovery
workflow is still pending. Prepaid event processing, financial settlement, metered
exports and checkout remain open. This change does not activate production writes,
transfer webhook ownership or replace original-account/live-provider acceptance.

Verification uses an isolated PostgreSQL schema, the actual Axum routes and real
Stripe/Polar HTTP adapters against a local provider server. Signed API delivery
covers redelivery/concurrency, current-state reconciliation of delayed events,
renewal, payment failure, cancellation before/after paid-through, legacy Polar
subscriptions, wrong customer/account metadata, ownership fencing, deferred
unlinked subscriptions and atomic rollback on period-owner conflict. The provider
fixture exposes only subscription GETs; no charge or customer-creation operations
occur. Financial ledger totals and existing pricing/caps/identities are checked
after processing. Receipt: `.context/cloud-events-acceptance.log`.

The verifier follows [Stripe's signing implementation](https://github.com/stripe/stripe-node/blob/master/src/Webhooks.ts)
and the [reference Polar SDK verifier](https://github.com/polarsource/polar-js/blob/main/src/webhooks.ts).
No new UI is introduced by this backend workflow.

Webhook configuration uses the original `STRIPE_WEBHOOK_SECRET`,
`STRIPE_PRICE_CLOUD_WORKSPACE_STANDARD_V1`, `STRIPE_PRICE_PERSISTENT_STANDARD_V1`,
`POLAR_WEBHOOK_SECRET`, `POLAR_PRODUCT_CLOUD_WORKSPACE_STANDARD_V1` and
`POLAR_PRODUCT_PERSISTENT_STANDARD_V1` variables. The deprecated
`POLAR_PRODUCT_SANDBOX_STANDARD_V1` cloud-product fallback remains supported.
A configured webhook requires at least one offer mapping; duplicate product/price
mappings fail configuration. The billing allowlist and explicit ownership marker
remain required for processing an account's events.

Final focused verification: six billing library checks pass, including the signed
HTTP/provider/PostgreSQL lifecycle workflow. Three existing integration checks pass
for portal ownership, usage/cap administration and billed workspace creation.
Receipts: `.context/cloud-events-acceptance.log`,
`.context/cloud-events-billing-regression.log`; API application checking passes in
`.context/cloud-events-api-check.log`.

### Cloud subscription checkout and signed browser receipt

The original `POST /v1/billing/checkout` now uses the host-selected personal or
organization billing owner. It checks the explicit Rust billing writer marker and
allowlist, reconciles existing non-ended subscriptions against their original
provider, preserves entitlement identities and refuses an already subscribed
account. Organization non-cloud purchases remain forbidden. Persistent-machine
sales remain unavailable until their provisioning/event workflow is implemented.

Stripe reuses the original customer mapping and verifies remote account metadata.
New customers use the existing `api_stripe_customers` reservation table and legacy
`zuse-customer:<owner>` idempotency key. The reservation commits before customer
creation; a lost response retries the same key. After 23 hours, checkout performs
bounded, cursor-persisted remote discovery. It only recovers a verified unique
customer or advances the original reservation generation after proving full remote
absence. Multiple matches or incomplete pagination remain actionable recovery
errors; they cannot create another customer. Each retry advances saved recovery.

Stripe checkout retains the reference subscription/base and optional metered
price lines, account/offer metadata, promotion codes, tax/address collection and
customer update settings. Subscription-only mode omits the metered line. Missing
meter configuration cannot sell a metered cloud plan. Polar checkout preserves
external customer identity, product mapping, metadata and disabled trials.

Success URLs carry an original-format Ed25519 `checkout-receipt+jwt` ticket for the
account/offer, valid for 24 hours. The unauthenticated return page only looks up an
order after verifying that ticket and requires the provider's checkout owner to
match. Missing/expired/wrong-purpose proofs expose only a generic pending page.
Receipt lookup is bounded to two seconds. Rendered amounts use exact cents, and
receipt pages set a restrictive CSP, no-store and no-referrer headers. Session URLs
and signing/provider keys are not logged.

Native/browser Cloud settings read scoped entitlements independently of usage, so
accounts without a billing period can subscribe. Existing subscribers open Manage
billing. Both actions use the root host authority, discard foreign-account replies
and are excluded from automatic transport replay. Refresh reads current entitlement
state after signed activation. Screens use the existing settings components.

`MACHINE_LIVE_CHECKOUT_ENABLED=true` explicitly enables checkout configuration and
requires the original `RELAY_MINT_PRIVATE_JWK`; it does not bypass billing writer
ownership. Provider webhook/offer configuration must also exist. Stripe retains
`STRIPE_CLOUD_BILLING_MODE`, `STRIPE_CLOUD_OVERAGE_PRICE_ID`,
`STRIPE_CLOUD_OVERAGE_METER_ID` and `STRIPE_AUTOMATIC_TAX_ENABLED`. Existing provider
price/product configuration and the legacy Polar cloud-product fallback remain.
No production flags or ownership markers are changed by this implementation.

Backend acceptance uses the real API, PostgreSQL and provider HTTP adapters. It
covers existing customer/subscription preservation, organization authorization,
checkout receipt authorization, lost customer-create responses, paginated recovery,
verified remote absence, ambiguous remote customers, signed subscription activation
and duplicate-subscription rejection. Separate transport acceptance covers Polar
checkout and Stripe subscription-only/tax-disabled parameters. Receipt:
`.context/cloud-checkout-acceptance.log`.

Live paid-provider acceptance, automatic customer-recovery scheduling, reconciliation
of uncertain checkout-session creation, full receipt-page visual parity, prepaid
credits, settlement/export and persistent-machine billing remain open before live
billing cutover. Source-compatible checkout session creation is never automatically
replayed; durable customer recovery does not establish checkout-session recovery.

Compiled browser and native Linux acceptance both opened checkout through the real
host/API/PostgreSQL/provider-HTTP path. A signed activation for that same fixture
account then made Refresh show Manage billing, its active allowance and the usage
panel in both clients. The native system URL launcher and browser external link were
intercepted only for the fixture provider host. No live purchase was made. The
browser trace confirms no `CloudEnvironment` compute wake. Receipts:
`.context/cloud-checkout-ui-api.log`, `.context/cloud-checkout-native-acceptance.json`
and `.context/cloud-checkout-browser-acceptance.json`. Screenshots:
`.context/cloud-checkout-native.png`, `.context/cloud-checkout-native-activated.png`,
`.context/cloud-checkout-browser.png` and `.context/cloud-checkout-browser-activated.png`.
Native and wasm builds pass; billing regression and action-retry checks are recorded
in `.context/cloud-checkout-regression.log` and `.context/cloud-checkout-retry-tests.log`.

### Prepaid invoice credits

Rust now serves the original `/v1/billing/prepaid`, `/prepaid/checkout` and
`/prepaid/complete` contracts. Provider selection retains existing non-ended
Stripe/Polar ownership. Stripe reads the verified original customer's USD invoice
balance; positive debt and available credit remain separate. Unsupported providers
report unavailable purchases, while existing Stripe balances remain readable when
purchases are disabled. Checkout accepts the reference $10/$25/$50/$100 amounts,
validates the configured active one-dollar, non-recurring USD price, reuses the
original customer reservation and allows card payments.

Signed completion/refund/dispute notifications fetch canonical checkout, captured
payment, charge, line-item and complete paginated refund evidence. Customer,
account metadata, configured price, currency, quantity and supported funding amount
must agree before any balance adjustment. Unpaid sessions do not fund credit.
Refund totals and dispute amounts are bounded by the purchase; ambiguous mixed
refund/dispute evidence requires reconciliation. Reinstatement applies the original
withdrawal and reinstatement keys in order, handling reversed event delivery.

Funding/reversals retain original `prepaid:purchase:*`, `prepaid:refund:*` and
`prepaid:dispute:*` keys in `api_stripe_meter_deliveries`. Serialized receipt payloads
keep the original field names and order. A committed delivery lease precedes every
remote balance transaction; acknowledged receipts never resend. Lost responses
retry the same provider idempotency key. Old uncertain writes stop after 23 hours
and retain actionable reconciliation state. Existing settled receipts remain
authoritative beyond that window. No provider customer/subscription IDs, cloud
usage ledger, allowance or cap are replaced.

The billing authority uses a separate bounded delivery connection pool so durable
receipts can commit while a control transaction holds the billing ownership fence.
Customer reservations use that pool too. Catalog connections remain read-only,
and production billing ownership/checkout flags remain unchanged.

Native/browser controls reuse existing settings components for balance, debit,
fixed amount selection, Buy credits, disabled purchases and Refresh. Host-authority
RPC scopes replies to the admitted account/organization, never automatically replays
a credit checkout and never acquires a workspace execution connection. Monetary
labels use exact integer cents. The purchase description retains the reference
invoice application/rollover semantics. Configure `STRIPE_PREPAID_CREDIT_PRICE_ID`
with the existing Stripe/webhook/live-checkout configuration to enable sales.

Verification: ten billing library checks pass, including actual signed HTTP routes,
isolated PostgreSQL and Stripe HTTP adapters. Funding tests lose an accepted balance
adjustment response, redeliver concurrently, reuse original historical receipts,
handle paginated refunds and out-of-order dispute reinstatement, reject an invalid
price and foreign customer metadata, fence legacy-owned writes and refuse old
uncertain adjustments. Allowance/cap preservation is asserted. Three billing
integration regressions and mutation retry classification pass. Receipts:
`.context/cloud-credits-acceptance.log`, `.context/cloud-credits-regression.log` and
`.context/cloud-credits-retry-tests.log`.

The compiled native Linux and browser flows opened $25 and $50 checkout respectively
and refreshed a verified $50 funding from $15 to $65. Native system/browser URL
launches were intercepted only for the fixture provider host; no live purchase was
made. Browser wire observation confirms no `CloudEnvironment`. The API fixture
checks the exact purchase amounts and final balance. Receipts:
`.context/cloud-credits-ui-api.log`, `.context/cloud-credits-native-acceptance.json`
and `.context/cloud-credits-browser-acceptance.json`. Screenshots:
`.context/cloud-credits-native-funded.png` and `.context/cloud-credits-browser-funded.png`.
Native and wasm builds pass.

The adapters follow [Stripe's invoice balance contract](https://docs.stripe.com/api/customer_balance_transactions)
and [Checkout contract](https://docs.stripe.com/api/checkout/sessions/create).
Live paid-provider and Mac acceptance remain open. This does not close settlement,
metered exports, persistent-machine billing, image administration or the rest of C.

### Owned billing and telemetry export worker

`zuse-cloud-api billing-worker [--once]` now drains the existing financial and
telemetry outboxes through real Stripe/Polar HTTP adapters. The optional Compose
`billing` profile runs it independently of API viewers and allocation/recovery.
`CLOUD_BILLING_EXPORT_ENABLED` and `CLOUD_USAGE_EXPORT_ENABLED` are separate,
default-off flags. Disabled once-mode returns before database/provider setup.
Enabled mode requires explicit `ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS`, provider
configuration and the default provider's overage meter for financial exports.
The worker does not transfer ownership or enable production flags.

Every processed account requires both original Rust billing ownership and a
separate `zuse_rust_control_owners` marker with `resource_kind='billing-export'`,
`resource_id` equal to the original owner and `backend='rust'`. Both markers stay
share-locked through processing. Before transferring this marker, fence the old
exporter as specified by G; a marker alone cannot stop old implementations that do
not honor it. Unowned accounts remain untouched, including retry counters.

Financial exports retain the original outbox ID, period, provider, timestamp,
signed cent amount and idempotency key. They lock their outbox/period, verify the
period owner/provider/currency and original Stripe customer, and acknowledge only
a successful provider delivery or existing durable receipt. Failed items retain
sanitized errors and the original bounded exponential retry schedule. PostgreSQL
acceptance uses the original bigint counters and composite provider/idempotency
index. Shadow/default-off runs never emit charges.

Stripe uses the reference SHA-256 `zuse_...` event identifier and byte-compatible
JSON receipt format in `api_stripe_meter_deliveries`. Credits and meter delivery
now share that durable lease/receipt implementation. Existing sent receipts skip
provider effects, even if invoice eligibility has since changed. Old uncertain
writes stop after 23 hours. New overage writes require the matching subscription
period/metered price or a matching draft invoice; finalized invoices stay retryable
without a meter write. Timestamp validation retains the reference 35-day/five-minute
window. Stripe requests explicitly use `2026-09-30.endive`, matching the reference's
[stripe-node 23.0.0 API version](https://github.com/stripe/stripe-node/blob/v23.0.0/src/apiVersion.ts).

Polar retains original external customer/event IDs, timestamps, metadata and signed
unit values. Telemetry only accepts the two reference observation/cost event names
and `billable=false`; it cannot inject invoice units through the usage queue. The
worker never obtains a workspace execution connection.

Reconciliation waits for settled outboxes, records attempts fairly (including
missing meters and failed requests), prevents duplicate concurrent polls, and
persists authoritative expected/observed totals. Stripe reads all summary pages
using the reference minute-aligned bounds; Polar reads its original customer meter.
A mismatch is recorded without automatically emitting corrective financial events.

Twelve billing library checks pass, including isolated PostgreSQL and actual
Stripe/Polar HTTP adapters. Export acceptance covers lost accepted responses,
concurrent reconciliation, missing/failing meters, original keys, an independently
generated JavaScript receipt fixture, draft/finalized invoice handling, old uncertain
delivery refusal, telemetry separation and ownership fencing. Existing credit,
checkout and signed subscription workflows still pass after sharing delivery logic.
Receipts: `.context/cloud-exports-acceptance.log`. The API executable builds and
disabled/no-ownership command checks pass in `.context/cloud-exports-api-build.log`
and `.context/cloud-exports-cli.json`. Billing integration regressions are recorded
in `.context/cloud-exports-regression.log`. No UI changes are introduced here.

Provider report ingestion/cost settlement, additional provider sources, snapshot
storage settlement, live provider/export acceptance and migration/cutover remain
open. The worker exports accepted ledger outboxes; it does not turn runtime
observations into invoice evidence. This does not complete C or G.


### Completed Boxd report settlement

The billing worker now polls closed Boxd daily usage windows with the reference
30-minute delay, rotates at most seven historical windows plus the latest, and
settles completed executions against original workspace/build identities. The
provider adapter uses SDK 0.2.14 `GetOrgUsage`/`GetVmUsage` gRPC-web contracts.
Per-machine reports must match the requested machine, time bounds, USD currency
and completion status. Costs come from canonical reports, including retained
resources; running observations alone cannot become customer charges.

Retired-machine observations retain attribution to the original account/resource.
Customer-connected providers, unmapped resources and incomplete reports are
excluded. Existing event/execution finalizations prevent recharging legacy usage.
Reports are bounded by resource creation and the explicit cutover; first-resource
reads retain the reference quarter-hour bucket alignment. Period gaps or overlaps
remain deferred without finalizing the execution.

One fenced transaction locks the account, revalidates resource identity/config and
all overlapping original period terms, then records every period segment, ledger
delta, reservation removal, nonbillable cost telemetry and cent outbox together.
Finalization commits with those writes. Included allowance, original markup,
manual entitlement credits, cap absorption, cumulative rounding, period-specific
provider and original event/outbox/idempotency formats are preserved. Failed
capture or database writes leave the window event retryable; a partial execution
cannot be finalized. Per-execution errors appear in the worker report.

Settlement is default-off and independent of export. Enable only after fencing
the old billing writer and assigning the original account's Rust billing marker:
`BOXD_BILLING_ENABLED=true`, `BOXD_ADAPTER_ENABLED=true`, a positive whole-second
RFC3339 `BOXD_BILLING_CUTOVER_AT`, `BOXD_API_KEY`, optional `BOXD_ORG`, and explicit
`ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS`. Cost-only runs do not require Stripe/Polar
credentials or export ownership. Export still requires its separate marker and
flags. `BOXD_BASE_URL` accepts an HTTPS cluster and derives token exchange using
the SDK's console-host rules; provider credentials do not follow redirects.

Fourteen billing tests pass, including isolated PostgreSQL and actual HTTP
provider adapters. The new workflow covers two billing periods, retained old
machine attribution, a prepared-image build, BYOK/incomplete/unmapped exclusions,
legacy finalization aliases, original ledger/subscription preservation, manual
credits, cap absorption, cent outboxes, reservation removal and nonbillable
telemetry. A database trigger interrupts the second period of a build; both
segments roll back, then concurrent retries settle exactly once. Receipt:
`.context/cloud-settlement-tests.log`.

Three Boxd adapter checks pass, including independently SDK-generated usage
request/response bytes through the HTTP transport, cached token exchange,
cluster/auth URL derivation, and rejection of incomplete, non-USD, wrong-machine
or wrong-period cost reports. Existing machine lifecycle/ambiguous-create checks
still pass. Receipt: `.context/cloud-settlement-provider-tests.log`.

The API executable builds (`.context/cloud-settlement-api-build.log`). Eight
command checks pass without a database connection: disabled mode, missing or
fractional cutover, adapter-off mode, missing account ownership, missing API key,
cost-only database requirements and rejection of insecure cluster configuration.
Receipt: `.context/cloud-settlement-cli.json`. Allocation and billing workers now
honor the same configured Boxd cluster.

Remaining scope includes other provider usage sources, snapshot/storage
settlement, live provider acceptance and billing writer migration. No production
flags or ownership have been transferred.


### Retained-image storage settlement

The billing worker now settles the reference Box retained-image records under
`CLOUD_SNAPSHOT_BILLING_CUTOVER_AT`. An absent cutover disables storage billing.
Storage-only execution requires the original billing account allowlist and Rust
writer marker, without Boxd or payment-provider credentials. No production flags
or ownership have been transferred.

The existing immutable `boat-snapshot-2026-10-v1` schedule remains $1.70 per
30-day month. Integer rational arithmetic carries the original sub-micro-USD
remainder between checkpoints. Ongoing retention settles after an hour; a stopped
or deleted image also settles its final partial interval. Existing period bounds,
cutover, checkpoint and stopped timestamps determine the billed interval. Period
gaps retain the reference remainder reset, and uncovered trailing time stays
pending. Existing finalized execution aliases are recognized without another
ledger/export write.

Settlement uses the original snapshot account advisory lock and defers while any
unexpired shared/exclusive image lease exists. It also holds the billing writer
marker and account lock. Snapshot checkpoints, fractional remainder, usage,
provider receipts, finalizations, ledger deltas, reservations, cent outboxes and
nonbillable telemetry commit together. Original event and entry IDs are preserved.
Imported customer-owned builds are excluded; other record fields and unrelated
accounts remain unchanged. No execution transport or machine wake is acquired.

The same confirmed-usage ledger routine now accepts both Boxd machine and Box
snapshot identities. Boxd retains its original per-period entry IDs and provider
cost evidence. Storage exports use `resource_kind=snapshot`, `measurement=confirmed`
and `cost_source=approved-storage-schedule`; the existing billing usage interface
continues exposing the compatible `other`/`snapshot-storage` classification.

Sixteen billing tests pass, including actual PostgreSQL settlement over two
periods, retained/deleted images, imported legacy finalization, BYOK and account
ownership exclusions, a live shared lease, concurrent retries, exact cent outboxes
and preservation of unknown checkpoint fields. An interrupted checkpoint rolls
back its usage/ledger/outbox/finalization; retry settles once. The arithmetic test
compares 720 hourly checkpoints with the original monthly cost and exercises
fractional micro-USD remainders. The original immutable price trigger is also
verified. Receipt: `.context/cloud-storage-tests.log`. The API executable builds
(`.context/cloud-storage-api-build.log`), five storage configuration guards pass
(`.context/cloud-storage-cli.json`), and the eight Boxd/export command guards still
pass (`.context/cloud-storage-cli-regression.json`).

Remaining image work includes provider presence inspection, build/promotion,
supersession, grace expiry, provider deletion/retry and their administration UI.
This adds storage settlement to C; it does not complete the image lifecycle,
remaining usage providers, live billing acceptance or migration/cutover.


### Owned repository catalog management

Added the pinned `GET/POST /v1/cloud/projects` and
`DELETE /v1/cloud/projects/:id` contracts with shared Rust wire types. Connect
normalizes repository identity/URLs, validates branch/environment values, checks
existing cloud entitlement and stores the original configuration-digest format.
The repository identity remains case-insensitive; reconnect preserves its original
project ID and creation time. Included retries return the original accepted row.
Concurrent retries serialize across the account and retain the original repository
advisory lock. Conflicting removed-row keys return an actionable conflict.

Remove only sets `included=false`. Existing project records, branches, workspace
rows, machines, chats, image builds and billing history remain intact. Reconnecting
the same repository reuses that project. Listing uses one read-only repeatable-read
snapshot, includes active compatible builds and latest build status, and recognizes
repository layouts from imported snapshots. Environment values, secret bindings,
private build settings and build logs never enter this public projection.

Reads use existing Content authorization. Writes require Administration, which
permits organization admins and denies ordinary/billing members. Every query is
scoped to the selected owner. An organization cannot remove a personal project
by guessing its ID. Secret bindings retain the reference vault-required error;
secret-like environment keys are refused without echoing their values.

Writes remain default-off. `ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS` configures a
separate writer pool; every mutation also requires the share-locked
`zuse_rust_control_owners` row with `resource_kind=project-catalog`, the original
owner ID and `backend=rust`. Fence/forward the old project writer before assigning
this marker. Billing or workspace ownership does not confer project permissions.
The catalog pool stays read-only.

`ZUSE_CLOUD_PROVIDER_TEMPLATE_VERSIONS` is a JSON provider-to-current-full-template
version map for public build compatibility. Missing versions never advertise an
unverified compatible build. Connection-specific full versions retain their
original `:connection:<id>` compatibility rule. This read model does not enable
unimplemented execution adapters or relax Rust runtime negotiation.

Acceptance passes against isolated PostgreSQL through the actual Axum routes:
concurrent connect retries, original ID/creation-time preservation on reconnect,
latest failed versus active compatible builds, private metadata exclusion, personal
and organization isolation, admin/member/billing permissions, expired entitlement,
writer ownership handoff, secret validation and retained workspace bytes after
removal. The configuration digest matches an independently generated Node
`crypto`/`JSON.stringify` value. Receipt: `.context/cloud-projects-tests.log`.
All thirty cloud library checks also pass, covering billing/settlement/export,
provider wire adapters, signed WorkOS identity, immutable objects and credential
transport after the shared Store/wire changes:
`.context/cloud-projects-regressions.log`. The API executable also builds:
`.context/cloud-projects-api-build.log`.

Image preparation/rebuild execution, account image configuration/status, live
account provider/connection resolution, GitHub enrollment and repository
administration UI remain open. This implements repository catalog operations,
not all image parity.


### Shared native/browser repository settings

A shared GPUI repository panel is connected through the root host account authority.
The new scoped RPCs list/connect/remove repositories and fetch the GitHub catalog;
clients carry their expected account/organization and the host rejects stale scope
before issuing HTTP requests. Remove uses the original DELETE contract. Reads do
not obtain environment transport; connect/remove are never automatically replayed.
The panel clears account state, cancels pending view requests, retains per-repository
idempotency keys after uncertain failures, and verifies account-bound replies.

The panel uses the existing settings frames, compact rows, search input and
floating picker. It supports multiple selection, private/default-branch metadata,
add/remove, loading, refresh/error and empty states. Both native Cloud settings and
the compiled GPUI browser render the same component.

GitHub enrollment is still owned by the original service. The optional
`ZUSE_CLOUD_LEGACY_GITHUB_URL` adapter forwards the verified user's bearer and
selected scope to its existing `/v1/cloud/github` endpoint. The original service
continues refreshing OAuth and installation grants. The adapter requires HTTPS,
refuses redirects, bounds/validates responses, strips unknown/private fields and
recomputes installation-management permission from current Rust authorization.
This is an explicit migration adapter, not a completed Rust GitHub authority port.

Verification passes:

- Thirty-one cloud library checks cover the new scoped GitHub migration reader,
  private-field filtering and redirect refusal alongside original billing,
  settlement, provider, authorization and repository operations.
  `.context/cloud-repositories-api-tests.log`.
- Native Linux: searched and selected an authorized repository, added it through
  the actual host/API/PostgreSQL path, removed another repository and reopened the
  view with persisted state. The final shared picker uses the embedded SVG check
  and outside-click dismissal. `.context/cloud-repositories-native-acceptance.json`;
  images `.context/cloud-repositories-native.png` and
  `.context/cloud-repositories-native-picker.png`.
- Compiled GPUI browser: authorized catalog/search, connect/remove through visible
  controls, stale account and organization refusal, and no environment allocation
  RPC. `.context/cloud-repositories-browser-acceptance.json`,
  `.context/cloud-repositories-browser-proof.json`; images
  `.context/cloud-repositories-browser.png` and
  `.context/cloud-repositories-browser-picker.png`.
- After the UI run, an independent SQL comparison against the original fixture
  verifies every retained workspace field, its original project binding, the
  added/removed selections and unrelated organization selection. The interactive
  fixture process terminated before its final cleanup assertion; this independent
  check supplies the post-UI storage evidence instead.
  `.context/cloud-repositories-retention-proof.json`.
- Native app, browser wasm, host server and API executable builds pass:
  `.context/cloud-repositories-{native,web,host,api}-build.log`.

The UI fixture deliberately has no payment writer/provider credentials, so its
billing section displays the existing read-only/unavailable state. Live GitHub,
live billing and Mac acceptance are not established by this run. Remaining work
includes GitHub enrollment/authority in Rust, image build/rebuild operations,
complete Cloud settings and full screen parity.


### Guest image preparation and Boxd snapshot primitives

The packaged Rust guest now supports `prepare-image --plan <file>` for an isolated
build machine. It prepares bare repository caches, validates branch heads and Git
object integrity, bounds aggregate cache size and publishes an identity-bound
manifest after credential cleanup. Structured progress/logs retain build/project
IDs and actual phases without copying Git diagnostics or secret values. The
manifest declares the actual Rust RPC/storage/session contracts; no conversation
engine or enrolled runtime storage is initialized by this command.

Git commands ignore inherited account configuration and credential helpers. Private
GitHub access uses explicitly supplied mode-0600, single-link, caller-owned token
files plus a temporary askpass helper. Tokens are refused for other production
hosts. Consumed secrets/helper files are removed on success and Git failures;
remaining secret files prevent readiness. Existing user history, enrollment data,
account credentials and SSH identity are refused instead of being cleared.
Repository caches must be bare and retain the expected credential-free origin.

Retries bind to the original plan digest/build ID, recheck the original manifest,
repository paths, commit heads and object integrity, and refuse changed/corrupt
ready data. A failed new preparation does not overwrite the previous ready
manifest. A cached image can materialize an independent working tree with the
original Git objects and bytes.

The Boxd adapter now implements the SDK 0.2.14 snapshot create/read/delete contracts
and recorded-machine snapshot provenance. Capture is one mutation attempt: a lost
or malformed response remains uncertain, and metadata polling never replays it.
Names must be unique per build because Boxd increments a reused name's version.
An explicit provenance check compares the restored machine's source kind, immutable
snapshot ID and version; legacy missing provenance is refused. Snapshot reads do not wake compute.

Verification:

- Two guest workflows pass with a real authenticated local Git HTTP server:
  private clone, bare-cache integrity, idempotent retries, temporary
  credential cleanup, failed rebuild preservation, independent working-tree
  reopen and corrupted-cache refusal. Enrolled user files remain byte-identical
  after rejected preparation. `.context/cloud-image-guest-tests.log`.
- Four Boxd transport checks pass, including independently SDK-encoded snapshot
  requests/responses, exact capture/read/delete framing, source ID/version
  validation and no automatic replay after uncertain capture. Existing machine
  and usage operations still pass. `.context/cloud-image-snapshot-tests.log`.
- The guest executable builds (`.context/cloud-image-guest-build.log`). Its actual
  `prepare-image` command clones a real public HTTPS repository, reopens the same
  manifest idempotently, and materializes its original commit and README from the
  cache. `.context/cloud-image-guest-cli-acceptance.json` and
  `.context/cloud-image-guest-cli.log`. No paid provider compute is used.

The control-plane build admission, scoped private-repository grant issuance,
durable worker scheduling, capture/promotion/cleanup and account image UI are
still open. These verified guest/provider pieces are prerequisites for that worker;
they do not establish a complete image build/rebuild workflow or live Boxd
acceptance. The existing ready-image records remain untouched.


## Owned account image builds

Rust now serves the pinned `GET /v1/cloud/image` and `POST /v1/cloud/image/build`
contracts. Build admission serializes the account catalog, verifies entitlement and
capacity, pins included repositories and configuration, and durably enqueues one
idempotent operation. Administration authorization gates builds; members cannot
read build logs. Status reads never allocate machines. A failed rebuild retains
its previous ready generation, including when that generation is outside the last
12 history entries.

The separate `image-worker` reconciles named allocations after uncertain provider
responses, prepares credential-free guest caches, captures an immutable named
snapshot, allocates a verifier with checked source/version, and reruns guest cache
integrity validation before promotion. It attributes both temporary machines to
the existing build cost ledger before retirement. Price-schedule-backed capacity
reservations use the existing account billing lock; they never emit charges.
Original `box` snapshot records and storage settlement remain compatible with
Rust's `boxd` execution provider. Successful promotion retains the new image and
marks previous storage superseded; durable cleanup removes superseded snapshots.
Failed preparation or verification cannot promote an image over the usable one.

Private GitHub builds use a fresh original-authority catalog authorization followed
by repository-restricted GitHub App installation tokens. The worker rechecks the
account installation linkage, suspension and allowed repositories before issuance.
The authorization proof expires after 15 minutes. Tokens stay out of operation
records and manifests; uploads use a root-only directory, and runner exit removes
staged and installed credentials before publishing completion. Guest preflight
requires the installed Rust runtime, Git and Claude CLI.

New chats can select any repository in an account image. The runtime installs a
private checkout from the pinned bare cache through a durable staging receipt,
restores the GitHub origin URL, and resolves non-default remote branches. Existing
ready journals preserve newer commits and changed files on reopen. Occupied
checkout destinations are refused. Legacy single-project Rust images retain
existing launch behavior.

Configuration (all disabled by default):

- Apply additive `crates/cloud/migrations/0007_images.sql` alongside the existing
  coordination migrations. Original project/build/snapshot/price tables must exist.
- Set `ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS` to explicit account owners. Transfer the
  original image and project catalog writers before setting their `image-catalog`
  and `project-catalog` ownership markers to Rust. Individual builds receive their
  own generation fence. Capacity reservations additionally require the separate
  `billing` marker to be Rust-owned. Enabling images transfers no billing/export
  ownership automatically; complete those handoffs independently.
- Set `ZUSE_CLOUD_RUST_IMAGE_BASE_SNAPSHOT`, positive
  `ZUSE_CLOUD_RUST_IMAGE_BASE_VERSION` and `ZUSE_CLOUD_RUST_IMAGE_TEMPLATE_VERSION`.
  The configured Boxd base must contain the packaged guest, installed Claude CLI,
  Git/Python/sudo/flock and the `zuse` user, without enrolled runtime/account data.
- Configure `ZUSE_CLOUD_DATABASE_URL`, `BOXD_API_KEY` and optional `BOXD_ORG` /
  `BOXD_BASE_URL`. Existing original provider price schedules are authoritative;
  an explicit `boxd` schedule takes precedence over the legacy `box` alias.
- Private repositories also require `ZUSE_CLOUD_LEGACY_GITHUB_URL`, `GITHUB_APP_ID`
  and `GITHUB_APP_PRIVATE_KEY` with the original app installation permissions.
- Run `zuse-cloud-api image-worker` separately, or enable the compose `images`
  profile. `image-worker --once` processes one due step. API/worker startup performs
  no ownership transfers, schema migrations or customer data imports.

Verification:

- Isolated PostgreSQL + Axum admission/status: concurrent idempotent requests,
  organization-member build denial, repository selection, reservations without
  ledger charges, separate billing ownership fencing, lost create/capture responses,
  verified promotion, successful supersession/storage cleanup, corrupt read-back
  preserving the previous ready generation, build ownership transfer and scoped
  log redaction. A ready generation remains available outside the last 12 attempts.
  `.context/cloud-image-controller-final.log` (3 focused checks, including the GitHub
  test below). Provider compute is deterministic in this acceptance.
- A real local HTTP GitHub fixture verifies the RS256 App JWT, API version and exact
  repository/content-read restriction. Expired admission cannot mint another token;
  issued secrets are absent from the serialized operation record.
- Seven guest checks pass, including real authenticated Git cloning, secret cleanup,
  cache integrity, non-default branch checkout, original GitHub origin, occupied
  destinations/dangling symlink refusal and preservation of advanced commits and
  changed bytes on reopen. `.context/cloud-image-guest-verified.log`.
- Cloud regression suite: 27 passed, eight environment-specific checks skipped.
  `.context/cloud-image-cloud-regressions.log`. API and guest binaries build;
  disabled `image-worker --once` exits without opening the database. Packaged guest
  revalidation of the original HTTPS Hello-World cache passes:
  `.context/cloud-image-guest-cli-reverified.json`.

Live Boxd base-image provisioning, image administration UI, custom snapshot imports,
provider-key workflows and full lifecycle/cutover acceptance remain open. This is
not Milestone C completion or a production rollout.


## Shared image administration UI

Native Cloud settings and browser Cloud settings now render the same repository /
image component. It reuses existing settings frames, compact rows, buttons and SVG
icons. The readiness row distinguishes missing, outdated, building, ready, failed,
authentication and unavailable states. Build/update, rebuild and uncertain-request
retry use the actual account-image API. An uncertain request retains its admission
key until acknowledged; account changes cancel pending callbacks and clear scoped
state. Account-host disconnection clears pending controls and preserves the retry
key. Polling uses bounded error retries and control-plane reads, never an execution
connection. Repository changes refresh image readiness.

Build history initially shows five attempts and can reveal the remaining entries.
Each attempt expands logs, Copy logs and pinned repository/runtime settings;
active generation, result, timestamp and duration remain visible in the row. The
host owns authentication, checks expected account/organization before effects and
exposes shared `CloudImage` / `CloudImageBuild` RPC methods. Viewports receive no
bearer token. Organization members cannot activate administrative controls. Native
appearance changes now update the shared palette, fixing dark cards in light Cloud
settings. Complete reference styling and full-screen parity remain Milestone F.

Verified:

- Compiled browser -> host -> Axum -> isolated PostgreSQL -> durable deterministic
  machine worker: failed-image retry/update and clean rebuild both progressed to
  Ready. Each user action admitted one key. Foreign account/organization requests
  were rejected; the request trace contains no `CloudEnvironment`.
  `.context/cloud-images-browser-acceptance.json`,
  `.context/cloud-images-browser-proof.json`; repeatable scenario:
  `scripts/acceptance/cloud-images.mjs`.
- Final native Linux build: the actual Rebuild button created one additional
  attempt; API observations progressed Building -> Ready with a new active
  generation. History pagination, log/settings expansion and light palette were
  inspected in the rendered app. `.context/cloud-images-native-acceptance.json`.
- The fixture test completed successfully after browser/native interaction and
  verified the original workspace row was unchanged and no builder machines were
  left running. `.context/cloud-images-ui-api.log`.
- Final native and browser builds passed:
  `.context/cloud-images-native-final-build.log`,
  `.context/cloud-images-web-final-build.log`; host:
  `.context/cloud-images-host-build.log`. Final rendered artifacts:
  `.context/cloud-images-native.png` and `.context/cloud-images-browser.png`.

These checks use actual HTTP/RPC and PostgreSQL, with a deterministic provider.
They do not establish live Boxd or Mac acceptance. Image deletion controls, custom
snapshot import/selection, other provider administration, complete settings routes
and live migration/cutover remain open. No milestone is marked complete here.


## Retained image deletion and source fencing

The pinned `POST /v1/cloud/image/delete` contract now admits account-owned saved
image deletion under administration, image catalog and billing ownership checks.
The existing snapshot record becomes the durable cleanup outbox. Admission records
one storage cutoff and preserves the checkpoint/fractional remainder for the billing
owner. Repeating admission does not move that cutoff. The image worker retries
unavailable cleanup, reconciles uncertain deletion through provider inspection and
marks Deleted only after successful deletion or confirmed absence. Error codes and
Deleting state remain visible until cleanup completes. Deferred cleanup does not
starve new image builds. Builds/history and workspace data are retained.

Create/allocation and archive restore admission share the snapshot lifecycle fence
with deletion and promotion. An accepted allocation or restore pins its source
while its target machine ID is uncertain. Both explicit deletion and superseded
image cleanup respect those pins. New launches exclude deleting/deleted images.
Restore selection also understands account images containing multiple repository
caches and the original `box` snapshot registry alias for Rust `boxd` execution.

The shared native/browser panel shows saved image storage and its actual billing
activation, a focused confirmation dialog, contained Escape/Enter/Space shortcuts,
and real cleanup progress/errors. Cancelling sends no mutation. Confirmation uses
the exact reviewed snapshot ID; account changes clear the dialog and pending
callbacks. The dialog explains that workspace data is preserved and another image
is needed for new launches.

Verification:

- Three focused checks pass, including real PostgreSQL/Axum authorization, busy
  builds, unresolved allocation pins, concurrent admission fencing, transient
  provider failure, lost delete response without a repeated physical mutation,
  idempotent cutoff, retained history and unchanged workspace contents. Connected-
  provider images refuse the platform credential path until their original
  provider connection adapter is available. Original
  storage settlement charges through the cutoff once and preserves fractional
  accounting; new builds advance while old deletion is unavailable.
  `.context/cloud-image-delete-final-tests.log`.
- Authenticated create/idempotency/capacity regression passes:
  `.context/cloud-image-delete-creation-regression.log`.
- Packaged guest/archive acceptance restores a retired workspace into a fresh
  generation and continues its original conversation:
  `.context/cloud-image-delete-restore-regression.log`.
- Compiled browser -> host -> Axum/PostgreSQL -> deterministic provider: Escape
  cancels, keyboard confirmation admits one deletion, polling observes removal,
  history remains, redelivery is safe and no `CloudEnvironment` call occurs.
  `.context/cloud-image-delete-browser-acceptance.json` and corresponding proof.
- Native Linux: actual Build -> Ready -> cancel -> keyboard-confirmed Delete ->
  Not built, with history preserved. `.context/cloud-image-delete-native-acceptance.json`.
  The UI fixture completed successfully, confirming the original workspace row was
  unchanged and no build machines remained: `.context/cloud-image-delete-ui-api.log`.
- Native/browser and API builds pass. UI artifacts:
  `.context/cloud-image-delete-browser.png`, `.context/cloud-image-delete-native.png`.

Provider compute in these tests is deterministic; live Boxd and Mac acceptance
remain open. Custom snapshot import/selection, provider administration, complete
UI parity and migration/cutover remain on the milestone checklist. This chunk does
not complete Milestone C or authorize production ownership changes.
