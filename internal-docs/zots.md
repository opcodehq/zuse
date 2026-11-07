# Persistent Zots and application control

Zots coordinate ordinary conversations. A worker conversation keeps its own
environment, provider, model, checkout, history and nested agents. The Zot owns
its purpose, repository scope, instructions, decisions, digest and reports.

## Authority and storage

Each Zot has one home engine. Its private state is stored under that engine's
resolved profile in `zots/state.sqlite`; its coordinator checkout is a private
directory beside that database. Local profiles require no hosted account.
Account profiles do not share this database with local profiles.

Commands require a caller-minted command ID. The input, result, state mutation,
file versions and delivery intents commit in one SQLite transaction. Reusing
an ID with different input fails. Retrying identical input returns the original
result, even when later commands changed the Zot. Revision and file-version
preconditions reject stale edits. Restoring a file creates a new version.

Unresolved decisions are structured records. Rewriting `DIGEST.md` never resolves
them. Instructions, worker rules, notes and digest are separate versioned files.
Files and decisions remain in private storage after deleting the coordinator;
worker conversations and repository files are preserved.

The coordinator runs in plan mode in its private workspace. It delegates repository
writes through scoped Zot tools. Worker requests use the existing conversation
queue, isolated worktree creation and provider permissions. Model reports cannot
grant authority. Agent follow-ups always debit the configured automatic limit;
an agent cannot label a follow-up as human-authored to avoid that limit.

## Delivery and supervision

The Rust supervisor reconciles durable delivery intents, current session records
and completion markers. Pending dispatches respect the Zot concurrency limit.
Sending and unacknowledged execution reserve capacity until reconciled. Commands
enter the existing conversation queue with stable identities; the destination's
processed ledger prevents duplicate execution after acknowledgment loss.

Worker results, questions, failures and placement problems produce deduplicated
durable events. Events wake an idle coordinator once per event/conversation;
notifications remain available until explicitly acknowledged. Regular monitoring
does not invoke a model. Completed turns are reports, not proof that an objective
is complete. Stale or absent execution status is unknown.

Remote workers use authenticated paired-engine links. Their executing engine
owns a persistent membership claim and worker instructions. Claims reject a
second Zot home. Versioned release tombstones reject delayed old claims, and
home-side release intents retry after reconnect. Delivery waits for a successful
destination claim. Remote instructions refresh before new orchestrated delivery
and are applied by the worker host on subsequent turns.

Pause prevents new automatic dispatch. Stop cancels pending delivery and records
worker interruption intents. Archive also archives worker conversations. Fresh
coordinator conversations preserve worker membership, files and decisions and
retain links to previous coordinator history. Home transfer, schedules and
external-event wake-ups are outside this version.

## Public control surface

`ControlInfo` reports the API version, operation catalog and canonical Zot command
schema. Rust generates the npm SDK's TypeScript contracts. The application UI,
SDK, CLI and MCP use the same engine operations. Provider model transports remain
separate from application control.

`zuse-control` links the RPC client and protocol, with the relay implementation
disabled. It never starts an engine. It supports `info`, `call`, `watch` and
`mcp`. `--discovery` selects `<profile>/zots/control.json`; the default searches
the local desktop/server profiles. Discovery files contain owner credentials and
must remain private. For cloud control, use an explicit discovery file and a
`wss://.../api/control` endpoint.

The cloud control endpoint forwards only authenticated `ControlCall` envelopes;
browser cookies and raw engine operation names grant no control access there.
Engine-injected agent credentials are bound to a live originating coordinator
and its Zot. Environment identity strings are attribution, not authorization.
The existing loopback RPC remains available for installed application clients.

Existing chat tools remain available for ordinary conversations. Coordinator
tool catalogs expose inspection plus scoped Zot operations, including versioned
file reads/writes, delegation and worker reports. The native harness receives
the same scoped context as subprocess providers.

## Installation and publishing

`scripts/package-control.mjs` produces a `control/` directory containing the
platform binary, npm tarball and version/checksum manifest. Include that directory
alongside the executable in native application and cloud runtime packages.
The Control artifacts workflow builds matching Linux and macOS artifacts and
can publish the npm package through the protected `npm` environment.

At startup, packaged applications verify the bundle and probe the binary version,
stage it in `~/.zuse/control/<version>`, and atomically replace the managed CLI
link. The previous binary installation remains available. Existing global npm
SDK installations update from the bundled tarball with lifecycle scripts disabled.
Project manifests and lockfiles are never edited. `ControlUpdateStatus` reports
failures; `RetryControlUpdate` retries them. Source builds without a bundle report
an unmanaged installation.

## Verification boundaries

Behavior tests cover receipts and restart, version conflicts, retained decisions,
membership exclusion, release tombstones, bounded follow-ups, cancellation,
profile separation, real isolated git worktree execution, and deduplicated reports.
SDK tests cover authentication envelopes, cancellation, connection failure without
replay, TLS requirements and bounded subscription buffers. Update tests cover
checksum failures, previous-version retention and unchanged project lockfiles.

Authenticated cloud/provider smoke tests, a full native UI walkthrough and actual
npm publication remain release gates. An isolated Rust UI type check cannot
replace the native WebKitGTK/browser build or desktop interaction acceptance.
