# Native performance baseline

Measured 2026-10-20. These are local engineering measurements, not comparative
product benchmarks. No agents, paid model calls, populated chat histories or
embedded browser previews were active.

## macOS release build

Built the default release profile on an Apple M4 Pro with 48 GiB RAM, running
macOS 26.6.2. Incremental compilation was disabled; the workspace release profile
uses thin LTO and strips symbols.

The arm64 native executable is **149.7 MiB** (157.0 MB decimal).
Gzip level 6 compresses that executable to **51.1 MiB**. This is not a complete
installer: the current distribution packager also includes helper executables,
browser assets and legal notices. Build cache size is unrelated to installed
executable size.

An additional XZ level 6 measurement compresses the same executable to
**31.49 MiB**, taking 33.3 seconds on the same Mac. It still expands to
149.7 MiB. The file is `.context/native-release-size.xz` on the Mac; this is a
compressed executable measurement, not an installer or a complete package.

Mach-O section inspection (`size -m target/release/zuse`) finds approximately
89.1 MiB of machine instructions (`__text`), 36.6 MiB of constant data
(`__TEXT,__const`), and 16.3 MiB across exception/unwind sections. These sections
are part of the 149.7 MiB total, not additional files. The very large
`__PAGEZERO` virtual reservation is zero-fill and occupies no corresponding
disk storage. UI fonts total only 2.15 MiB, file icons 1.35 MiB and sounds
0.77 MiB on disk; asset directory sizes do not establish their exact linked
contribution.

Size-reduction candidates, not yet benchmarked:

- Compare `opt-level = "s"` and `codegen-units = 1` against the current
  speed-oriented release settings, preserving the original benchmark binary.
- Audit enabled dependency features, native shader stacks, image formats and
  bundled syntax grammars. Obtain a linker map before assigning per-library
  savings; section totals alone do not identify the contributing libraries.
- Separate optional large components only where product installation and
  offline behavior remain acceptable. Loading later only reduces initial
  installation size if the component is also distributed separately.
- Do not blindly switch to `panic = "abort"`: existing provider, visual,
  diagram and worker isolation uses unwind-catching. Such a change affects
  failure recovery and needs an explicit design decision.

The compressed single-executable result establishes that a download under
50 MB is possible for that file. It does not establish a complete installer
under 50 MB, or an installed application under 50 MB.

An initial ten-second-warmup measurement caught a headless process-tree RSS
spike of 521.1 MiB and a desktop spike of 383.5 MiB while startup children were
still active. The engine later settled near 41 MiB. These transient children
are included in the sampled process-tree totals; no agent turns were started.

The settled runs used 45 seconds of warmup followed by thirty one-second
process-tree samples:

| Measurement | Headless engine | Desktop with embedded engine |
| --- | ---: | ---: |
| Median resident memory (RSS) | 40.8 MiB | 144.8 MiB |
| Peak sampled RSS | 40.8 MiB | 145.4 MiB |
| Mean sampled idle CPU (one core = 100%) | 0.064% | 0.159% |
| Maximum app process count | 1 | 1 |
| IPC socket readiness | 8.04 s | 0.344 s |

These are single runs on the same Mac, not statistically established startup
figures. Earlier runs observed 10.07 s headless IPC readiness and 0.604 s desktop
readiness. The cause of this headless startup delay has not been investigated.
Neither socket readiness result establishes first-frame latency.

The CPU sampler accumulates nonnegative per-process deltas so that retiring
startup children cannot subtract CPU already consumed. Processes that start
and exit between samples may be missed. RSS is not macOS Activity Monitor's
physical-footprint metric and does not capture all GPU allocations.

## Linux development build

The existing x86-64 executable was built on October 18 with the local
unoptimized development configuration. SHA-256:
`ca9714059ae9bd8d6bdf2cc35eeb55dc6e5285822c4d4da71824fc881571726f`.
It predates the latest source changes. Desktop measurement used Xvfb and software
Vulkan in the cloud sandbox, not a physical GPU.

Each run used a disposable fresh profile, ten seconds of warmup and thirty
one-second process-tree samples. CPU percentages use one logical core as 100%.

| Measurement | Headless engine | Desktop with embedded engine |
| --- | ---: | ---: |
| Median resident memory (RSS) | 69.2 MiB | 250.7 MiB |
| Peak sampled RSS | 69.4 MiB | 250.8 MiB |
| Median proportional memory (PSS) | 39.0 MiB | 229.1 MiB |
| Mean idle CPU | 0.10% | 0.30% |
| IPC socket readiness | 51 ms | 51 ms |
| Maximum app process count | 1 | 1 |

IPC readiness measures when the local control socket accepts a connection. It
does not measure first frame, provider readiness or perceived startup time.
RSS counts resident shared pages fully; PSS apportions them between processes.
The sampled peak can miss brief allocation spikes between samples.

The executable occupies 340.6 MiB. Stripping a separate copy produces 234.9 MiB;
gzip level 6 compresses that copy to 68.1 MiB. These are executable sizes, not a
release installer or a complete package. Optional browser assets in this
checkout occupy another 43.4 MiB; system libraries and external agents are not
included in executable sizes.

## Reproducing

```sh
python3 scripts/benchmark-native.py --binary target/release/zuse \
  --mode headless --warmup 45 --output .context/benchmark-headless.json
python3 scripts/benchmark-native.py --binary target/release/zuse \
  --mode desktop --warmup 45 --output .context/benchmark-desktop.json
```

The desktop mode needs a graphical session. The script launches its own app
with an isolated temporary profile and stops it afterward. It does not modify
the normal user profile. JSON includes all samples, binary path/mtime and OS.
Run on an otherwise idle machine; avoid concurrent compilation when comparing
results. The headless and desktop modes each start their own embedded engine;
the development watcher's separate engine/UI arrangement is a different case.

## Unmeasured

Battery power, physical GPU use, first frame, sustained streaming, loaded
histories, many active agents and their descendant processes still need
measurements. Idle CPU alone does not establish battery efficiency. Agent count
claims must include the selected providers' processes and tools, not only the
desktop shell.

## Large-chat switching diagnostic (2026-10-27)

A native Linux debug build on a private Xvfb desktop with software Vulkan
reproduced slow switching between cached 10,000-message chats. Keyboard input
was timed until the correct chat's final-message pixels appeared. Alpha/Beta
negative-control images differ; accepted frames match the expected marker.
The fixture uses full offline legacy snapshots; current thin online history
is not measured. This is a same-machine diagnostic, not a macOS release latency
measurement.

Four debugger samples found the main UI thread searching the complete row list
once per prompt inside the transcript outline. At 10,000 alternating messages,
that requires roughly 25 million comparisons per redraw. The outline now builds
one borrowed row-id index per render, preserves first-match targets and prompt
order, and reads the transcript without cloning its complete message payload.
No history or navigation marks are removed.

| Scenario | Before p95 | After p95 |
| --- | ---: | ---: |
| Two chats, 1,000 messages each (control) | 113 ms | — |
| Two chats, 10,000 messages each | 1,739 ms | 346 ms |
| 125 stored chats, 10,000 messages each; 20 switches | 2,265 ms | 456 ms |

The original 125-chat repro passes its 500 ms diagnostic budget after the fix.
The captured outline pixels also match. All 15 outline tests pass, including
missing rows, first-match duplicate ids, ordering, previews and navigation math.
Raw timings, executable hashes, the input/pixel loop and stack samples are
preserved in `.context/efficiency/ui-lag/`.

A separate three-sample composer check timed real keystrokes until draft text
appeared, with no prompt submitted. Observed p95 (the maximum of three samples)
fell from 1,595 ms to 286 ms; median fell from 1,431 ms to 262 ms. This small
diagnostic sample is not a general input-latency distribution.

## Concurrent full-history memory diagnostic (2026-10-27)

A Linux debug headless native engine on eight logical CPUs and 16 GiB RAM
received simultaneous `WatchDocMessages` requests over its real WebSocket.
Every 10,000-message response was checked for complete count, text and order.
The fixture uses full offline legacy snapshots, not current thin online
history. These are single-process diagnostic trials, not Mac release figures.
External test-client memory is excluded; engine RSS was sampled every 50 ms.

| Concurrent chats | Original peak RSS | First-pass peak RSS | Original / first-pass batch time |
| --- | ---: | ---: | ---: |
| 50 | 3,744 MiB | 875 MiB | 9.37 / 9.23 s |
| 100 | 5,545 MiB | 945 MiB | 17.70 / 13.47 s |
| 125 | 6,730 MiB | 1,059 MiB | 22.41 / 17.76 s |

Whole-list JSON expansion was a modest contributor. The larger issue was
unbounded initial history materialization combined with a 256-frame socket
output queue. At that checkpoint, each connection admitted four history openings
before opening their documents and buffered eight outgoing socket frames. Admission
lasts through an opening preview and full hydration; established watches
release it. Canceled requests release their slots and other RPC methods
remain independent. This bounds bursts without truncating history or changing
persistence. Decoding also captures stable row handles and materializes one
row at a time; strict decoding borrows its JSON input instead of cloning it.

After 12 seconds idle, the 125-chat RSS fell from 4,593 to 1,023 MiB; the
initial 512 MiB-at-12s diagnostic target remains unmet. Allocator-reclaim
probes are separate evidence and excluded from headline numbers. The Mac
release path still requires verification; allocator behavior differs by OS.
All 123 document and 28 RPC unit tests pass, together with six targeted
engine tests for replay, reconnect, context updates and full opening history. The new admission regression
failed before the fix, then passed with cancellation, full hydration after
previews, unrelated RPCs and idle watch admission release covered.
Raw measurements and diagnostic scripts live in `.context/efficiency/ram/`.

A second fixed 125-chat run verified all 1.25 million messages again. RSS was
986 MiB at 12 seconds idle and 361 MiB at 70 seconds, through the existing
Linux allocator cleanup. A later diagnostic reclaim did not lower it further.
That repeated run peaked at 1,011 MiB and completed in 19.92 seconds. It is
kept separately rather than substituting its lower peak for the first trial.

## Direct transcript serialization follow-up (2026-10-27)

History updates now stay typed until the RPC transport serializes them into
the existing `{id,item}` envelope. This removes a complete intermediate JSON
object tree and drops the typed payload before waiting for the output queue.
Value-based streams and typed streams share one sending loop. A single typed
update producer supplies full history and deltas; opening previews retain
initial admission through full hydration. Encoding errors return a terminal
RPC error and release admission. No wire format or history storage changes.

Fresh controls reran the first-pass executable before comparing the completed
source. Every response's complete count, order and text was checked. The
following are Linux debug headless engine trials with full offline legacy
snapshots, not Mac release app measurements. Each row has one process trial
per build. Timings varied across sessions, so timing comparisons below use
these fresh controls instead of the earlier first-pass timing table.

| Chats × 10,000 messages | First-pass peak RSS | Typed peak RSS | Control / typed batch time |
| --- | ---: | ---: | ---: |
| 50 | 933 MiB | 750 MiB | 17.61 / 13.04 s |
| 100 | 927 MiB | 713 MiB | 31.78 / 24.10 s |
| 125 | 1,019 MiB | 763 MiB | 40.66 / 31.55 s |

The completed executable's SHA-256 is
`ca7959db4938e0c5f883c9b0935daca5959dda54c1c88b1e25df6459ab70c73e`.
The 125-chat run used 743 MiB at 12 seconds idle and 302 MiB at 70 seconds
before any debugger reclaim. The 512 MiB-at-12s target remains unmet.

That measured snapshot used four history openings and eight socket output frames. A
two-opening probe lowered RAM but slowed the fresh 50-chat batch from 17.61
to 29.28 seconds; a two-frame queue showed no convincing RAM benefit. Neither
change was retained. Warm transcript mirrors retain their existing behavior.

All 123 document tests, 32 RPC tests and seven targeted engine/integration
checks pass. New tests cover deferred single encoding, compatibility with
the existing envelope, typed cancellation and preview admission, and recovery
from encoding errors. Strict Clippy remains blocked by existing lints in
unchanged protocol and RPC client/managed/device-room files; modified RPC
files produced no lint diagnostics.

The benchmark scripts, fresh controls, exact-source measurements, probes and
validation logs are preserved in `.context/efficiency/ram/`. Mac release and
current thin online history still require measurements.


## Speed-prioritized history opening follow-up

The current opening policy allows five simultaneous history openings and eight
queued socket frames. Reset frames share the host's immutable transcript through
`Arc<Vec<SessionMessageEntry>>`; an owned viewport reset reuses its allocation,
while a shared reset copies on application. JSON envelopes are unchanged.
The transport reserves output capacity before encoding a frame, after polling
its stream, so idle watches do not hold output slots. There is no new process
RAM ceiling or history truncation.

A matched Linux debug headless-engine comparison changed only opening concurrency
from four to five, using the same established JSON decoder and shared snapshots.
Each chat contained 10,000 messages; every text and position was verified before
canceling its subscription. These are concurrent history receipts, not sustained
active provider sessions or rendered chat-switch measurements.

| Concurrent chats | Four / five opening receipt time | Four / five peak app RSS |
| --- | ---: | ---: |
| 1 | 0.91 / 0.95 s | 126 / 127 MiB |
| 50 | 11.66 / 9.36 s | 602 / 704 MiB |
| 100 | 22.09 / 18.68 s | 685 / 773 MiB |
| 125 | 28.41 / 22.24 s | 741 / 800 MiB |

Single-chat figures are medians of three alternating runs; stress cases have one
trial per binary, with app-only RSS sampled every 25 ms. Bulk cases improved at
a modest memory cost, but single-chat improvement is not established and a
five-second complete-history target has not been achieved. A separate matched
decoder trial did not justify retaining a custom deserializer, so it was removed.

The final native app build, 123 document tests and six RPC server tests passed.
Snapshot sharing, unchanged wire format and copy-on-application were also covered
by model and engine tests during the preceding validation. The known picker
failures described in `cleanup-audit.md` and the baseline plugin-form mouse-focus
test failure remain unresolved; this is not a claim of a green full UI suite.
Raw results and binary identities are retained under
`.context/latency-rebalance/throughput/`.
