# Persistent Zappers and application control

Zappers coordinate ordinary conversations. A worker conversation keeps its own
environment, provider, model, checkout, history and nested agents. The Zapper owns
its purpose, repository scope, instructions, decisions, digest and reports.

## Authority and storage

Each Zapper has one home engine. Its private state is stored under that engine's
resolved profile in `zappers/state.sqlite`; its coordinator checkout is a private
directory beside that database. Local profiles require no hosted account.

Profiles from before Zots were renamed Zappers kept this state in `zots/`.
On first open the engine copies the database and control token into `zappers/`,
leaving the originals in `zots/` so an older build still opens them, and
renames the `zot_*` tables and `zot` columns in the copy (`zappers/migrate.rs`).
Zappers created before the rename keep their coordinator folder under `zots/<id>`,
because harnesses key saved conversations by working directory. Engines keep
accepting the old method, field, tool and chat-id names (`zappers::legacy`) for
one release.
Account profiles do not share this database with local profiles.

Commands require a caller-minted command ID. The input, result, state mutation,
file versions and delivery intents commit in one SQLite transaction. Reusing
an ID with different input fails. Retrying identical input returns the original
result, even when later commands changed the Zapper. Revision and file-version
preconditions reject stale edits. Restoring a file creates a new version.

Unresolved decisions are structured records. Rewriting `DIGEST.md` never resolves
them. Instructions, worker rules, notes and digest are separate versioned files.
Files and decisions remain in private storage after deleting the coordinator;
worker conversations and repository files are preserved.

The coordinator runs in its private workspace and delegates repository edits. It delegates repository
writes through scoped Zapper tools. Worker requests use the existing conversation
queue, isolated worktree creation and provider permissions. Model reports cannot
grant authority. Agent follow-ups always debit the configured automatic limit;
an agent cannot label a follow-up as human-authored to avoid that limit.

## Delivery and supervision

The Rust supervisor reconciles durable delivery intents, current session records
and completion markers. Pending dispatches respect the Zapper concurrency limit.
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
second Zapper home. Versioned release tombstones reject delayed old claims, and
home-side release intents retry after reconnect. Delivery waits for a successful
destination claim. Remote instructions refresh before new orchestrated delivery
and are applied by the worker host on subsequent turns.

Pause prevents new automatic dispatch. Stop cancels pending delivery and records
worker interruption intents. Archive also archives worker conversations. Fresh
coordinator conversations preserve worker membership, files and decisions and
retain links to previous coordinator history. Home transfer, schedules and
external-event wake-ups are outside this version.

## Public control surface

`ControlInfo` reports the API version, operation catalog and canonical Zapper command
schema. Rust generates the npm SDK's TypeScript contracts. The application UI,
SDK, CLI and MCP use the same engine operations. Provider model transports remain
separate from application control.

`zuse-control` links the RPC client and protocol, with the relay implementation
disabled. It never starts an engine. It supports `info`, `call`, `watch` and
`mcp`. `--discovery` selects `<profile>/zappers/control.json`; the default searches
the local desktop/server profiles. Discovery files contain owner credentials and
must remain private. For cloud control, use an explicit discovery file and a
`wss://.../api/control` endpoint.

The cloud control endpoint forwards only authenticated `ControlCall` envelopes;
browser cookies and raw engine operation names grant no control access there.
Engine-injected agent credentials are bound to a live originating coordinator
and its Zapper. Environment identity strings are attribution, not authorization.
The existing loopback RPC remains available for installed application clients.

Existing chat tools remain available for ordinary conversations. Coordinator
tool catalogs expose inspection plus scoped Zapper operations, including versioned
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

A Zapper owns a durable issue backlog in its home profile. The Overview groups
issues by status, followed by threads, pull requests and Drive documents. The
Issues page supports search, assigned-to-Zapper and mention filters, grouped/collapsible
statuses and issue details. Native and browser clients embed the same GPUI
component for these surfaces and for attributed Zapper message headers.

Issues have stable caller-minted IDs, sequential display numbers within their
Zapper, title, description, status, priority, assignment, optional calendar due date,
labels and relations to other issues in that Zapper. Assignment is to the owning
Zapper; threads are linked work, not assignees. Issues are local application records,
not synchronized copies of an external issue tracker. The current surface does
not move issues between Zapper homes.

The public `ZapperCommand` actions are:

- `createIssue` and `editIssue`: `issueId` and `details` (`title`, `description`,
  `status`, `priority`, `assigned`, `dueDate`, `labels`, `relations`).
- `linkIssueThread`: `issueId` and a `chatId` belonging to this Zapper.
- `linkIssuePullRequest`: `issueId`, canonical HTTPS GitHub PR `url`, and `title`.
- `unlinkIssueThread` and `unlinkIssuePullRequest`: remove a link using its
  `chatId` or `url`; unlinking does not release or stop a worker.
- `commentIssue`: `issueId` and `text`.
- `delegate`: optional `issueId` alongside the existing worker, model and base.
  The issue must be assigned and open. Linking the new thread, moving the issue
  to `inProgress`, worker membership and durable delivery commit together.

These use the same revision preconditions and command receipts as every other
Zapper mutation. Replays preserve the original result and do not duplicate comments,
links, notifications or worker launches. Old stored Zappers default to an empty
issue list. Issue activity survives engine restart.

Human assignment, edits, comments and PR links wake an active coordinator through
durable events. Coordinator-authored mutations do not wake themselves. Paused
Zappers retain these events until resumed. Worker reports enter linked issue activity
once; failures, unavailable placement and input requests mark open issues blocked.
A settled worker turn never automatically marks an issue done. The coordinator
must inspect verification, link the PR when applicable, and explicitly advance
review/completion. Terminal issues are not reopened by delayed worker reports.

### Ownership and lifecycle

Every issue has at most one owner: the Zapper, one of its threads, or the user
(`details.owner`). Issues saved before owners existed read `assigned: true` as
the Zapper; `assigned` stays in step with the owner for older clients. Issues
also carry `doneCriteria`, `outOfScope` and an optional `parent` (one request
issue with a child per shippable change).

The engine keeps owner and status consistent. The user owning an open issue
means `needsYou`; anything else moving off `needsYou` returns to `inProgress`
when a thread is linked, else `todo`. `delegate` makes the new thread the owner.
`askUser` (`issueId`, `question`, optional `recommendation`) hands an issue to
the user with a question entry; a person's comment on a `needsYou` issue hands it
back to the newest linked thread still in the Zapper, else to the Zapper, and
wakes it. `closeIssue` (`issueId`, `outcome` merged or dropped, `note`) is the
only way the coordinator closes work; merged requires a linked PR.

Coordinator-authored changes need their evidence: new issues start in backlog
or todo, and todo needs `doneCriteria`; `inProgress` needs a linked thread,
`inReview` a linked PR; `needsYou` only through `askUser`; done and canceled only
through `closeIssue`; `doneCriteria` cannot be cleared. People editing in the UI
are not restricted. A thread report whose first lines say `Status: blocked` (or
`needs…`) marks the issue blocked and hands it back to the Zapper.

The coordinator prompt lists closed issues as one line each. Pending events reach
an idle coordinator as one message per supervision pass, with each event's id
and a shared ~48 KB budget; `zapper_events` keeps every report in full.

Local and paired workers receive their linked issue IDs in their host context.
Their live, engine-issued credentials allow only comments and PR links on issues
linked to their own thread. The Zapper home rechecks membership and issue scope;
workers cannot edit assignment/status, create unrelated issues or close work.

Zapper-delivered messages store a display envelope with the sender's name, avatar
and coordinator chat identity. Both clients render this above a left-aligned
message bubble. The envelope is display context, not a permission grant. Existing
chat-to-chat attribution remains supported. Model-free activity notices carry a
separate display flag and render as compact rows. Host instructions and linked
issue context stay outside the visible message body. Reports retain the final
assistant text, capped at 16,384 characters with an explicit truncation marker.
Coordinator activity uses its stable chat identity so renames and fresh
conversations retain readable authorship. Linked PR pages provide issue/thread
backlinks and keep their navigation inside the Zapper inspector.

### Conversation and routed answers

The issue detail is one conversation shared by the Zapper and its linked
threads. A **Threads N** control lists the coordinator and every linked thread,
marks the owner (the one accountable routing target), and opens each
conversation. Comments, worker reports and questions render as attributed cards
with replies nested one level deep; quiet events (status, owner, priority and
link changes) render as rows, and runs of three or more fold into one summary
line that never hides questions, blocked or needs-you changes. New updates store
typed `changes` (`field`, `before`, `after`, wire values); older entries keep
their text. Clients treat stored values as plain text. CI and merge events are
not persisted in Zapper issue data, so the timeline does not show them.

A person's comment (owner-authenticated scope only; coordinator and worker
credentials always carry an actor) goes straight to one thread only when it
answers that thread explicitly:

- a reply to an entry the thread wrote (a report or a comment), or
- an answer to the open question on a `needsYou` issue when `askUser` recorded
  the live thread that held the issue (`thread`), the question is still the
  newest one, and the comment's `expectedRevision` is not older than the
  issue's details revision.

Anything else (ordinary comments, untied or stale answers, replies to the
Zapper) wakes the coordinator as before; untied and stale answers record why in
`delivery.detail`. A routed comment enqueues one `messageWorker` outbox row
with id `<commentId>:answer` in the comment's transaction, so receipts and the
destination ledger make retries, double submits and restarts deliver it at most
once. It waits while the thread is busy (it never steers a running turn), while
the Zapper is paused or a remote worker is offline, or for a free slot. Closed
issues, stopped or archived Zappers, threads that left the Zapper or the issue,
and threads the person stopped are never revived: the comment is kept with
`delivery.state = held` and a reason, and the same checks run again just
before dispatch (`cancelled`). An answer held from its thread hands the issue to
the Zapper, never to that thread or another one. Answers without an
`expectedRevision` stay with the Zapper and say a revision is required. The
coordinator's event shows the comment as plain text; the recipient lives in the
board's hidden `delivery` metadata, and the coordinator guide says not to relay
a delivered comment.

Each supervision pass holds the admission lock every command takes, so the
pre-dispatch check is the linearization point: a stop, release or close commits
either before it (the answer is cancelled) or after the message was queued
(the thread's turn is then interrupted like any other stop). A queued message
is not recalled.

`delivery.state` is `queued`, `retrying` (with the error), `sent` (the
thread's queue accepted it, which is not proof it was handled), `answered`,
`cancelled` or `held`. A report becomes the answer only by turn identity: the
transcript entry that started the settled turn ending at `lastCompletedTurn`
must be one of that thread's delivered `:answer` messages, so several queued
answers each match their own turn. If several delivered answers are
coalesced into one input turn, only the one that started it is linked; the
others stay `sent`. Delivery states are projected with a fixed
number of queries per Zapper. The link is stored on the comment
(`delivery.reply`) before the report is recorded, is retried while the
transcript hasn't caught up, and is never replaced. Interrupted, failed and
input-request turns are not linked; their comment stays `sent`.

The line above the comment box shows the owner's (else the coordinator's)
confirmed run state: working only from a fresh session report, "can't confirm"
for a busy-looking chat without one, queued with its slot, needs input, or a
queued or retrying reply. Comment drafts stay until the engine confirms them.

Checks cover persistence/retry, stale revisions, atomic delegation/rollback,
link validation, scoped worker writes, legacy records and report deduplication.
`scripts/acceptance/zapper-issue-conversation.mjs` drives a deterministic
coordinator and worker through a tied question, a top-level answer, an explicit
reply, a retried command and an engine restart, and checks the worker received
each answer once and its replies nested under them.
The real-engine acceptance scenario uses a deterministic ACP coordinator and
worker, exercises isolated checkout and scoped writes, and verifies clean visible
kickoff text, hidden issue context, activity notices and explicit acceptance.
Browser and Linux native walkthroughs cover the issue surface, attributed
messages and linked PR navigation. The native verification build excludes the
unused WebKit helper; it does not verify WebKit integration or macOS rendering.
