# Harness performance

The repeatable workload exercises the real Rust runtime with local model replay,
real file reads and atomic, fsynced checkpoints. It uses no model credentials or
network requests. This isolates application overhead from model and network
latency. It does not measure desktop rendering, provider CLI processes or battery
consumption.

## Run

Build once, outside the measurement interval:

```sh
export CARGO_TARGET_DIR=.context/harness-benchmark-target
cargo build --locked --release -p zuse-harness --example workload
python3 scripts/benchmark-harness.py --threads 20 --events 500 --repeats 5 \
  --output .context/efficiency/normal
python3 scripts/benchmark-harness.py --threads 20 50 100 125 --events 10000 \
  --repeats 3 --output .context/efficiency/stress
```

Each thread is an independent root session, rather than a child of one root.
This preserves the production limit on children. Every four seeded events are
a user message, an assistant tool call, a tool result and an assistant response.
Payloads include text, paths, Unicode and escaped characters. The exact serialized
dataset size is recorded in each result.

All histories stay loaded while the sessions execute concurrently on four runtime
workers. Each round streams 64 chunks, makes two replayed model requests and reads
one real local file per thread. The workload verifies history counts, stream
counts, tool counts and checkpoint reloads before reporting success. Use `--rounds`
to exercise repeated turns without recreating sessions.

The context window is deliberately large enough to retain the seeded history in
model requests. This tests full-history overhead; compaction correctness is covered
by the runtime tests. The native transport has separate streaming, cancellation, accounting and
continuation fixtures. The `workload` example retains the direct local replay
mode. The historical `--sdk` mode and its Node bridge were retired by the native
transport migration; the SDK measurements below describe their original source
revision, not the current executable. `scripts/benchmark-kernel.py` separately
samples the journal/artifact kernel matrix with `kernel-bench`.

Results include total workload time, per-thread p50/p95 completion time, sampled
peak process-tree RSS, logical checkpoint bytes written, tool and chunk counts,
platform and executable identity. The sampler interval is 100 ms, so brief peaks
may be missed. RSS is not physical footprint or GPU memory.

Use separate target directories for different source checkouts to avoid cached
artifacts being reused across worktrees. For before/after runs, preserve each
release executable with `--binary`, use the
same workload source and toolchain, and measure sequentially without builds or
other workloads running concurrently. Keep raw artifacts in `.context/`. Report
run medians and variation, rather than selecting the best sample. These local
measurements are not portable CI timing thresholds.

## Runtime behavior

- Checkpoint serialization uses a bounded 64 KiB buffer. File and directory fsync,
  private permissions, validation and atomic replacement remain required.
  Checkpoints and process metadata use the same atomic JSON writer.
- Empty mailbox checks do not rewrite an unchanged checkpoint. Final responses
  and their summaries commit together; tool receipt durability is unchanged.
- Reconfiguring only nonpersisted runtime limits does not rewrite model history.
- Prompt rendering and context accounting share one retained-context iterator.
  Accounting borrows messages, masks image payloads and preserves the previous
  JSON-size token estimate without constructing a temporary JSON tree or buffer.
  Serde writes into a byte counter so escaping, numbers and role encoding have
  one implementation.
- Journals, model requests, inherited child histories and snapshots share immutable
  messages. Account portability copies only records containing private metadata.
- Native broker retries share the message vector. Compatible continuations keep
  provider metadata; portable conversion copies only payloads with private fields.
- Root-ID lookups do not clone a session checkpoint.

### Historical SDK implementation

The following mechanisms applied to the retired SDK bridge and explain its
recorded before/after measurements. Current production uses native HTTP/SSE.

- SDK admission uses one 128-request semaphore. Excess requests wait without
  expanding wire buffers; cancellation and shutdown release waiting requests.
- The pinned SDK patch selects message schemas by `role` and content schemas by
  `type`. It keeps the existing validators while avoiding construction of errors
  for unrelated variants on every retained message. Both the published source
  and distribution are patched through Bun's versioned patch mechanism; no
  validator is copied into the bridge. Run `bun install --frozen-lockfile`,
  `bun run test` and `bun run build` in `crates/harness/bridge` when updating it.
  Preserve the multimodal, reasoning, tool-approval and malformed-input checks.
- Once a shared-pipe write begins, its complete JSON frame keeps the writer lock
  even if the caller is cancelled. Other requests cannot receive a truncated frame.

### Linux validation-dispatch measurements (historical SDK revision)

On the same Linux sandbox with Node 24.22.1, a CPU profile identified construction
of validation errors for unrelated message variants as a major SDK cost. The
versioned patch keeps the existing schemas and dispatches directly to the matching
role or part type. A private parity check covered 3,340 inputs before integration;
the production bundle exactly matched the tested candidate. The full 44-test Rust
harness suite and SDK transport/schema checks passed.

Each result below is one completed optimized replay run, with 10,000 retained
events per independent thread, two model requests, a real file tool, streaming and
durable recovery. These are harness timings; they exclude seeding and rendering.
They are diagnostic samples rather than a statistical latency guarantee.

| Threads | Before schema dispatch | After schema dispatch |
| --- | ---: | ---: |
| 50 | 103.46 s | 20.47 s |
| 100 | 176.95 s | 34.50 s |
| 125 | 204.15 s | 49.10 s |

The 125-thread sampled process-tree peak was 2,077 MiB, close to the previous
2,098 MiB. This step improves CPU time. Checkpoint contents, counts, tool results
and recovery checks remain intact. The direct replay path has three final runs
per stress size; its 125-thread median was 6.35 s and 1,250 MiB peak RSS.

Repeated SDK builds now preserve identical bundle and license-file bytes and
timestamps. A subsequent optimized harness build finished in 0.51 s without
recompiling after two identical SDK builds. Temporary histories are removed by
the benchmark driver after each run.

Rust history/request interfaces use `Messages` (`Vec<Arc<Message>>`); model outputs
remain owned messages until committed. Checkpoint and bridge JSON formats are
unchanged, so existing private journals require no migration.

## Cloud verification (2026-10-26)

Release Linux x86-64 measurements against commit
`768939434f2e1c4b1d5767bf25dd71547fb325a4`, using identical seeded journals.
Direct replay: five normal runs and three runs per stress tier. SDK replay:
three normal runs; final stress tiers use three samples at 50 threads, two at
125 and one elsewhere, with one reference SDK run per tier. Times are medians where repeated;
RAM is the median sampled process-tree peak RSS. The final release executable
is byte-identical to the executable used for these measurements.

| Replay | Sessions × seeded events | Seconds before → after | Peak RSS MiB before → after |
| --- | ---: | ---: | ---: |
| Direct | 20 × 500 | 1.268 → 0.051 | 26.9 → 13.6 |
| Direct | 20 × 10,000 | 22.866 → 0.662 | 461.9 → 245.6 |
| Direct | 50 × 10,000 | 57.530 → 1.651 | 1070.4 → 532.5 |
| Direct | 100 × 10,000 | 111.640 → 3.389 | 2001.2 → 1010.7 |
| Direct | 125 × 10,000 | 124.862 → 4.686 | 2571.0 → 1249.7 |
| SDK | 20 × 500 | 3.987 → 2.177 | 424.1 → 499.9 |
| SDK | 20 × 10,000 | 57.435 → 24.736 | 1416.8 → 679.5 |
| SDK | 50 × 10,000 | 146.192 → 76.851 | 3128.2 → 1250.8 |
| SDK | 100 × 10,000 | 303.424 → 117.608 | 5808.9 → 1748.4 |
| SDK | 125 × 10,000 | 357.791 → 170.293 | 6901.0 → 2195.8 |

The 20-chat, 500-event SDK workload has a 17.9% higher sampled
RAM peak. Heap-size tuning and an extra SDK preparation queue were tested and
rejected because they did not improve the overall memory/throughput tradeoff.

One-turn checkpoint saves fell from 14 to 12 per session, reducing logical bytes
written by about 14.3%. This does not reduce retained journal data or establish
a smaller installed application size. A separate SDK run completed 20 rounds
across 20 existing sessions with checkpoint recovery and stream/tool counts intact.

The harness suite passes all 44 tests, including 125 simultaneous SDK requests,
cancellation-safe pipe frames, stalled-stream independence, invalid-prompt cleanup,
compaction compatibility, permissions and process recovery. Strict Clippy passes
for all harness targets. The app engine type check, SDK translation tests and
24 targeted engine tests also pass: 15 native-adapter tests, seven account-routing
tests and two reviewed-recovery tests.

Raw run JSON, per-run ranges, executable hashes, rejected experiments and the
local report are preserved in `.context/efficiency/`. Reference builds used an
isolated target directory after an initial artifact cache collision was detected
and excluded. Background compilation in another worktree was observed during
baseline and final measurement periods; timing comparisons are shared-VM
engineering estimates. These measurements support local engineering decisions;
they do not establish desktop speed, real-provider latency or a product ranking.

Targeted app-boundary verification:

```sh
cargo test -p zuse-engine --lib native_harness:: --locked
cargo test -p zuse-engine --lib harness_models::tests --locked
cargo test -p zuse-engine --lib native_recovery:: --locked
```

Debug information was omitted from the cloud engine unit-test executable to fit
the VM's available disk. Production source and release build settings were unchanged.

## Interactive replay verification

Five runs per case with one and five active chats. SDK timings include cold
worker startup. These use the same local HTTP replay and checkpoint assertions.

| Active chats × events | SDK seconds before → after | SDK peak RSS MiB before → after |
| --- | ---: | ---: |
| 1 × 100 | 0.320 → 0.262 | 130.5 → 135.6 |
| 5 × 100 | 0.458 → 0.412 | 203.1 → 191.3 |
| 1 × 500 | 0.622 → 0.359 | 214.1 → 207.3 |
| 5 × 500 | 1.155 → 0.776 | 357.6 → 372.4 |

Memory improvements are workload dependent. Very short direct-replay RSS
bursts can fall between 100 ms samples, so small peak differences are weak
evidence. Compare the cases individually, including cold SDK worker startup.

## Desktop and energy measurements

Use `scripts/benchmark-native.py` separately for fresh-profile desktop/engine idle
measurements. Its socket readiness metric is not first-frame latency. Loaded
desktop responsiveness, physical GPU use, energy consumption and installed bundle
size need separate measurements on the target OS and hardware. Do not extrapolate
them from the harness workload.

## Mac harness verification (2026-10-27)

Release ARM64 replay on an M4 Pro (14 cores, 48 GiB), macOS 26.6.2,
Rust 1.98.0 and Node 24.18.0. Checkpoints use ordinary host APFS. The
standalone example uses the default allocator; the GPUI app uses mimalloc v2,
so these figures do not establish application memory usage.

Three normal samples per case and one sample per completed stress case.
Workload seconds exclude initial seeding; RSS includes the SDK worker when used.

| Replay | Chats × seeded events | Seconds before → after | Peak RSS MiB before → after |
| --- | ---: | ---: | ---: |
| SDK | 1 × 500 | 0.833 → 0.313 | 222.9 → 181.0 |
| SDK | 5 × 500 | 2.770 → 0.727 | 379.6 → 373.8 |
| Direct | 50 × 10,000 | 368.906 → 5.540 | 870.6 → 478.9 |
| Direct | 100 × 10,000 | 687.470 → 10.528 | 1734.9 → 937.0 |
| Direct | 125 × 10,000 | incomplete → 12.647 | incomplete → 1164.3 |
| SDK | 50 × 10,000 | 433.199 → 45.192 | 2445.8 → 1143.5 |
| SDK | 100 × 10,000 | incomplete → 91.830 | incomplete → 1735.5 |
| SDK | 125 × 10,000 | incomplete → 116.427 | incomplete → 2039.1 |

All optimized stress cases verified history, tool/stream counts and checkpoint
recovery. The repeated SDK run completed 400 turns and 25,600 chunks across
20 existing chats in 41.246 seconds, at 561.7 MiB sampled peak RSS.

The three incomplete references hit a 900-second total-case timeout. Do not
compare that timeout directly with workload-only duration as an exact speedup.
Mac sleep interrupted the direct 125-thread reference. Earlier direct timings
may overlap constrained build/recovery activity; these are local engineering
observations on an actively used machine. The later SDK comparisons ran
sequentially after task builds and app measurements ended.

A fresh full GPUI build stopped at a conservative 4.2 GB additional-disk guard,
within the requested approximate 5 GB limit. It did not produce a verified
current application result. All task-owned Mac build copies, datasets, profiles,
processes and output archives were cleaned after evidence export. Raw samples,
failed cases, source/executable hashes and cleanup receipts are preserved under
`.context/efficiency/mac/`; this evidence does not establish rendered UI
performance, physical/GPU footprint or energy use.

A later full ARM64 GPUI release build succeeded by preserving dependency-cache
source paths while rebuilding all project libraries in an isolated target clone.
The build took 9m 59s, with 1.74 GB of observed new file allocation and a 2.78 GB
maximum filesystem free-space drop, below the same disk guard. Cache lock
contents were preserved. Its GUI samples remain unverified: no on-screen window
was detected and macOS subsequently reported a locked session. The successful
compile does not establish rendered responsiveness or runtime memory usage.

An unlocked follow-up rebuilt the same production patch in 14m 05s and passed
three fresh-profile native-window trials. Median post-idle kernel footprint was
91.0 MiB; median sampled RSS was 137.9 MiB. Window detection times were
2625, 325 and 343 ms, retaining the slower first launch in the evidence.
These are onboarding/idle measurements with no projects or providers, not a
loaded-workspace or first-token benchmark. Observed new file allocation peaked
at 1.74 GB, below the 4.2 GB guard. Build clones, the executable, temporary
profiles and export files were removed after verified cloud export.
