# Repeated native harness comparison — 2026-10-28

This is a new full live suite after the workflow and orchestration changes.
It preserves the earlier [historical evaluation](harness-reference-evaluation.md)
and [targeted probes](harness-tool-efficiency.md); those measurements are not
relabeled as results for the current implementation.

## Setup and accounting

- Native source: `b7c8c775af454f4b10f74a7ea5403dffcbe42126`; frozen release SHA-256
  `47f57d1df865853bd3e293fe4dcb96ee18cc78853e8945e228e35107f9feb7a1`. No harness changes during the suite.
- Same pinned reference CLI v0.99.1, commit
  `7206efd0c1dbf49ef38d06b63f3a96a3a7cee611`.
- Four unchanged tasks, three repetitions, two harnesses and two models:
  **48 coding runs**, plus four one-request authentication/model smoke runs.
- Exact `claude-opus-5-5` and `claude-sonnet-5-5`, medium reasoning,
  4,096 maximum output tokens and **six physical requests per coding run**.
  Both arms use the same subscription credential; no paid API fallback.
- Fresh checkouts/profiles; unchanged external acceptance tests and protected-file
  checks; arm order alternates by repetition. The task prompt bytes match the
  historical suite. No context reductions or cache-warming requests.
- The owner's explicit request to rerun authorized a separate follow-up allowance.
  The original **300/300** ledger is unchanged. This suite uses
  **201/300** follow-up requests, including all smoke requests,
  retries, summaries and children if used. Reservations reconcile exactly with
  proxy-observed physical requests; no limit was increased within a task.

## Completion and elapsed time

Completion requires normal process completion, passing code acceptance and
unchanged protected files. A code fix without final completion still fails.
Latency values below use matched pairs that both completed; their sample counts
are explicit. All outcomes remain in the published run data.

| Model, medium | Historical native completed | Fresh native completed | Fresh reference completed | Fresh native median elapsed | Fresh reference median elapsed | Matched pairs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Opus 5.5 | 7/12 | 12/12 | 12/12 | 12.01 s | 14.66 s | 12 |
| Sonnet 5.5 | 12/12 | 12/12 | 12/12 | 13.98 s | 10.58 s | 12 |

### Per-task results

| Model / task | Native completed | Reference completed | Native physical requests | Reference physical requests | Native median elapsed | Reference median elapsed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Opus / boundary-fix | 3/3 | 3/3 | 14 | 11 | 12.23 s | 11.32 s |
| Opus / multi-file-change | 3/3 | 3/3 | 15 | 10 | 14.75 s | 9.46 s |
| Opus / noisy-evidence | 3/3 | 3/3 | 17 | 16 | 11.65 s | 17.06 s |
| Opus / instruction-retention | 3/3 | 3/3 | 13 | 12 | 11.79 s | 18.51 s |
| Sonnet / boundary-fix | 3/3 | 3/3 | 14 | 9 | 13.74 s | 10.22 s |
| Sonnet / multi-file-change | 3/3 | 3/3 | 12 | 9 | 16.63 s | 8.81 s |
| Sonnet / noisy-evidence | 3/3 | 3/3 | 11 | 12 | 15.93 s | 13.94 s |
| Sonnet / instruction-retention | 3/3 | 3/3 | 12 | 10 | 11.16 s | 9.91 s |

## Resource and usage observations

Live resource measurements cover each CLI process tree, not the desktop app or
a shared host. The separate [shared-host idle test](harness-roadmap-results.md)
uses a different lifecycle and must not be combined with this table.

| Model | Native / reference physical requests | Native / reference median peak RSS | Native / reference median peak PSS | Native / reference median profile size |
| --- | ---: | ---: | ---: | ---: |
| Opus 5.5 | 59 / 49 | 22.23 / 56.84 MiB | 10.29 / 49.17 MiB | 41.15 / 200.40 KiB |
| Sonnet 5.5 | 49 / 40 | 20.35 / 57.18 MiB | 9.12 / 49.25 MiB | 36.60 / 178.89 KiB |

| Model / arm | Total input per successful task | Output per successful task | Cache-read tokens | Cache-write tokens | Uncached input tokens |
| --- | ---: | ---: | ---: | ---: | ---: |
| Opus / native | 33,740.42 | 994.00 | 358121 | 46558 | 206 |
| Opus / reference | 93,669.58 | 989.75 | 1085975 | 22692 | 15368 |
| Sonnet / native | 26,973.92 | 715.08 | 281398 | 42105 | 184 |
| Sonnet / reference | 76,723.67 | 786.50 | 882921 | 21724 | 16039 |

Input totals include cache reads, cache writes and uncached input exactly once.
Per-success numerators include unsuccessful coding runs, if any; smoke runs are
excluded from task-efficiency metrics but included in the global request count.
Nested iteration counters are not added again. Unknown counters stay unknown.
Token counts do not establish actual subscription charges or quota savings.

## Validation and limits

All 4/4 smoke runs passed. All 48 coding runs completed and passed acceptance/protected-file checks; no failed or capped runs occurred in this suite.

Every physical dispatch is checked for the selected account, exact model,
medium effort and output ceiling before forwarding. Successful responses retain
served-model identities; no served-model mismatch was observed. The native and
reference binary checksums remain unchanged, and the historical ledger hash
matches its pre-run value.

These are small coding fixtures and only three repetitions per task. Improvements
are observed on this suite; they do not prove general quality equivalence or
eliminate all ways to exhaust the request cap. Model latency is stochastic.
One native Sonnet noisy-evidence run took 36.38 seconds; it is retained, not
excluded. Neither an isolated fast run nor the medians identify which harness
change caused a timing difference.

Both arms share the metering proxy and upstream TLS path. The reference also
uses private-CA TLS on loopback; native uses loopback HTTP. Prompts, native tools
and shipped feature sets differ. Resource sampling is every 20 ms and may miss
short-lived children; CPU/write counters are sampled estimates. Profile sizes
exclude checkouts, binaries and external evidence. The benchmark establishes
observed outcomes under these settings, not blanket speed/cost superiority.

## Published evidence

[Sanitized run-level JSON](benchmarks/harness-followup-2026-10-28.json) contains
all 48 coding runs, all four smoke runs, source/binary/fixture fingerprints,
physical request counts, provider usage counters, outcomes and summary metrics.
It excludes credentials, account identifiers, prompts, raw responses and private
profile contents. Private execution scripts, raw streams and the separate ledger
remain in `.context/rebenchmark-20261028/` on the cloud VM.
