# Rust harness

`crates/harness` ports the Zuse agent kernel into Rust. It is currently a compiled and fixture-tested runtime component. It is not registered in the application's provider picker yet. Native filesystem/process tools, model-connection OAuth and vault integration, provider event adaptation, the ownership lease, and recovery controls still need application integration before this is an accepted Zuse session provider.

## Runtime authority

`Engine` owns a durable graph of one root and nested children, bounded to three running children and depth two. Rust schedules concurrent child requests, records mailboxes and idempotent delivery identifiers, enforces inherited read-only and plan modes, and delivers child outcomes before root synthesis. The model request budget is shared across the graph and includes compaction and broker retries. User continuation explicitly grants another turn's budget.

`Host` supplies model transport, native tools, checkpoint storage, instructions and application events. It must acquire the session ownership lease before opening a runtime and retain it through shutdown. Tool definitions and permission decisions belong to the trusted host; model input cannot grant permission or call recovery APIs. `FileStore` writes private checkpoints using temporary files, fsync and atomic replacement. Invalid graphs and damaged journals are preserved for inspection.

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
