# Native tool workflow follow-up

Cloud VM, 2026-10-27. This follows the fixed suite in
[harness-reference-evaluation.md](harness-reference-evaluation.md), rather than
replacing its 12-runs-per-model results with a smaller sample.

## Findings and changes

Captured native runs repeatedly performed a shell read, attempted to replace an
existing file with `expected_hash: null`, then needed another model turn to read
a fresh hash and retry correctly. Several runs performed extra cleanup commands
after verification. Long final messages also increased generated output and
latency. Local engine work was small compared with provider request time.

Native tool schemas now own stable workflow guidance, composed with application
instructions by `ToolHost`. It describes direct reads of known paths, grouping
independent reads and related edits, the existing fresh-hash requirement, checkout
relative paths, ordinary JSON escaping and combined verification when practical.
Routine final status is limited to five lines, with exceptions for requested
explanations. Further necessary tests and failure investigation remain required.
No model, reasoning level, output ceiling, permission or context policy changes.

Creating a file that already exists now fails under the checkout's file lock
before replacement construction, temporary-file writes or fsync. The error
explains how to obtain and supply the edit hash. The final exclusive `linkat`
still arbitrates concurrent creation; the preflight is not the atomic guarantee.
Tests prove early rejection skips the transform and a writer arriving between
preflight and publication retains its content. Existing fresh-hash, containment,
cancellation, permissions and concurrent-root tests still pass.

Guidance is captured in new roots' durable system instructions. Existing resumed
histories keep their prior instruction snapshot; they are not silently rewritten.
A future versioned instruction-refresh design must account for explicit changes,
recovery integrity and cache invalidation.

## Targeted live checks

Both exact 5.5 models remain at medium with the original six-request run cap and
4,096-token output ceiling. Fixtures and external acceptance checks are unchanged.
The first five runs use workflow guidance, before the final five-line refinement.
Prior values are medians of three historical native runs for the same task; new
values are one run per row, so latency comparisons are exploratory.

| Model / task | Previous requests | New requests | Previous elapsed | New elapsed | Previous completed | New completed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Sonnet / boundary | 4 | 3 | 11.90 s | 13.36 s | 3/3 | 1/1 |
| Sonnet / instruction retention | 6 | 4 | 19.96 s | 17.57 s | 3/3 | 1/1 |
| Opus / noisy evidence | 6 | 6 | 19.61 s | 13.19 s | 0/3 | 1/1 |
| Opus / instruction retention | 6 | 6 | 20.39 s | 16.36 s | 2/3 | 1/1 |
| Opus / boundary | 5 | 4 | 12.67 s | 11.37 s | 2/3 | 1/1 |

All five completed, passed acceptance and preserved protected files. For the two
Sonnet cases the new median is 15.47 s against 15.93 s for the corresponding
historical case medians. This is not the previous suite's overall 13.91 s median,
and does not prove it decreased. The three Opus cases completed in this follow-up;
that does not replace the original 7/12 with a claimed 12/12.

After adding the five-line final-status guidance, a final Sonnet boundary probe
completed in **4.87 s / three requests**, with acceptance and protected files
passing. Its immediately preceding same-task run was **13.36 s / three requests**.
Final-response output decreased **302 to 129 tokens**; total output decreased
571 to 388 tokens. All three provider request times also fell, including the first
two, so attributing the entire speedup to the final-response guidance would be
incorrect. Repeated matched checks are needed to estimate its actual effect.

The five initial checks consumed 23 physical requests; the final probe consumed
three. The existing global ledger is now **300/300** including all earlier work.
The last probe had only three global reservations available; it completed within
them. No additional live requests were sent and no new ledger was substituted.
Credentials remain outside tracked files and report bodies.

Evidence is under `.context/round-trip-followup/`: `report.json`, per-request SSE,
per-run acceptance/resources, and `concise/report.json` for the final probe.
The precise binary SHA-256 is recorded in each phase's report. All 69 harness and
example checks pass, with one pre-existing manual benchmark ignored. The final
release also passes the local HTTP runner's 12 coding runs, three smoke runs,
cap and rate-limit checks (61 local requests, zero external requests).

## Next priorities

Reduce repeated discovery and tool mistakes before pursuing smaller allocator
savings. Evaluate compact final reports separately from tool efficiency, measuring
first-token latency, generated tokens, physical requests and successful completion.
Keep failed/capped runs in every score and preserve actual quality checks.

The accompanying private source audit compares the published reference's
provider, cache, tool, session and extensibility mechanisms with this codebase.
A valid multi-session comparison must measure each implementation's shared host,
not infer shared-server behavior from separate CLI processes. Do not weaken hash
checks, allow implicit retries of uncertain side effects, or enable context
reductions to produce a better benchmark score.
