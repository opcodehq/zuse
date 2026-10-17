# Rust harness

`crates/harness` implements the durable Zuse agent kernel in Rust. The application registers it as the Zuse provider with account-backed model transport, native file/process tools, normalized chat events, approvals, uploads and persistent sessions. Component and application fixtures cover these paths; live authorization, hosted plugins and complete UI acceptance remain pending.

## Runtime authority

`Engine` owns a durable graph of one root and nested children, bounded to three running children and depth two. Rust schedules concurrent child requests, records mailboxes and idempotent delivery identifiers, enforces inherited read-only and plan modes, and delivers child outcomes before root synthesis. The model request budget is shared across the graph and includes compaction and broker retries. User continuation explicitly grants another turn's budget.

`Host` supplies model transport, native tools, checkpoint storage, instructions and application events. Native adapters use `Engine::open_native_with`, which acquires a per-root OS lease before reading its private journal and retains it until host cleanup succeeds. Root IDs map to hashed profile directories, and journal identities must match their owner. `Engine::open` remains available for fixtures and custom hosts that supply their own ownership authority. Tool definitions and permission decisions belong to the trusted host; model input cannot grant permission or call recovery APIs. `FileStore` writes private checkpoints using temporary files, fsync and atomic replacement. Invalid graphs and damaged journals are preserved for inspection.

Steering deliveries share a host admission epoch with Stop. New deliveries wait for Stop to settle; drivers discard cancelled frames even when transport delayed them past that boundary. The kernel persists cancellation tombstones for unconsumed mail, including explicit runtime termination, so reopening the journal cannot resurrect it. Consumed history remains intact. Failed cancellation persistence restores mailbox order. An unexpected stream disconnect preserves retryable mail rather than treating it as a user cancellation.

A tool's signed input identity and `started` receipt are persisted before execution. Its completed output is persisted before the tool-result message enters model history. Recovery reuses committed outputs; a started receipt with no outcome pauses execution. A trusted user control surface can call `resolve_unknown` with an inspected outcome or explicit retry approval. Approval is saved before continuation and rolled back in memory if storage fails. Approval itself never launches work. The application exposes this through user-only recovery RPCs and native/browser review controls; live UI acceptance remains separate.

Compaction retains the full journal, system instructions, original task and complete recent exchanges. Summaries create a durable boundary rather than deleting history. A summary that fails to reduce context pauses the run. Context sizes are estimates, not provider-reported billing measurements.

## Model transport

`AccountBroker` owns connection health and root affinity across children. Quota exhaustion can switch eligible connections; an authentication failure pauses for reauthorization. Additional physical requests debit the same Rust budget. Account-bound reasoning and provider metadata are removed from portable requests.

The SDK bridge is restricted to schema/message translation and model streaming. It neither executes tools nor schedules agents. Rust starts one lazy Node process, multiplexes concurrent requests, cancels individual requests without disrupting the others, and retires the process after idle time. Credentials cross a private pipe and are never process arguments or environment variables. Node 22 or newer is currently required; selecting and bundling the packaged runtime remains part of packaging work.

The pinned transport uses stateless streamed requests and validates its request field allowlist. Its protocol follows the [official subscription transport requirements](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations) and [streaming event documentation](https://developers.openai.com/api/docs/guides/streaming-responses). HTTP redirects are rejected. The bridge contains pinned SDK dependencies, a reproducible build, and retained third-party notices.

## Commands and evidence

```sh
cargo test -p zuse-harness
cd crates/harness/bridge
bun install --frozen-lockfile
bun run test
bun run build
```

The native-tool integration fixtures require `rg` on PATH in addition to the Node runtime used by bridge fixtures. Packaged ripgrep resolution remains part of application packaging.

Rust scenarios cover unknown-outcome recovery, explicit inspected retry, failed approval persistence, committed-output replay prevention, concurrent child execution and final synthesis, read-only enforcement, steering deduplication, shared budgets, compaction, checkpoint corruption, account fallback and affinity, and a real bundled Node process with concurrent mocked streams and cancellation. The SDK protocol fixtures cover text, tool calls, usage, request validation and incomplete streams. These are component and protocol fixtures; no live paid-model request or complete application harness session is claimed.

Native ownership fixtures exercise same-process and child-process exclusion, independent roots, stale PID stamps, preserved damaged journals, root identity validation, and release only after host cleanup. Application provider registration and live native session acceptance remain pending.

## Native commands

`processes::Processes` supplies owned background commands, stdin, bounded output
pages, readiness notices, and a durable completion outbox. Both desktop and server
entrypoints run `--zuse-command-host` before application initialization; the helper
uses a current-thread Rust runtime. Its specification and stdin travel over private
pipes rather than supervisor arguments. Commands run in a fresh process group.
An independent watchdog kills that group if its engine lease stops advancing,
even when output backpressure stalls async tasks. Closing the control pipe also
stops the group. These process groups are lifecycle ownership, not an OS sandbox.

`Engine::open_native_with` takes root ownership before its factory initializes
native tools or recovers process records. Each command supervisor also holds a
physical OS lease before the engine acknowledges launch. Recovery waits for that
lease to be released before marking a recorded running command interrupted; it
never signals a PID recovered from stale metadata and never reruns the command.
Runtime close fences new admissions and waits for admitted workers and metadata
writes before releasing root ownership.

Output is streamed into private files, capped at 64 MiB per command by default.
Memory holds bounded pipe frames and record metadata rather than entire outputs.
The default limits are eight running commands and 128 retained records. An
undelivered notification backlog blocks new admission rather than silently evicting
pending notices. Consumers must accept stable notice IDs durably and idempotently;
failed delivery remains in the outbox for retry. Root agents can inspect child
processes, while siblings cannot access each other's process IDs.

Eight process scenarios cover stdin ordering, UTF-8 cursors, ownership isolation,
limits/deadlines, descendant cleanup, output backpressure, launch acknowledgement, control EOF, recovery
lease ordering, shutdown races, and notification restart without command replay.
Native tool dispatch, application provider registration, and live conversation
acceptance remain pending.

## Native coding tools

`native_tools::NativeTools` supplies file/image reads, directory listing, ripgrep
search, hash-checked create/write/edit/multi-edit, supervised command controls,
and optional user-question and plan callbacks. One field description generates
the model schemas and validates execution arguments, including defaults and bounds.
Missing `expected_hash` is rejected; explicit null means exclusive file creation.

Checkout resolution permits internal symlinks and rejects paths escaping the
checkout. Directory descriptors and no-follow opens remain held through file
reads and atomic commits. Edits share OS locks across conversation roots, require
a fresh SHA-256, validate every replacement before writing, and preserve executable
modes. Conflicts retain the existing file. Large text results go into private
artifacts while retaining the path/hash metadata needed for subsequent edits.
File reads are bounded to 2 MiB; line selection avoids allocating an entry for
every line in the file.

The permission policy preserves runtime modes, plan mode, accept-edits behavior,
and forced approval for sensitive paths. Read-only children cannot override that
authority with tool arguments. Cancellation and dropped futures cancel pending
file work; close waits for blocking file jobs before releasing root ownership.
Successful background commands retain their agent's interruption lineage instead
of being cancelled when the initial tool invocation returns.

`NativeTools::attach` combines these tools with application model transport,
instructions, events and orchestration tools. Name collisions fail explicitly.
Create this composition inside `Engine::open_native_with` so recovery and tool
initialization occur under the root lease. Application callbacks must preserve
session ownership and permission-generation checks.

The complete harness suite now passes 32 tests. Eight native-tool scenarios
cover file conflicts and atomic edits, cross-root writes, symlink containment,
permissions, image/binary/large reads, process/search dispatch, questions/plans,
abort cleanup, and a durable-kernel coding workflow with real filesystem and
command execution. That workflow uses a scripted model; selectable application
provider registration, live model sessions, UI acceptance and packaged ripgrep
resolution remain pending.

## Account-backed model service

`engine::harness_models::HarnessModels` connects the engine's model accounts to
the shared `AccountBroker` and lazy SDK bridge. It discovers each account's live
model inventory, keeps provider-qualified model IDs, filters visibility and
media-only entries, and retains reported context/compaction limits. Eligibility
is checked per connection rather than inferred from a global model list.

Concurrent discoveries share each account's fetch. Inventory is cached for 15
minutes, transient failures for 15 seconds, with at most 128 cache slots. Explicit
refresh bypasses the cache. Requests reject a credential/provider mismatch before
sending a bearer token to either inventory or SDK transport. Account authorization
generations invalidate stale inventory and broker health after reconnect. The
broker retains root/child affinity and emits account-switch notices without using
an email address as the displayed account name.

Token renewal runs in a tracked owned task. Abandoning a model caller does not
discard a provider's rotated refresh token. Local shutdown drains an admitted
renewal and saves it before returning, while withholding credentials from retired
callers. Hosted storage retains its identity/cancellation fences. The connection
service gates lazy initialization and mutations against retirement so a late
initializer cannot escape shutdown. Engine teardown freezes queues and stops
sessions before draining model-account operations.

Thirteen model-account tests and nine engine model/lifecycle tests pass. The new
fixtures cover abandoned renewal, retirement drainage, concurrent inventories,
entitlement/provider checks, account fallback, reconnect recovery, discovery
cancellation, identity retirement during an active model stream, and lazy-store retirement. Inventory uses a local HTTP fixture and
model execution uses a wire fixture. The selectable application provider, session
protocol adapter, live provider authorization and full chat workflows remain pending.


## Turn boundaries and steering receipts

The kernel separates `stop_turn` from `interrupt`: stopping root generation
keeps independent child runners and commands whose tools have already returned.
Explicit agent/process interruption and runtime close still stop those commands.
A command transfers cancellation ownership only after native `exec_command`
has assembled its successful result. Abandoned tools keep their cancellation
ownership; they cannot leave an unreturned background command running.

A new explicit root turn can run alongside retained children. `reconfigure`
adopts model, reasoning, budget and context settings at subsequent model
boundaries, persists model identity before adoption, and preserves the runtime.
Collaboration-tool topology still requires a fresh runtime. Context percentages
retain fractional provider values rather than truncating them to integers.

`MailConsumed` acknowledges a delivery only after its content joins durable
history. A checkpoint keeps consumed delivery IDs independently of compaction;
a repeated enqueue re-emits its acknowledgement without adding its content
again. A failed save emits no acknowledgement, and recovery retains the pending
mail. Initial prompts and internal child messages do not become steering ACKs.
The application adapter must correlate these IDs with session steering commands.

The full harness suite passes 35 tests. New coverage exercises command handoff,
turn cancellation, explicit cleanup, retained-child continuation, in-place model
changes, fractional context limits, duplicate/recovered acknowledgements, and
failed-save recovery. These are kernel/tool fixtures; selectable application
provider/session integration and live chat acceptance remain pending.


## Application session adapter

`engine::native_harness` registers Zuse in the engine provider catalog on macOS
and Linux. It uses the shared account inventory/broker/SDK transport and the
owned Rust kernel rather than launching another coding-agent CLI. The existing
provider picker, chat stream and session engine consume its standard protocol.
Earlier milestone notes above describe the component state at those milestones.

Each runtime owns a `zuse:<UUID>` journal, a checkout manifest and the native
root lease. Resume rejects foreign cursors, missing journals and checkout
mismatches. The persistent event stream translates assistant text/boundaries,
normalized tools/results/plans, context, usage, compaction and nested child
transcripts. Child user messages are emitted after their history is persisted.
Event attribution reads only model/parent identifiers; it never clones the full
transcript or inline images for each text chunk. Bounded event channels cancel a
failed/overflowed stream and retain its journal for inspection.

Permission prompts and questions use the session engine's existing input bridge.
Read-only contexts also constrain application tools. Hosted plugin discovery and
execution reuse the shared MCP tool catalog, namespaced as `zuse_*`, for root and
child agents alike. The engine overwrites incoming MCP fields with the host's
loopback endpoint and originating chat/device; model arguments cannot choose the
plugin origin. `zuse_read_upload` reads bounded text/base64 chunks through the
profile upload cache's path jail. Image attachments arrive as bounded inline
content, with magic-byte MIME validation. Repository instruction reads reject
symlinks and non-regular files.

The adapter correlates durable mail-consumption IDs with session steering ACKs,
retains background work on turn stop, reports that work before publishing Done,
and adopts model/effort/context/permission changes without replacing the runtime.
Process notices hold a weak kernel reference, avoiding a host/engine retain cycle.
Dropping the stream cancels and drains the owned runtime before its execution
lease is released.

The application fixtures use scripted model output, real durable journals and
filesystem edits. Live model authorization, real Executor tenant/plugin calls,
full UI queue/stop flows, packaged Node/ripgrep, complete instruction/skill parity,
provider-switch history migration, OS command sandboxing and authenticated
unknown-outcome recovery UI workflows still require live acceptance.
This is not full application acceptance.

Verification for this adapter milestone: all 35 harness tests pass, the five
application session fixtures pass, and all 13 session-engine tests pass. The
native-filter engine regression run passes 12 tests (including the five adapter
fixtures and host-origin stamping). Server/provider test targets compile,
formatting passes, and the GPUI Web/WASM bundle builds. Updated desktop CI and
live native/browser conversations remain separate acceptance gates.

The Stop/steering cancellation milestone adds durable withdrawal and delayed-frame
coverage: 36 harness tests, eight native application fixtures, 14 session-engine
tests, one shared receiver test and 15 provider stop fixtures pass locally.
The application cases exercise both in-place Stop and complete runtime termination
through the document/session engine, then continue the same saved conversation.
Server compilation and formatting also pass. CI run `39031148407` established
macOS/Linux native compilation for the preceding UI mapping fix; it does not cover
these subsequent cancellation changes or establish live application acceptance.

## Reviewing an uncertain tool outcome

The native session card contains **Review interrupted tools**. The browser workspace
panel contains a **Recovery** tab. Stop the conversation before opening recovery;
a running native harness retains its root lease and refuses offline review. The
view displays the complete tool input and offers **Copy full input**. Inspect the
checkout, external service or process output before deciding what happened.

**Authorize retry after inspection** removes only the uncertain receipt.
**Save inspected outcome** records your text as that tool's completed result.
Both actions append a durable review audit and leave execution stopped. Continue
from chat only when ready. Recording an outcome skips that tool on continuation;
retry authorization permits the pending call to execute again. Existing completed
receipts are preserved.

`InspectRecovery` and `ResolveRecovery` use the existing authenticated RPC
transport, addressed to the conversation's host device. They are absent from the
model tool catalog. The engine derives the journal from the host-owned chat row,
checks its native session identity, and holds the root lease throughout each
inspection or atomic decision. Resolve checks the session and a digest of the
exact pending call/receipt; stale clicks cannot authorize a different outcome.
The response includes pending calls and review counts, without exporting the
conversation history. Inspection is bounded to 1 MiB of complete call/receipt
data; larger reviews require inspecting the local journal. Recorded result text
is limited to 64 KiB. Native UI requests reset on conversation or engine changes;
browser requests reset on conversation changes and connection events.

Two RPC integration fixtures interrupt a real kernel tool after its execution
starts, reopen the private journal, resolve it through the application RPC, then
explicitly continue. They verify recording versus retry, stale decisions,
foreign-device rejection, exclusive live ownership and no execution on review.
The 36-test harness suite, integrated server check and browser build pass.
Native compile and actual recovery UI interaction remain separate acceptance gates.

## Provider continuity

Automatic resume checks the provider that owns the saved cursor, alongside its
checkout. Live sessions record that ownership; restart recovery derives it from
typed journal events. A native Zuse cursor also identifies its owned format.
Legacy opaque cursors with unknown ownership take the fresh-session path with
transcript context. Explicit invalid native resume requests still fail and
preserve their journal for inspection.

A fresh ordinary conversation turn receives prior visible messages, including
message status, historical tool state/output references, errors and image upload
references. The current user message is delivered once. Later accepted messages
stay behind their own queue boundaries. Text is normalized before JSON framing,
and Cursor avoids prepending a second copy of the same carried conversation.
Transcript read errors propagate instead of silently opening an empty context.
Same-provider runs continue through their own saved cursor.

A scripted-provider integration fixture switches Claude, Codex, Zuse and Cursor,
checks same-provider resume and recovers ownership after clearing the live cache.
A real native-harness fixture migrates from an opaque foreign cursor and checks
prior content, tool/error/image references, and exclusion of a future user row.
All 15 session-engine tests and nine native application fixtures pass, alongside
the server compile and formatting checks. Live cross-provider conversations,
prior-image vision, bounded import of very large histories, deferred context
transfer for command-only provider switches, and crash/consumed-delivery replay
still require implementation or acceptance.
