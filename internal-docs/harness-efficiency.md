# Native harness: implementation and evaluation

2026-10-26. Worktree: `/home/vercel-sandbox/worktrees/lightweight-harness`.
Baseline: `7689394`. All implementation and measurements run on the cloud Linux VM.

## Runtime and storage

Rust owns scheduling, permissions, account affinity, renewal, cancellation and
persistence. `ModelTransport` has native Responses, Messages, GenerateContent and
Chat Completions adapters using a shared HTTP client. The production bridge and
its bundled JavaScript dependencies have been removed. The harness needs no Node
runtime. Commands still use the owned Rust command supervisor; file operations
use the shared blocking pool.

Messages, metadata, tool schemas and receipt outputs share immutable allocations.
Token estimation traverses borrowed JSON and counts serialized bytes instead of
cloning trees and allocating serialized buffers. Broker retries share history;
portable conversion happens when origin compatibility requires it.

Native roots use a sequenced, checksummed transition journal. Ordinary commits
append changed agent heads, new history entries, receipts, delivery decisions and
configuration. Identical state saves do no disk work. Tool start, output and
acknowledgement barriers remain durable. Streamed text fragments are observational
and are not individually persisted. Storage work is serialized per root and
admitted to the runtime's blocking pool through four shared permits across roots. Workers retain the root lease if their
caller is dropped.

Snapshots rotate after 4 MiB of journal growth or 1,000 commits. Publication uses
file fsync, atomic replacement and directory fsync before log truncation. Only
incomplete uncommitted tails can be repaired; corruption is preserved and rejected.
Large immutable values use SHA-256 artifacts, shared within the root, with garbage
collection only after a complete replacement snapshot is durable. Full history
and recovery receipts remain represented. This is not lazy history paging: the
complete recovered history is still resident. Artifact recovery reads and hashes
one bounded buffer, then parses a slice so large strings retain their actual
length rather than geometrically oversized reader buffers.

Version-one checkpoints migrate under the root lease. The original is retained
until successful recovery from the new snapshot. A version-two marker prevents
older binaries from silently resuming a stale checkpoint. Unsupported formats
and foreign root identities fail without resetting history. Downgrading requires
an explicit backup/recovery procedure; replacing the marker manually is unsafe.

## Provider configuration

Existing subscription accounts keep the application's authentication and renewal
service. No API fallback is selected implicitly. Custom subscription authentication
must be supported by the provider; API support does not imply subscription access.

Explicit API routes are loaded at startup from `<profile>/harness/providers.json`:

```json
{
  "connections": [{
    "id": "primary-api",
    "protocol": "responses",
    "endpoint": "https://api.openai.com/v1/responses",
    "credentialEnv": "OPENAI_API_KEY",
    "capabilities": {
      "images": true, "tools": true, "reasoning": true, "promptCache": true
    },
    "maxOutputTokens": 16384,
    "includeEncryptedReasoning": true,
    "models": [{
      "id": "YOUR_ENTITLED_MODEL_ID",
      "label": "Configured model",
      "contextWindowTokens": 32000
    }]
  }]
}
```

Set model IDs, context/output limits and capabilities for the actual entitlement;
the example's context limit is illustrative. Configuration contains environment
variable names, not secrets; `apiKeyEnv` remains an alias for `credentialEnv`.
Explicit Bearer routes and internal environment handoff are covered in the
[live transport check](harness-live-evaluation.md). Entries with missing credentials are unavailable.
Models appear as `api/<connection-id>/<model-id>`. API-key identity participates
in continuation/cache scoping, so rotating a key cannot reuse account-bound metadata.

Protocols/endpoints:

| Protocol | Endpoint shape | Authentication |
| --- | --- | --- |
| `responses` | Provider's `/v1/responses` | Bearer |
| `messages` | `https://api.anthropic.com/v1/messages` | API-key header |
| `generate-content` | `https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse` | API-key header |
| `chat-completions` | Provider's `/v1/chat/completions` | Bearer |

HTTPS is mandatory in application configuration. Loopback HTTP is allowed only
by the transport for local protocol fixtures. Redirects are rejected. Capabilities
must describe the configured models; provider-specific model/effort compatibility
still requires live verification. Chat Completions rejects image tool results;
image user input is supported. The other three protocols support image tool results.
This transport intentionally covers the harness's needs rather than every SDK feature.

Requests, response streams, event frames, tool arguments, output items and
concurrency have explicit bounds. Authentication, connection and parse failures
are sanitized before reporting. Reasoning/signatures are preserved only for the
same model, protocol/route, account and authorization generation.

## Cache and context policy

Stable instructions and sorted tools precede evolving history. Cache identity is
scoped to account, authorization generation, model and root. Local fingerprints
track stable configuration; they are diagnostic, never proof of a provider cache
hit. Actual hits come from provider usage. Messages uses provider cache breakpoints;
Responses uses a scoped cache identity where supported. Automatic caching for
other providers remains provider-controlled. No warming requests are generated.

The default is `ContextPolicy::Preserve`. `BoundedTools` is explicitly opt-in:
older large text tool results become bounded previews with durable output cursors.
`read_tool_output` retrieves kernel receipts; `read_artifact` pages native file
artifacts. Instructions, task, recent complete exchanges and durable history are
preserved. Incremental process cursors and existing summary boundaries remain
available. Changed files/tool results are never reused as if fresh, and intentional
repeated user messages are not deduplicated.

The policy participates in estimation before scheduling a paid summary. Live
quality checks have not passed, so no reduction is enabled by default. No model
or reasoning downgrade is applied. Summaries, retries and child requests count
against the shared request budget.

## Accounting and capped evaluation

Each physical dispatch reports identity, account, model, billing route, purpose,
latency, retry index, stable-prefix fingerprint and any observed usage. Cache read/write,
uncached input and reasoning counts remain optional. Provider totals are normalized
without adding already-included counters twice. Failed/abandoned requests retain
partial usage where available. Unknown usage stays unknown. Subscription usage
is never presented as actual dollar charges or inferred quota savings. No price
catalog is bundled; API dollar estimates are currently omitted.

Build and run deterministic checks:

```sh
cargo test -p zuse-harness --all-targets
cargo test -p zuse-engine --lib native_
cargo test -p zuse-engine --lib harness_models
cargo test -p zuse-engine --lib harness_api
cargo build -p zuse-harness --examples --release
# Existing code triggers these three style lints; keep all other warnings fatal.
cargo clippy -p zuse-harness --all-targets -- -D warnings \
  -A clippy::collapsible_if -A clippy::redundant_closure_call \
  -A clippy::too_many_arguments
python3 scripts/test-harness-evaluation.py \
  --binary target/release/examples/harness-evaluate --output .context/offline-evaluation
python3 scripts/benchmark-harness.py \
  --binary target/release/examples/kernel-bench \
  --baseline /path/to/pinned-original-kernel-bench \
  --output .context/kernel-comparison.json
```

Use the identical `kernel-bench.rs` source and dependency versions in both release
builds. The Python sampler measures Linux process RSS/PSS, CPU, elapsed turn/replay
time, sampled bytes written and retained disk. It runs 1/2/5/10/20 roots with
0/1/10 MiB histories, image payloads and compaction, three repetitions per arm.
Kernel workloads use scripted streaming and tools, then close and reopen storage.
Stop/resume, process trees and active command cancellation are functional tests;
the kernel performance matrix does not measure those workloads or desktop/UI cost.

The local HTTP evaluator exercises the real native adapter, broker, tools and
journal. Its scripted acceptance passed 12 coding runs, three smoke runs and one
six-request capped run: 60 local requests on 16 TCP connections, zero external
model requests. This proves integration/metering behavior, not model quality.

Live evaluation uses `scripts/evaluate-harness.py --manifest ... --output ...`.
The manifest has up to four arms:

```json
{"arms":[{
  "label":"native",
  "protocol":"native-evaluation-v1",
  "command":["/absolute/path/to/harness-evaluate"],
  "connection":"/private/path/connection.json",
  "boundedContext":false
}]}
```

The private connection JSON, mode `0600`, contains `connectionId`, exactly one of
`accessToken` or `credentialEnv`,
`model`, optional `reasoning`, `contextWindowTokens` and the `route` structure
(`endpoint`, `protocol`, `subscription`, `capabilities`, `maxOutputTokens`, optional
`toolNamespace` and `includeEncryptedReasoning`). Never commit it or paste tokens
into command arguments. Keep model, reasoning, task and access route matched.

Every eligible runner must implement `--describe` and enforce metering at its
physical transport boundary. Unmetered external CLIs are excluded. The shared
persistent `.context/harness-evaluation-global.ledger` enforces **300 reservations
across suites**, including retries, summaries, children and smoke requests. Do not
reset or replace it between comparative runs. Each coding run allows six physical
requests; each smoke run allows one. Four tasks × three repetitions × four arms
allow at most 288 requests, plus 12 smoke requests. Conservative reservations
can consume capacity without a dispatch; they never create extra allowance.
The runner reports acceptance failures, capped runs and unavailable arms, and
exits unsuccessfully when acceptance or provider availability is incomplete.

Offline checks pass: 58 harness tests (plus one ignored filesystem microbenchmark),
24 native-path application tests, seven model-service tests and two API-configuration
tests. Clippy passes with the three existing style-lint exceptions shown above.
The application test executable was linked without debug symbols to fit the shared
VM; the release benchmark uses the workspace release profile.

Live status was updated on 2026-10-27: Conductor's environment credential reached
authenticated model inventory. Native smoke requests for the two selected models
at medium reasoning both returned HTTP 429, so coding/cache comparisons have not
run. See the [live check and request accounting](harness-live-evaluation.md).
No claims about live success rate, cache-hit improvement, cost per successful task,
quota savings or superiority over other harnesses are supported yet. Four small
coding fixtures alone would not establish broad quality equivalence.

## Release measurements

126 deterministic runs passed, three repetitions per workload and arm, using
release binaries and pinned original source/dependencies. The fixture performs
two turns per root, including streamed text, tools and durable restart validation.
These figures exclude provider/network/UI time. MiB uses 1,048,576 bytes.

| Roots / history per root | Turn median, original → native | Peak RSS median, original → native | RSS after close, original → native | Sampled writes, original → native | Replay median, original → native |
| --- | --- | --- | --- | --- | --- |
| 1 / 0 MiB | 23.83 → 6.45 ms | 3.54 → 4.12 MiB | 3.54 → 4.12 MiB | 0.12 → 0.12 MiB | 0.13 → 0.36 ms |
| 1 / 1 MiB | 64.55 → 10.31 ms | 9.45 → 7.96 MiB | 6.51 → 6.23 MiB | 28.12 → 1.12 MiB | 0.81 → 2.42 ms |
| 1 / 10 MiB | 465.83 → 66.54 ms | 63.41 → 43.89 MiB | 33.50 → 24.26 MiB | 280.12 → 10.12 MiB | 8.89 → 21.53 ms |
| 20 / 10 MiB | 7142.39 → 1063.18 ms | 263.34 → 253.98 MiB | 43.60 → 4.96 MiB | 5602.43 → 202.43 MiB | 99.61 → 429.13 ms |

For one 10 MiB root, turn latency improves about 7×, peak RSS about 31%, and
sampled physical writes about 96%. The 20-root case improves turn latency about
6.7× but peak RAM only about 4%. Do not generalize the single-root memory reduction
to many roots. After closing/replaying 20 large roots, retained RSS falls from
about **44 MiB to 5 MiB**. Close takes 15.33 ms versus 13.57 ms for all 20 roots;
turns plus close plus replay take 1,507.93 ms versus 7,250.75 ms (median per-run
totals). These close timings include allocator cleanup.

Recovery remains slower because it verifies/restores content-addressed payloads
and replays transitions. Small empty histories use more RAM. The bounded artifact
read eliminates oversized recovered string capacities. On Linux/glibc, final
native-host destruction calls `malloc_trim(0)` after histories and runners drop.
This releases allocator-owned free pages without changing live allocations or
allocator configuration; see the [allocator interface](https://www.kernel.org/pub/linux/docs/man-pages/book/man-pages-6.12.pdf).
No trim runs per request or checkpoint. Other platforms keep their normal
allocator policy; their post-close behavior has not been measured here.

Journal retention increases final disk for short runs: one empty root retains
12,746 bytes versus 2,204 bytes; one 10 MiB root retains 10,499,515 versus
10,487,980 bytes. Lower write volume does not imply smaller retained storage in
every workload. Sampling can miss final CPU/IO activity; high-water RSS includes
fixture seeding and recovery. Post-close RSS is the median of the final five
samples during a 100 ms wait after roots and fixture hosts drop and replay checks
finish. It includes allocator and blocking-pool retention.
The original kernel fixture uses scripted models too, so these measurements do
not quantify removal of the former model bridge's process memory. Shared-VM timing
noise is material; the table reports the final matched release binary pass.

Raw results and offline evaluation artifacts are retained under the primary
workspace's ignored `.context/` (`harness-kernel-comparison-release.json`,
`harness-offline-evaluation-release/` and `harness-live-availability-final/`). They are cloud-local evidence, not a public CI
performance guarantee. Context reductions remain gated and live comparison remains
pending successful provider requests.
