# Cloud timing measurements

Measured October 29, 2026 from this cloud development workspace. These are
component measurements, **not complete workspace-to-first-reply timings**. No real
model inference or customer authentication was performed.

## Live provider lifecycle

Create begins at the provider create request and ends at the first successful
shell command. Resume begins at the resume request and ends when the disk/RAM
continuity probe command returns. Provider authentication/read checks are separate.
No prepared project image, Git checkout or agent process is included.

| Scenario | Create → command, median (range) | Resume → command, median (range) | Samples |
| --- | ---: | ---: | --- |
| Boxd | 787 ms (773–813) | 481 ms (455–487) | 3 complete cycles |
| Boat immediate | 971 ms (928–2,435) | 630 ms (603–663) | 4 creates, 3 completed resumes |
| Boat forced cold restore by resizing | 1,073 ms | 2,066 ms | 1 cycle |

Small sample counts do not establish production percentiles or capacity. Boxd's
production Rust auth/inspect/exec probe also passed before and after resume on all
three machines. The probe time is additional to the shell-readiness totals above.

One Boat attempt acknowledged Stop in 379 ms but did not reach an accepted sleeping
state within 90,065 ms. Its resume was not attempted. A separate recheck completed
normally; retain the failed sample rather than discarding it. This does not establish
a fleet failure rate or root cause. The original failure receipt predates enum-state
observations; the benchmark now records bounded state counts on timeout for future
diagnosis. The [Stop investigation](boat-stop-reliability.md) subsequently found
that the benchmark ignored the provider's Stop operation status; this diagnostic
gap is fixed with operation-aware tests. The original provider-side cause is still
unknown because a fresh live verification hit API connection timeouts. Confirmed suspension took another 7.8–9.4 seconds on the three successful
immediate Boat cycles and 25.6 seconds on the cold test. Boxd confirmation took
2.2–2.7 seconds. Stop acknowledgement must not be treated as settled suspension.

Auth/read preflight medians were 783 ms for Boxd and 477 ms for Boat immediate.
They include the benchmark's authenticated list request; they are not the same as
account OAuth renewal, workspace grant delivery, or a required per-launch preflight.

Boxd retained disk and the RAM-only responder on all three cycles. Boat immediate
retained both on the three completed resumes. Forced cold restore retained disk,
changed boot ID and lost the RAM-only responder. A cold resume needs process
restart/reconnection, even when the provider makes the machine command-ready quickly.

Eight uniquely named machines were created; all eight were deleted and absence
confirmed. No existing workspace, database, DNS or running service was modified.

## Local credential and harness overhead

| Measurement | Result | Scope |
| --- | ---: | --- |
| Initial sealed credential grant | 4.11 ms median; 4.23 ms p95; n=30 | localhost HTTP + production Rust envelope crypto; synthetic authority/account |
| Forced credential grant renewal | 4.15 ms median; 4.53 ms p95; n=30 | same; no actual OAuth renewal |
| Cached credential lookup | 0.00123 ms median; n=1,000 | in-process; zero HTTP requests |
| Managed launch wrapper | 49.34 ms median; 68.98 ms p95; n=50 | Python release selection + exec `/bin/true`; no real harness |
| Direct launch baseline | 2.13 ms median; n=50 | exec `/bin/true` from the same benchmark runner |
| Release selection within Python | 0.207 ms median; n=1,000 | existing pin, manifest/path checks |
| Synthetic subprocess → text | 64.03 ms median (62.04–67.98); n=10 | fake Codex protocol peer; not model first-token latency |
| Stop → interrupted Done during renewal | 0.158 ms median (0.122–0.185); n=10 | same peer; resolver simulates a 30-second delay |

The managed wrapper adds approximately 47 ms relative to the direct baseline on
this development VM. Its first pin-and-launch took 47.26 ms. It made no network
requests. This is small compared with provider/network setup, but it excludes the
runtime version probe and actual Claude/Codex startup. The synthetic artifact tree
and synthetic promotion evidence existed only in a temporary directory and were
removed; no real release channel was promoted.

The local grant numbers prove software/crypto overhead, not the latency of a remote
account authority or token renewal. The control fixture proves that a slow auth
resolver does not stall streaming or Stop. It does not measure upstream provider
interruption, production load, or mobile/browser rendering.

## Remaining end-to-end measurement

A sub-3-second/sub-5-second complete cloud start has **not** been established. That
needs the deployed API/relay/schema wiring and supported test model credentials,
then measurements of runtime enrollment, credential delivery, repository readiness,
first real agent token, reconnect, and cold resume. Existing deployment gaps are
tracked in [the provider verification report](cloud-provider-verification.md).

Raw receipts are gitignored under `.context/cloud-timings/`: `summary.json`,
`boxd-{1,2,3}.json`, `boat-{1,2,3}.json`, `boat-recheck.json`, `boat-cold.json`,
`credentials.json`, `harness-overhead.json`, and `control-latency.json`. The scripts
used for local launch measurement and aggregation are retained there as well.

Reproduce the credential component probe with:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  cargo test --locked -p zuse-cloud-runtime --lib \
  grants::tests::credential_delivery_timings -- --ignored --exact --nocapture
```

The fixture control test also emits a scoped timing JSON record:

```sh
cargo test --locked -p zuse-providers --test codex \
  brokered_renewal_does_not_block_streaming_or_stop -- --exact --nocapture
```
