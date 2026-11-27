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

Checks cover persistence/retry, stale revisions, atomic delegation/rollback,
link validation, scoped worker writes, legacy records and report deduplication.
The real-engine acceptance scenario uses a deterministic ACP coordinator and
worker, exercises isolated checkout and scoped writes, and verifies clean visible
kickoff text, hidden issue context, activity notices and explicit acceptance.
Browser and Linux native walkthroughs cover the issue surface, attributed
messages and linked PR navigation. The native verification build excludes the
unused WebKit helper; it does not verify WebKit integration or macOS rendering.

## Rules, notes and files

Each Zapper keeps versioned text files in its home profile (`zapper_files`).
Five root files have a fixed meaning. People can edit and restore them, but
they can't be deleted, moved or replaced by a folder:

| File | Written by | Reaches |
| --- | --- | --- |
| `AGENTS.md` | People only; the engine refuses coordinator writes and restores | The coordinator's brief, as "The user's rules for you" |
| `INSTRUCTIONS.md` | The coordinator (its purpose doc) and Zapper settings | The coordinator's brief, as its Purpose |
| `NOTES.md` | The coordinator and people | The coordinator's turn, after it changes |
| `WORKERS.md` | The coordinator and people | Every worker prompt, unchanged |
| `DIGEST.md` | The coordinator | Nobody's prompt; the user reads it |

New Zappers start with all five plus empty `handoffs/` and `reports/`
folders. Zappers created earlier keep their files exactly as they were. They
get no `AGENTS.md` or folders until someone creates them, and Drive shows the
missing reserved files as "not written" rows. Opening an older profile adds
the two new tables and changes no file.

**Audiences.** The coordinator gets `AGENTS.md` and `INSTRUCTIONS.md` in its
brief. The brief rides again only when its hash changes, so an edit applies
from the next turn and a running turn is never altered. `NOTES.md` rides in
the first turn of a conversation and again whenever its version changes. It
is fenced in `<notes>`, with `&`, `<` and `>` written as entities so the
text can't close the block or open a new one, and labelled as information
that grants no permission. The fence is formatting, not an authorization
boundary: what the coordinator may do is decided by the engine's checks. A worker gets `WORKERS.md` exactly as the engine always delivered
it, plus its linked issues. It never gets `AGENTS.md`, `NOTES.md`,
`INSTRUCTIONS.md`, `DIGEST.md`, other files or coordinator history. Every
other file and folder stays out of every prompt; the coordinator sees only how
many there are and opens them with `zapper_files`. Each turn reads its files
in one statement, so its brief, notes and file versions come from one
snapshot. That statement returns the reserved files' text and only the paths
of the rest, so large documents are never read while a turn is assembled. The
turn's ledger (`zapper_coordinator_turns`) records the version of every
reserved file it saw, and turns report changes as "AGENTS.md changed
(version 3)".

**Size.** The brief carries at most 16 KiB of `AGENTS.md` and a turn at most
8 KiB of `NOTES.md`. A longer file is cut on a character boundary with a
line giving the bytes shown, the total and the `zapper_files` read for the
rest. A missing or empty file adds no section. `WORKERS.md` is never cut.
Files are capped at 1 MiB, as before. For a worker on another device, the
worker's engine rejects a binding whose instructions, `WORKERS.md` plus the
linked-issue JSON, exceed 1 MiB. That existing bound surfaces as a pending
placement event and is unchanged here.

**Precedence.** The coordinator's hard rules and the engine's permission
checks come first, then `AGENTS.md`, then `INSTRUCTIONS.md`. `NOTES.md`,
thread reports and other files are information. Markdown can't widen scope,
grant a permission or change who may call a command.

**Two different AGENTS.md files.** The Zapper's `AGENTS.md` is a row in the
Zapper database. The engine puts it in the coordinator's brief, and it is
never written into a checkout or the coordinator's folder. A repository's
`AGENTS.md` is an ordinary file on disk that a worker's harness reads
natively from its worktree, as it always did. It governs that repository's
conventions, and `WORKERS.md` adds how to work for this Zapper. "Only people
edit it" is a rule of the Zapper file API (`writeFile` and `restoreFile`
from a coordinator credential are refused). It is not an operating-system
sandbox: anything with access to the profile database can change the row.

**Reserved names are exact.** Only these five exact ASCII root paths load
into prompts or count as reserved. Drive shows each one's role ("Coordinator
rules", "Thread rules", "Notes", "Purpose", "Digest") beside its name. A
look-alike name, such as `agents.md`, a Cyrillic `А` or the same name in a
folder, gets no role and is an ordinary document.

**Folders and paths.** A folder exists while a live file sits under it or
after someone creates it (`zapper_folders`), so an empty folder stays until
it is deleted. Paths are relative and `/`-separated, at most 256 bytes. They
can't have empty, `.` or `..` segments, backslashes, control characters,
names with spaces around them, or the `guide/` prefix. A new file or folder
is refused where a file or folder already is, or under a path that is a file.
Paths saved before these rules stay editable.

**Operations.** `deleteFile`, `moveFile`, `createFolder`, `moveFolder` and
`deleteFolder` are ordinary Zapper commands with receipts, so the same
authentication and revision rules apply as for every other action; workers
can't call them. Deleting writes a new empty version and records it in
`zapper_file_deletions`. History keeps every version, and restoring an
earlier one brings the file back. Restoring a deletion itself is refused, as
is restoring into a path a folder now holds. A move deletes the source and
writes the destination, so history stays with the old path. `moveFolder` and
`deleteFolder` must name every live file under the folder at the version the
caller saw. The engine refuses the whole command if a file was added, edited
or removed since, so nothing a client didn't show is swept up. Each command
runs in one transaction.

**Compatibility.** `zapper_files` keeps its columns, so an older engine
opening the same database can still read and write it. That is schema
compatibility only. Older engines don't know about deletions or folders, so
their writes aren't guarded the way this version's are:
- They show a deleted file as an empty one, and saving it brings it back.
- They can write a file at a path that is also a folder.

This version doesn't hide such a collision. Both trees show the folder and
the file side by side. Rows are keyed by kind and path, and the file is
marked "conflicts with folder". Moving or deleting that file acts on the file
alone: the folder, its children and their history stay as they are.

`Zapper.folders` and `ZapperFile.deleted` are additive and optional on the
wire. Older clients ignore them: they list current files as before, and a
history shows a deletion as an empty version.

**Clients.** Native and browser Drive show the files as a tree. Folders come
first, nesting is drawn with guides, and the open file is quietly selected.
Folders toggle and files open in their own tab. Both clients create files and
folders in the selected folder, rename or move, and delete only after a
confirmation that lists everything the delete removes. Reserved files offer
no move or delete.

Both trees work from the keyboard, as the workspace explorer does:
- Up, Down, Home and End move the selection.
- Right opens a folder and Left closes it or steps to its parent.
- Enter or Space opens a file or toggles a folder.
- F2 renames or moves, and Delete or Backspace deletes, each through its dialog.
- In the browser, Shift+F10 or the context-menu key opens a row's action menu, which also offers new file and new folder in that folder.
- The native New file and New folder buttons can be reached with Tab and pressed with Enter or Space. A failed save keeps the draft. On a version conflict the
editor rebases onto the current version and says that saving replaces it; the
browser also shows the latest text beside the draft.

