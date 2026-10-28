# Log-tail candidate and repeated speed measurement — 2026-10-28

The owner authorized **up to 150 additional physical model requests** after the
[previous speed follow-up](harness-speed-2026-10-28.md). This report retains the
initial result and a fixed balanced confirmation; it does not replace the initial
result with only the more favorable sample.

## Candidate and validation

`read_file` now accepts optional `tail: true` with the existing line limit, avoiding
a separate line-count query when a caller wants final log lines. Default forward
reads remain unchanged. Tail mode requires zero offset and returns its actual
selected offset. The SHA-256 still covers the entire file. A prefix change outside
the selected tail therefore rejects an edit using the old hash.

The new test first failed because the parameter did not exist, then passed after
implementation. All 12 native-tool tests pass, covering permissions, containment,
file hashes, cancellation, tools and ordered batches. The frozen release passes
the local scripted evaluator's 12 coding runs, three smoke checks, cap handling
and rate-limit classification: 61 local HTTP requests, zero external calls.
Clippy passes with the previously documented inherited style allowances.

**The live model did not use this new parameter.** Trace inspection finds zero
`read_file` calls with `tail: true`. The feature is unit-tested and available, but
there is no live evidence that it caused the observed timing changes. Faster
three-request log tasks used existing tools. No context reduction became a default.

## Frozen setup

- Native source `498d12c84aeffb55f146ca95e6fbe06c414876e7`; release SHA-256
  `40e0881808567df2b924b49977b6b6a7aae4da415076c3766e25c0ec1e5881db`. Reference remains pinned to v0.99.1,
  commit `7206efd0c1dbf49ef38d06b63f3a96a3a7cee611`.
- Four unchanged coding tasks and external acceptance/protected-file checks;
  exact Sonnet 5.5, medium effort, adaptive thinking, 4,096 output tokens and
  **six physical requests per task**. Same subscription credential; no paid API
  fallback. No source, settings or fixture changes during the suite.
- Fresh checkouts/profiles and alternating arm order; both arms use the same
  admission proxy and upstream TLS. The reference also uses private-CA TLS on
  loopback, while native uses loopback HTTP.
- Primary comparison: three repetitions, 12 matched pairs / 24 runs.
- After seeing the primary result, one additional complete repetition of all four
  tasks was declared before dispatch, with reference first. It balances the total
  first-arm order two repetitions each. All four confirmation pairs run regardless
  of outcome; live testing then stops. This is an explicitly post-primary
  confirmation, not a claim of a preregistered independent study.
- Three preselected native Opus regression probes are reported separately and do
  not constitute a new repeated Opus speed comparison.

## Results: retain every stage

| Stage | Native completed | Reference completed | Native median | Reference median | Native / reference requests |
| --- | ---: | ---: | ---: | ---: | ---: |
| Initial three repetitions | 12/12 | 12/12 | 8.46 s | 8.17 s | 41 / 40 |
| Fixed fourth repetition | 4/4 | 4/4 | 7.27 s | 9.20 s | 13 / 14 |
| All four repetitions | 16/16 | 16/16 | 7.93 s | 8.44 s | 54 / 54 |

The initial 12-pair median still trailed the reference: **8.46s / 8.17s**.
Across all 16 matched pairs, native's median is **6.0% lower**:
**7.93s / 8.44s**. Mean task time is
8.66s / 8.99s. Native is faster in
9/16 pairs; the median paired time ratio
is 0.934. Both use 54 physical requests across 16 tasks.

This is a modest observed edge in the expanded sample. It does not establish
statistical significance, general speed superiority or a causal benefit from
tail mode. Provider latency is stochastic and both arms became faster compared
with the preceding suite. The initial and confirmation results are both retained;
no failed/capped result is excluded. All 32 comparative runs pass acceptance and
protected-file checks. All three targeted Opus probes also pass within the cap.

## Expanded resource and usage observations

| Metric, 16 Sonnet runs per arm | Native | Reference |
| --- | ---: | ---: |
| Median peak process-tree RSS | 20.43 MiB | 52.19 MiB |
| Median peak process-tree PSS | 9.28 MiB | 43.95 MiB |
| Median retained profile | 32.23 KiB | 179.20 KiB |

These measure headless CLI process trees and default profiles, not full desktop
apps or shared-server idle roots. Twenty-ms sampling can miss short-lived
children. The shipped feature surfaces and prompts/tools differ.

| Expanded provider counters | Native | Reference |
| --- | ---: | ---: |
| Uncached input | 186 | 21415 |
| Cache reads | 298306 | 1171454 |
| Cache writes | 58410 | 49677 |
| Output | 11606 | 12966 |

Counters are provider-reported and counted once. Unknown values remain unknown.
They do not establish actual subscription charges or per-task quota savings.

## Accounting and published evidence

This allocation used **119/150** physical requests: 108 for the
32 Sonnet runs and 11 for the three Opus probes. All 119 have HTTP 200 responses,
the requested served-model identity, medium effort, adaptive thinking and the
4,096-token ceiling. The original **300/300** and subsequent **293/300** ledgers
remain byte-for-byte unchanged; this allocation has its own append-only ledger.
The remaining 31 requests are unused. No additional live calls are planned for
this iteration and no budget was reset.

[All primary runs, confirmation runs and Opus probes](benchmarks/harness-tail-speed-2026-10-28.json)
include source/binary/fixture fingerprints, physical-request reservations,
provider counters, tool names and outcomes. Tool arguments, credentials, account
identities, prompts, private profiles and raw streams are excluded. Private
execution evidence remains under `.context/speed-tail-20261028/` on the cloud VM.
