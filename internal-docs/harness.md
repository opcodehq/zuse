# Rust harness

`crates/harness` ports the Zuse agent kernel into Rust. It is currently a compiled and fixture-tested runtime component. It is not registered in the application's provider picker yet. Native filesystem/process tools, model-connection OAuth and vault integration, provider event adaptation, and recovery controls still need application integration before this is an accepted Zuse session provider.

## Runtime authority

`Engine` owns a durable graph of one root and nested children, bounded to three running children and depth two. Rust schedules concurrent child requests, records mailboxes and idempotent delivery identifiers, enforces inherited read-only and plan modes, and delivers child outcomes before root synthesis. The model request budget is shared across the graph and includes compaction and broker retries. User continuation explicitly grants another turn's budget.

`Host` supplies model transport, native tools, checkpoint storage, instructions and application events. Native adapters use `Engine::open_native_with`, which acquires a per-root OS lease before reading its private journal and retains it until host cleanup succeeds. Root IDs map to hashed profile directories, and journal identities must match their owner. `Engine::open` remains available for fixtures and custom hosts that supply their own ownership authority. Tool definitions and permission decisions belong to the trusted host; model input cannot grant permission or call recovery APIs. `FileStore` writes private checkpoints using temporary files, fsync and atomic replacement. Invalid graphs and damaged journals are preserved for inspection.

A tool's signed input identity and `started` receipt are persisted before execution. Its completed output is persisted before the tool-result message enters model history. Recovery reuses committed outputs; a started receipt with no outcome pauses execution. A trusted user control surface can call `resolve_unknown` with an inspected outcome or explicit retry approval. Approval is saved before continuation and rolled back in memory if storage fails. Approval itself never launches work. This API still needs the application's authenticated recovery controls.

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
