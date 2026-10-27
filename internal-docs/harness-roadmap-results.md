# Native roadmap implementation results

## Shared-host idle measurement

Cloud Linux VM, release native example and pinned reference release, one process
host per arm, 1/2/5/10/20 roots, three repetitions with alternating arm order.
All 30 runs completed. No prompts or model requests were sent. Each run used an
isolated profile/checkout; reference egress was disabled and authentication was
synthetic. Exact reference provenance and controls remain in the ignored
`.context/harness-roadmap-implementation/shared-host-manifest.json`.
Raw samples are in `shared-host-final/report.json` in that directory.

| Roots | Native settled PSS MiB | Reference settled PSS MiB | Native ready ms | Reference ready ms |
| --- | --- | --- | --- | --- |
| 1 | 4.23 | 41.56 | 5.6 | 56.4 |
| 2 | 4.45 | 41.52 | 7.6 | 60.2 |
| 5 | 5.20 | 41.66 | 12.3 | 57.2 |
| 10 | 6.53 | 41.83 | 31.4 | 65.1 |
| 20 | 8.89 | 42.97 | 43.9 | 95.2 |

At 20 idle roots native PSS was approximately 79% lower in this measurement.
After closing those roots, native settled PSS was 4.74 MiB. The reference's
published control did not expose the same close operation, so its post-close
value is unknown. Readiness includes process initialization and session creation,
not generation latency. The native 20-root readiness samples ranged from 39.5
to 223.7 ms, versus 76.5–113.8 ms for the reference; these shared-VM timings have
visible outliers and do not establish a general startup-speed advantage.
The first repetition overlapped the short local acceptance runner; later
repetitions ran without that runner. PSS apportions shared pages; summed RSS is not physical
memory. Sampling peaks cover the settled interval, not all startup transients.

The native example opens real owned roots, journals, native tools, a shared HTTP
transport and process managers. It has no desktop UI, model turn, external MCP
server or application callback projection. The reference server also has
additional daemon services; these are different feature surfaces. This supports
a narrow idle-core resource comparison, not active-workload superiority or a
claim that live completion rates improved. The previously measured live latency
and capped completion results are unchanged pending a new evaluated live budget.

## Implemented behavior

- Bounded concurrent reads with durable admissions and ordered model results;
  configurable child/read limits and concrete delegation guidance.
- Stage timings, partial-attempt telemetry, actual cache-read observations and
  bounded prefix-change diagnostics without retaining prompt payloads.
- On-demand local skills, fresh AST outlines and coalesced native peer-edit notices.
- Shared lazy MCP stdio/HTTP connections with explicit trust, credential/permission
  isolation, bounded frames, no SDK result caching and no automatic uncertain retries.
- Mixed text/image external results and explicit route reasoning capabilities.
- Application-native integration, strict profile settings and updated operation docs.

See [configuration and limitations](harness-native-capabilities.md). No model or
effort downgrade, default context reduction, live cache-warming traffic or extra
model request was introduced. The global live ledger remains 300/300.

## Validation

- 85 core/MCP tests pass; the separate explicit disk benchmark remains ignored
  in the normal suite. Includes scheduling order/cancellation, native permissions,
  skills containment, AST freshness, transport continuity/images, journals,
  MCP legacy/modern HTTP, isolated stdio pooling, oversize/disconnect handling
  and owned descendant shutdown. A definitive external server error enters the
  existing completed-failure path; ambiguous outcomes retain reviewable starts.
- 34 application checks pass: 24 native integration/recovery checks, three API
  profile checks and seven model catalog/account routing checks.
- Release examples build, final engine check and minimal-core check pass.
- The release deterministic evaluator passes 12 coding runs, three smoke runs,
  one request-cap run and one rate-limit run: 61 local HTTP requests, zero external
  model requests. This checks the runner/transport/tools/recovery, not model quality.
- Clippy passes with existing `collapsible_if`, `redundant_closure_call` and
  `too_many_arguments` lint classes allowed; the unrestricted warning-as-error
  run still reports pre-existing code in file/process/state helpers.

Commands and logs are retained under the ignored implementation evidence directory:

```sh
cargo test -p zuse-harness -p zuse-harness-extensions
cargo test -p zuse-engine --lib native_
cargo test -p zuse-engine --lib harness_api
cargo test -p zuse-engine --lib harness_models
cargo check -p zuse-engine
cargo check -p zuse-harness --no-default-features
cargo clippy -p zuse-harness -p zuse-harness-extensions --all-targets -- \
  -D warnings -A clippy::collapsible_if \
  -A clippy::redundant_closure_call -A clippy::too_many_arguments
```
