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

The coordinator runs in its private workspace and delegates repository edits. It delegates repository
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

## Issues and linked delivery

A Zot owns a durable issue backlog in its home profile. The Overview groups
issues by status, followed by threads, pull requests and Drive documents. The
Issues page supports search, assigned-to-Zot and mention filters, grouped/collapsible
statuses and issue details. Native and browser clients embed the same GPUI
component for these surfaces and for attributed Zot message headers.

Issues have stable caller-minted IDs, sequential display numbers within their
Zot, title, description, status, priority, assignment, optional calendar due date,
labels and relations to other issues in that Zot. Assignment is to the owning
Zot; threads are linked work, not assignees. Issues are local application records,
not synchronized copies of an external issue tracker. The current surface does
not move issues between Zot homes.

The public `ZotCommand` actions are:

- `createIssue` and `editIssue`: `issueId` and `details` (`title`, `description`,
  `status`, `priority`, `assigned`, `dueDate`, `labels`, `relations`).
- `linkIssueThread`: `issueId` and a `chatId` belonging to this Zot.
- `linkIssuePullRequest`: `issueId`, canonical HTTPS GitHub PR `url`, and `title`.
- `unlinkIssueThread` and `unlinkIssuePullRequest`: remove a link using its
  `chatId` or `url`; unlinking does not release or stop a worker.
- `commentIssue`: `issueId` and `text`.
- `delegate`: optional `issueId` alongside the existing worker, model and base.
  The issue must be assigned and open. Linking the new thread, moving the issue
  to `inProgress`, worker membership and durable delivery commit together.

These use the same revision preconditions and command receipts as every other
Zot mutation. Replays preserve the original result and do not duplicate comments,
links, notifications or worker launches. Old stored Zots default to an empty
issue list. Issue activity survives engine restart.

Human assignment, edits, comments and PR links wake an active coordinator through
durable events. Coordinator-authored mutations do not wake themselves. Paused
Zots retain these events until resumed. Worker reports enter linked issue activity
once; failures, unavailable placement and input requests mark open issues blocked.
A settled worker turn never automatically marks an issue done. The coordinator
must inspect verification, link the PR when applicable, and explicitly advance
review/completion. Terminal issues are not reopened by delayed worker reports.

Local and paired workers receive their linked issue IDs in their host context.
Their live, engine-issued credentials allow only comments and PR links on issues
linked to their own thread. The Zot home rechecks membership and issue scope;
workers cannot edit assignment/status, create unrelated issues or close work.

Zot-delivered messages store a display envelope with the sender's name, avatar
and coordinator chat identity. Both clients render this above a left-aligned
message bubble. The envelope is display context, not a permission grant. Existing
chat-to-chat attribution remains supported. Model-free activity notices carry a
separate display flag and render as compact rows. Host instructions and linked
issue context stay outside the visible message body. Reports retain the final
assistant text, capped at 16,384 characters with an explicit truncation marker.
Coordinator activity uses its stable chat identity so renames and fresh
conversations retain readable authorship. Linked PR pages provide issue/thread
backlinks and keep their navigation inside the Zot inspector.

Checks cover persistence/retry, stale revisions, atomic delegation/rollback,
link validation, scoped worker writes, legacy records and report deduplication.
The real-engine acceptance scenario uses a deterministic ACP coordinator and
worker, exercises isolated checkout and scoped writes, and verifies clean visible
kickoff text, hidden issue context, activity notices and explicit acceptance.
Browser and Linux native walkthroughs cover the issue surface, attributed
messages and linked PR navigation. The native verification build excludes the
unused WebKit helper; it does not verify WebKit integration or macOS rendering.
