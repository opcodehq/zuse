# Headless harness reference evaluation

**Historical suite.** The [2026-10-28 full rerun](harness-followup-2026-10-28.md)
records current-code results: native Opus completed 12/12 under the unchanged
six-request cap. The earlier 7/12 result below is retained as historical evidence.

Cloud Linux VM only, 2026-10-27. The earlier kernel microbenchmark compares the
old and new implementations of this project's kernel. It is not an external
harness comparison. This evaluation runs a published reference CLI and the
release native evaluator as complete processes, including their tool children.

## Method

The private manifest/evidence lives in `.context/reference-comparison/`. The
reference archive is verified against its published SHA-256 manifest, version
0.99.1, release commit `7206efd0c1dbf49ef38d06b63f3a96a3a7cee611`. The distributed
shell wrapper is retained. The native executable is pinned before running, and
its SHA-256 is recorded in the report so other workspace builds cannot change
an arm halfway through evaluation.
The pinned evaluator includes the empty-argument fix. Final source additionally
validates unknown empty-delta blocks and retains supplied reasoning counters;
usage comparisons come directly from the proxy-captured provider counters.

Four fixed coding fixtures from `scripts/evaluate-harness.py` run three times
per arm and model: boundary correction, a change across two modules, large
untrusted evidence, and conflicting instructions in data with protected files.
Each checkout starts as a separate repository with identical input files and
external acceptance checks. Both arms use the same subscription token, exact
`claude-sonnet-5-5` / `claude-opus-5-5` models, medium effort and a 4,096-token
output ceiling. Default context preservation stays enabled. Reference tool
exposure uses the distributed full profile; the native evaluator exposes its
native tools. This compares the usable headless harnesses rather than identical
prompts or feature sets. Neither desktop application is measured.

A local admission proxy validates account, model and effort before every upstream
model dispatch. It atomically reserves the existing shared 300-request ledger and
limits each run to six physical dispatches, including retries. Local denials do
not consume upstream requests. An inference-only OAuth token is handed to the
reference through its documented environment import, wrapped with an expiry for
the benchmark window; no refresh credential or paid API fallback is introduced.
The provider's authenticated model inventory seeds the reference's model cache
because its static list predates the selected model IDs. No reference source is
patched. Non-inference profile probes are rejected locally; they are outside the
model budget and their quota data is unavailable with this token. Credentials,
authorization headers and request bodies are not logged.

Both arms pass through the same proxy and upstream TLS path. The reference also
uses validated TLS on loopback with a private, process-scoped CA; the native arm
uses loopback HTTP. That extra handshake is a timing confound and prevents using
small latency differences as proof of transport superiority. The proxy and its
response capture are excluded from both measured process trees. Raw response SSE
is retained privately to inspect stream failures and verify served model IDs.

RSS and PSS are sampled over each process tree every 20 ms. RSS counts shared
pages in each process; PSS divides shared pages. Short-lived child peaks and final
IO can escape sampling, so the named discovery driver reports CPU and writes as
sampled estimates. Retained profile sizes are logical file bytes, including each
arm's default profile logs, excluding downloaded executables, checkouts and
external evaluation logs. Binary size comparisons describe these executables'
shipped feature sets, not equivalent desktop distributions.

The reusable evaluator now measures process-tree RSS/PSS, retains child identity
across reparenting, and adds wait-based child CPU and block-output accounting.
The local scripted integration suite verifies command helpers appear in the tree.
These additional reusable metrics do not retroactively replace the discovery
driver's sampled counters.

## Interpretation

Acceptance and normal completion are separate: a capped run may have fixed the
code but still failed to finish within six model requests. Every failed and capped
run stays in the report. Usage per successful task includes the unsuccessful
runs' usage. Cache read/write counters are summed once from provider message
usage; nested iteration counters are not added again. Reasoning counters are
recorded when supplied, without adding them again to inclusive output totals.

This subscription route exposes token/cache measurements rather than actual task
charges. Reference defaults may request a different cache retention period. A
higher cache-hit percentage alone does not establish lower cost: absolute prompt
size, writes, output and request count also matter. No subscription dollar savings
or broad quality equivalence is inferred. Context reductions remain opt-in.

## Live results

The corrected suite completed all 48 scheduled runs. No served model mismatch
was observed. All dispatched requests retained provider usage. The suite used
209 physical model requests; the persistent ledger stands at **274/300** after
including prior diagnostics and conservative reservations. No further live
requests are needed for these results.

| Model, medium | Native median peak RSS | Reference median peak RSS | Native completed and accepted | Reference completed and accepted |
| --- | ---: | ---: | ---: | ---: |
| Sonnet 5.5 | 19.02 MiB | 55.98 MiB | 12/12 | 12/12 |
| Opus 5.5 | 19.68 MiB | 56.11 MiB | 7/12 | 12/12 |

All 24 code outputs from each arm passed acceptance and protected-file checks.
Five native Opus runs exhausted the six-request ceiling before final completion;
they remain failures for the completion and usage-per-success comparison. Native
Sonnet used 52 requests versus 41; native Opus used 64 versus 52. No parser errors
recurred after the empty-delta fix.

Among matched pairs that both completed, Sonnet's median elapsed time was
**13.91 s native / 8.48 s reference** across 12 pairs. Opus was **12.67 s / 15.06 s**
across seven pairs; selecting only these seven excludes five native capped runs
and cannot establish superior overall Opus speed or reliability. These are small
coding fixtures with stochastic model latency, rather than representative product
latency distributions. Ordering alternates by repetition.

Across both models, median retained profile bytes were **40.57 KiB native /
200.71 KiB reference**, about 80% lower for these profile configurations. Median
peak RSS was about 66% lower. Native and reference executables were respectively
6.4 MiB and 163.4 MiB, with substantially different shipped feature sets.

| Model | Native input tokens per successful task | Reference input tokens per successful task | Native output tokens per successful task | Reference output tokens per successful task |
| --- | ---: | ---: | ---: | ---: |
| Sonnet 5.5 | 25,226 | 78,522 | 1,004 | 807 |
| Opus 5.5 | 58,082 | 99,203 | 2,304 | 991 |

Input totals here include cache reads, cache creation and uncached input, each
once. Numerators include failed/capped runs. Cache read fractions were 87.2% /
96.4% for Sonnet and 88.1% / 95.0% for Opus, native/reference. Native cache writes
were **higher**, despite the smaller prompt: 38,415 / 18,759 Sonnet and 48,119 /
44,516 Opus. Output usage and extra round trips also favor the reference. Smaller
input totals therefore do not establish lower subscription charges or universally
better cache handling. Provider quota consumption per run remains unavailable.

The results establish lower memory and retained profile sizes for these headless
workloads. They do not establish a generally faster, cheaper or more reliable
harness. The next bottleneck is avoidable model/tool round trips: captured native
runs show shell reads followed by rejected writes lacking a fresh file hash,
then a proper read/edit, verification and additional cleanup. Fresh-hash editing
protection remains in force. Improve tool guidance and evaluate completion under
the same cap before claiming overall superiority. Do not reduce reasoning or
remove editing/recovery protections to improve the score.

Private evidence: `validated/report.json`, `summary.json`, request streams and
per-run results under `.context/reference-comparison/`. Discovery failures and
interrupted runs remain separately preserved.

## Scripted process stress

A separate local Messages fixture completed **90/90 batches**: 1/2/5/10/20 CLI
roots, 0/1/10 MiB of synthetic assistant evidence, two arms and three repetitions.
Each root executes one real file read and receives a final response. Both arms
use one CLI process per root beneath an identical shell launcher; this does not
compare shared-server multiplexing. External model requests: **zero**.

| Roots / evidence per root | Native peak RSS | Reference peak RSS | Native elapsed | Reference elapsed | Native CPU | Reference CPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 / 0 MiB | 10.68 MiB | 50.89 MiB | 0.132 s | 0.206 s | 0.011 s | 0.103 s |
| 1 / 10 MiB | 70.74 MiB | 138.95 MiB | 0.338 s | 0.641 s | 0.149 s | 0.496 s |
| 20 / 0 MiB | 148.93 MiB | 928.99 MiB | 0.158 s | 0.447 s | 0.161 s | 1.880 s |
| 20 / 10 MiB | 1,022.80 MiB | 1,748.57 MiB | 1.529 s | 2.545 s | 2.671 s | 8.267 s |

Values are medians of three batches, sampled concurrently over the complete
launcher/CLI/tool process tree. At 20 roots/10 MiB, sampled PSS was 900.16 MiB /
1,022.03 MiB, substantially narrowing the apparent advantage compared with RSS
because shared executable pages are counted repeatedly in RSS. Retained profiles
were 200.18 MiB / 202.96 MiB; wait-based block output was 201.95 MiB / 407.11 MiB.
The large-history retained disk difference is therefore small, despite lower
write volume. Small live-profile disk savings do not generalize to all histories.

The synthetic server adds 50 ms to each response, uses a sufficient connection
backlog and accepts the reference's native gzip request encoding. It is outside
the measured tree; its parsing and encoding still affect elapsed time. Reference
loopback TLS remains an additional timing confound. Synthetic evidence is emitted
before the file read, with a 16M-token fixture window to avoid compaction; it is
allocation/serialization/persistence stress, not a real 4,096-output-token model
response, recovery benchmark or quality evaluation. No restart, image or context
reduction comparison is claimed by this fixture. Temporary profiles are measured
before cleanup and removed after each batch.

An earlier fixture version mishandled compressed requests and had an undersized
connection backlog. Its failed results are preserved as fixture-discovery data
and excluded from the corrected table. Corrected data is `offline-report.json`;
all scripts and raw results remain under the private evidence directory.

## Validation

All 67 harness/example checks pass (one pre-existing manual microbenchmark is
ignored). New regression coverage accepts empty-only argument deltas, retains
initial arguments, assembles nonempty fragments after empty deltas, and rejects
malformed JSON and unknown blocks. Reasoning usage from Messages is retained
when present, without changing inclusive output counts. The local end-to-end
runner passes 12 coding runs, three smoke checks, a six-request-cap check and a
rate-limit classification check: 61 local HTTP requests, zero external requests.
Its assertions verify nonzero process-tree metrics and inclusion of command
helpers. Clippy passes with the three documented inherited style allowances.


## Subsequent work

The historical suite and its 274/300 checkpoint above are preserved. The
[tool-efficiency follow-up](harness-tool-efficiency.md) records subsequent targeted
checks and the final 300/300 ledger. The [improvement roadmap](harness-improvement-roadmap.md)
separates implemented mechanisms from remaining diagnostics, scheduling, provider
coverage and optional capability work. The source audit confirms shared native
hosting and basic Todo UI already exist; neither requires a fresh architecture.
