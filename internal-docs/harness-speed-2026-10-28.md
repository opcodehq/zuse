# Native workflow speed follow-up — 2026-10-28

The [preceding full comparison](harness-followup-2026-10-28.md) showed Sonnet
completion at 12/12 but slower median elapsed time. This follow-up changes the
schema-owned workflow guidance and measures it again; it does not claim a win
from a smaller selected sample.

## Change and validation

Captured traces showed discovery before reads of named files, separate model
responses for writes and known verification, and failed interpreter aliases.
Native tool/storage work took milliseconds; model waits dominated seconds.

The guidance now explicitly requests known-path reads and ordered edits followed
by predictable verification in the same response. It retains fresh hashes,
permissions, actual verification, project-specific commands and concise reports.
No model, medium effort, context reduction, output ceiling or per-task cap changed.
This applies to new roots; recovered roots retain their saved instructions.

The durable composition test executes write/read/verification both separately
and as one batch, verifies that the command sees preceding writes, checks durable
receipts and ordered tool results, and observes two versus four model admissions.
Native tool/permission and scheduling/cancellation tests pass. Text-guidance
adherence has no deterministic unit-test seam; the live comparison remains the
speed gate. The new frozen release also passes the local HTTP evaluator's 12
coding runs, three smoke checks, cap handling and rate-limit classification:
61 local requests, zero external model calls.

## Full Sonnet comparison

Frozen native source `2710527af34531349e421ea6137a31d4e8c51ee3`, release SHA-256
`8b1a12ff8430f5dc0d125ab3324bfa821671dd92421586dbf99c4a5df8844bcd`. Same pinned reference v0.99.1.
The same four task prompts and acceptance checks run three times per arm:
24 coding runs, Sonnet 5.5 medium, 4,096 output tokens and six physical requests
per task. Fresh profiles/checkouts, alternating arm order and the same subscription
credential and metering proxy. No extra inference smoke calls were sent.

| Metric | Native | Reference |
| --- | ---: | ---: |
| Completed and accepted | 12/12 | 12/12 |
| Median elapsed | 11.44 s | 10.17 s |
| Physical requests across 12 tasks | 42 | 41 |
| Median peak process-tree RSS | 20.15 MiB | 57.43 MiB |
| Median peak process-tree PSS | 9.17 MiB | 49.26 MiB |

**The speed goal is not met.** Native median elapsed is 12.5% higher than
this contemporaneous reference median. Its observed median improved 18.1%
from the preceding native 13.98s, and physical requests decreased 49 to 42.
The earlier reference median was 10.58s; it also varied between suites. These
numbers support fewer round trips and an observed improvement, not isolated
causality or an overall speed win.

All 24 runs pass code acceptance and protected-file checks. Every physical
response is HTTP 200 with the requested served model; no capped or failed run
is excluded. Two preselected harder native Opus probes (noisy evidence and
instruction retention) also pass, using five and four requests. These two probes
are not a replacement for the preceding source's 12/12 full Opus result or a new
Opus speed comparison.

## Accounting and limitations

This phase consumed 92 additional physical requests: 83 for
Sonnet and nine for the two Opus probes. It appends reservations 202–293 to the
existing follow-up allocation, now **293/300**. Its first 201 reservations, the
preceding report and the original **300/300** historical ledger are unchanged.
Seven requests remain; they cannot fund a full repeated matched comparison.
No paid API fallback or quota/dollar savings is inferred.

The reference has additional loopback TLS, and prompts/tool surfaces differ.
Provider latency is stochastic; the fixtures and sample are small. Twenty-ms
process sampling may miss short-lived children. The same performance caveats
as the preceding suite apply. Existing Opus full-suite timings predate this
instruction change, and are labeled accordingly.

[All 24 Sonnet runs and two Opus probes](benchmarks/harness-speed-2026-10-28.json)
are published with metadata and provider counters, excluding credentials, account
identities, prompts and raw streams. Private evidence and the red-capable speed
gate remain in `.context/speed-followup-20261028/`.
