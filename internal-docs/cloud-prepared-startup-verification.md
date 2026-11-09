# Staging startup verification, November 2, 2026

**Not accepted.** Prepared startup p95 exceeds five seconds and memory resume p95
exceeds two seconds. These are measured results, not engineering budgets.
No warm pools, preallocated account computers or speculative wakes were used.
Every fresh machine was created after its explicit task request. A software-only
prepared base image contains no account credential or repository checkout.

## Sequential measurements

Thirty fresh Boxd + Codex starts completed their test tasks. Thirty memory resumes
were then measured on the first test computer, preserving its original runtime and
native Codex processes. Percentiles use nearest-rank p95 and median p50.

| Observed interval | Samples | p50 | p95 | Maximum |
|---|---:|---:|---:|---:|
| Send → actual Git fetch process spawn | 30 | 22.557 s | 27.445 s | 28.827 s |
| Send → native Codex `turn/start` acknowledgement | 30 | 24.475 s | 29.610 s | 29.891 s |
| Resume request → API ready observed | 30 | 3.910 s | 4.401 s | 4.647 s |
| Resume request → authenticated viewer gateway verified | 30 | 5.602 s | 6.411 s | 11.611 s |

Resume timings include status polling and viewer verification overhead. They are
observed end-to-end timings, not isolated provider restore durations. Each resume
preserved kernel boot ID, runtime and Codex PIDs/start ticks, branch and the local
execution counter. The counter remained one and the Git-fetch count remained one.
All thirty fresh test computers were deleted and their provider absence checked.
Disk restoration was not measured.

The sequential cohort used API/worker SHA
`5c0f4f3cb145c6baef3350ae3036e494bcb97acfb98d7d3a038bc8771bfd094d`
and guest SHA
`607c23c769ff0f00807cbed0ab49f0914f5cee6a4b8b0f4f25feb022e99c176f`.
The prepared artifact was `snap_13485d42a1cb4be1b5e1eda3f328ae22`.
Private receipts are under ignored `.context/startup-verification/`; owner-session
credentials, temporary gateway tickets and database connection strings are not
included in this report.

## Where time went

One traced application start issued **165 allocation SQL statements before guest
launch**, with **9.374 seconds of cumulative statement duration**. Guest launch
started **18.896 seconds after Send**. Statement totals can include overlapping
work and are not a complete wall-clock decomposition.

That trace included 39 existence queries, 21 workspace reads, 24 commits,
15 allocation reads, 17 ownership/generation reads, 16 advisory-lock statements
and seven base-registry reads. Lock waits reached two to three seconds. The
allocation continuation also repeated a substantial pass after a provider helper
reported pending, with a deferred retry delay.

The same trace measured provider creation at about 571 ms, runtime identity setup
at 203 ms, bootstrap HTTP at 1,320 ms and engine assembly at 37 ms. A native Codex
turn-acceptance round trip took 128 ms. These are individual observations, not
percentiles. The old enrollment duration spans orchestration and handoff; it does
not mean generating an identity itself took that long.

The independent provider-adapter baseline reached an installed-runtime command in
897, 2,454 and 2,575 ms. Its three samples cannot certify a provider percentile,
but the application trace establishes substantial orchestration overhead.

The next performance work must reduce sequential database round trips and repeated
allocation passes while preserving ownership checks at effects. Switching languages
or only optimizing identity generation cannot remove this measured overhead.

## Concurrency and fault findings

The first ten-concurrent-start cohort admitted three requests; seven failed with
`github_observation_superseded` before any workspace allocation. Two admitted tasks
completed the observer checks; the third encountered a gateway observation failure.
All three admitted provider machines were deleted. A separate scoped database
audit verified that rejected requests admitted no workspace. The cleanup harness
was corrected to query workspace IDs: deletion clears idempotency keys, so the old
key-based query incorrectly reported database cleanup failures.

A regression test reproduced the authorization race with ten simultaneous real
HTTP/database observers: nine failed before the fix. The fix serializes observation
per owner/installation within an API process, without caching provider authorization
or holding a database connection while waiting. Every caller still reads current
provider metadata and uses the existing durable observation nonce. Regression,
revocation and unlink tests pass. Multiple API replicas retain nonce fencing;
this change does not claim cross-replica coordination.

The fix is deployed to staging API/worker SHA
`30424d1f1266ad56ef3dd534113b211dcb6c34c1dab09342aa014cc3fc9053db`.
The separate ten-start rerun admitted all ten requests. Two tasks completed their
observer/exact-once checks; eight recorded durable repository-preparation failure.
The two successful tasks spawned Git at 41.929 and 45.655 seconds and accepted
native turns at 45.483 and 60.156 seconds. Missing failed-start measurements are
not treated as zero or discarded to manufacture a ten-sample percentile. The
original observer had no native terminal event for failed preparation; later
observations fail immediately for workspace IDs whose durable repository stage
was already failed. This is a test diagnostic, not an application recovery path.

All ten provider machines were independently verified absent. Database deletion
was confirmed in a subsequent workspace-ID audit: nine deletions finished after
the controller's cleanup observation window. Original receipts retain their
initial cleanup observation; the later audit is separate.

With Git deliberately stopped, the native agent accepted its prompt while local
preparation remained pending. Two fresh authenticated read-only gateway connections
succeeded, each observing one user message. The subsequent full completion observer
reported `gateway_reconnect_required`; that test is **not a completed pass** and
must reconnect to observe the original task, without resubmitting it.

The Stop fault test exposed a correctness bug: preparation became cancelled and
no task marker executed, but the fetch process remained alive. A deterministic
regression now exercises a stopped process group. The local fix propagates the
preparation cancellation token into the blocking fetch loop, kills the process group
and reaps the child before returning. The regression and all runtime unit tests
pass. **This guest fix is not part of the measured deployed image and has not yet
passed the live Stop test.**

Both fault-test machines were deleted and independent provider absence was verified.

## Test coverage and remaining gates

- Cloud library: 143 passed; 70 integration tests explicitly ignored by the unit run.
- Runtime library after the local fetch-cancellation fix: 60 passed; four ignored.
- Engine library: 742 passed; three ignored.
- Seven explicit PostgreSQL integration checks passed for startup acknowledgement,
  allocation fences, preparation retry handling, enrollment, gateway ownership and
  retained candidate/provider bindings.
- Authorization concurrency, standalone revocation and catalog unlink integration
  checks passed against isolated PostgreSQL and real local HTTP fixtures.
- The guest executable passes compilation checks with the cancellation fix.
- Publication hygiene, supervisor, installed-image bootstrap and release tooling
  checks passed; the prepared-image publication and idle proof passed on staging.

Expired credentials have fixture coverage, but a live expired connected-agent account
was not tested: the shared connected account was not revoked or disconnected.
Required setup failure, disk restoration and complete first-model-output percentiles
remain unverified. First-output collection was incomplete in the sequential cohort.
All thirty sequential samples recorded one provider create, one inspect, one
readiness adapter call, one initial-source verification, one bootstrap helper,
one boot-credential upload and one runtime-launch command in their recorded
provider phase counters. These are adapter observations; proof validation and
readiness can be local, and an SDK call can perform additional internal HTTP.
SQL/profile counts and explicit driver operations are available, but a complete
raw provider/HTTP request-count accounting was not collected. Do not describe these
acceptance gates as passed or extrapolate results to other providers or agents.

## Rollout state

The staging fast-start account flag is disabled after the failed acceptance run.
The previous prepared-base selection is restored; all three pre-existing computers
remain bound to their admitted images. Staging API/worker retain the authorization
race fix and are healthy. Production API/worker, staging auth worker and edge
service were unchanged. The newer test base remains a published staging artifact;
it is not a pool of running sandboxes.

The first rollback attempt correctly failed readiness and automatically restored
its prior configuration: retained exceptions duplicated the restored default
candidate. The corrected rollback omits those redundant exceptions, retains the
one computer using another image, and passed readiness. No existing computer was
restarted, resumed, deleted or replaced by rollback.
