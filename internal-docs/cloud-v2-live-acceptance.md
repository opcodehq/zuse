# Staging v2 operator acceptance driver

This operator tooling is not evidence of a certified v2 flow. Staging has applied
`0026` and `0027`; exact candidate activation is limited to the staging owner. Root
owns activation and each bounded GO. These scripts never enable rollout themselves.
The current factual outcome is recorded in the delivery ledger.

Run on Hetzner, outside the checkout, beside the captured cloud infrastructure
scripts. The owner session stays in `/etc/zuse/cloud/staging/owner-canary`.
Never copy that session, gateway tickets, transcript dumps or terminal output to
workspace/chat. Reports contain IDs, states, counts, booleans and durations.
No production endpoints are accepted. No Cargo build is needed. The driver uses the existing trusted operator Node at
`/var/tmp/zuse-harness-candidates.TIVvSo/node` (or explicit `--node-binary`).
Run `preflight` before GO: it verifies Node22+, WebSocket availability, binary
ownership/permissions and SHA, then reads the scoped API context. No download or
system-wide Node installation is required; the production runtime remains Rust.

## Prepare without side effects

Create a private configuration file (mode0600) with public metadata:

```json
{
  "apiOrigin": "https://api-rust-stag.zuse.sh",
  "owner": "user_REPLACE",
  "projectId": "project_REPLACE",
  "connectionId": "connection_REPLACE",
  "agent": "codex",
  "model": "gpt-6.1-sol",
  "provider": "boxd",
  "baseRef": "main",
  "candidateReleaseId": "REPLACE",
  "candidateSha256": "REPLACE_WITH_64_LOWERCASE_HEX"
}
```

`python3 staging-v2-acceptance.py validate --config /private/config.json` is fully
offline: it does not import the authentication helper, read the owner session,
open a socket or allocate a computer. `prepare` additionally takes `--state-dir
/etc/zuse/cloud/staging/owner-canary/v2-RUN` and mints durable task/message/action
identities and an isolated `zuse/v2-acceptance-*` branch; it does not call any API.
Use a distinct private directory for each run. The repository is never pushed.

Only after Root has verified the exact active candidate, Root creates a private
GO file with `approved:true`, the exact configuration's `owner`, `provider`,
`candidateReleaseId`, `candidateSha256`, the prepared `runId`, and an `expiresAt`
Unix-seconds deadline. Every effect requires that exact GO; it authorizes one
prepared run, never a broad owner cohort. The runtime manifest and actual provider
capability must both negotiate version2 before creation. No legacy fallback is
allowed. Exact recorded-resource Delete remains allowed after GO expiry so a
finished test cannot strand compute; creation/messages remain forbidden. The real API response must confirm `server-admitted-v2`.

## Run the actual flow

Every command below includes the same private `--config`, `--state-dir`, and
`--go-file` arguments. `--owner-dir` defaults to the existing private owner
profile. The scripts journal each exact request before POST. Run each mutation
once. A lost response stops the driver; use `inspect --step NAME` and the original
server action/receipt. Never mint another command or replay a possibly executed
action. A missing create response requires an independent database/idempotency
inspection by Root; the driver deliberately refuses automatic recreation.

1. `create`: admits one task and its first message atomically. It does not submit
   the first message a second time. Immediately run `gateway --observe-step first`.
   It watches real public-TLS gateway frames and reports live deltas separately
   from an already-complete snapshot. A snapshot alone is not streaming proof.
   `inspect --step first` checks the factual `launch:{workspace}` execution.
2. Before follow-up, start `gateway --observe-step followup` in a separate process.
   Wait for its private `observer-ready-followup.json` marker, then run
   `step --step followup`. The owner refresh lock is released while observing.
   The exact prompt demands a file edit, real shell test and a once-only append.
3. `step pause`, `step add`, `step edit`, `step remove`: queue operations use the
   actual API revision, preserve typed input and operate while auto-drain is
   paused. Removal must not create a native execution or a physical counter.
4. `step long-run`, then inspect its factual execution until running. Save an
   older factual turn from `WatchSessions.last_completed_turn` (the observer does
   this when available). While the new run is active, `step stale-stop` must return
  409 and must not cancel the newer run. Do not fabricate a native turn ID.
5. `step steer-add`, then `step steer` while that same run is active. This uses the
   typed server SteerNow path and actual queue revision. Confirm factual control
   progress and observe its effect; acceptance must not count merely admitted
   delivery as executed steering. If the agent cannot steer, record that case as
   failed rather than issuing an opaque native RPC fallback.
6. `step urgent-add`, then `step send-now`: selected item executes ahead of queued
   unleased work using its exact internal interrupt dependency, preserving the
   prior queue pause. Its prompt includes a foreground wait to expose the Stop
   window. `inspect --step urgent-add` follows its claimed execution identity.
7. `step stop` while that admitted Run is active: inspect original target/control
   progress. Ordinary Stop must pause auto-drain without deleting future queue
   content. Also run Stop against the pre-native/input-preparation window in a
   separate explicitly approved scenario (`scenario:prelease-stop`): `create`,
   immediately `step stop`, inspect the original launch and control, then send only
   `after-stop`. That scenario's physical check requires no first-turn counter and
   exactly one after-stop counter. If the original was already leased, record the
   different observed race; do not label it prelease proof. API/guest fixtures alone are not
   live proof of that race. Never label an uncertain result cancelled.
8. A new explicit `step after-stop` may execute while the queue remains paused.
   Observe it with `gateway --observe-step after-stop`. `step resume-queue` is an
   explicit independent action; it must not be implicitly sent by follow-up.
9. Close the observer, then invoke a new `gateway` process and `history`. Fresh
   gateway tickets and canonical delta application prove reconnect. History must
   show stable message IDs, complete tools and the authoritative queue pause.
   History reads never request a wake. Test paused-history separately after the
   approved lifecycle sleep; this driver does not implicitly sleep/resume.
10. `gateway --gateway-action verify` reads files and uses a real terminal to run
    a read-only filesystem check. It requires exactly one physical line for each
    first/followup/after-stop counter, no counter above one, and no removed queue
    execution. This is independent of API admission/ACK counts. Its terminal
    command only reads the isolated directory; uncertain reads may be repeated.

A run with any unreviewed `outcome-unknown`, missing factual progress, missing tool
resolution, duplicate counter, missing stream delta, or stale-target failure is
not accepted. Stop and inspect files/receipts before a separately reviewed
ReviewUnknown/new command. Never replay the old CID. The driver does not clear
uncertainty or bypass lane fencing automatically.

## Cleanup and independent verification

Before requesting Delete, use `staging-v2-cleanup-verify.py inventory` with exact
`--workspace`, `--owner`, a private `--database-url-file` (verified TLS DSN only)
and an immutable private `--targets-file`. Its explicit read-only staging SQL
records every allocation/create machine ID before deletion can purge metadata.
It reads no transcript or credential content. No owner JWT is required.

Then `staging-v2-acceptance.py delete` persists the deletion command once.
`cleanup-check` waits by explicit operator invocation for API state `deleted` but
**does not certify cleanup**. Finally run independent verifier `verify` with the
same target inventory and `--provider-env-file` pointing to staging's private
worker environment, plus `--boxd-sdk` for the pinned0.2.14 SDK when needed.

The verifier requires deleted SQL state/desired state, terminal deletion receipt,
zero pending jobs, cleared provider/credential/key references, then performs only
provider GET inspections for every original machine. It uses existing pinned
Boxd/Boat adapters' `absent()` methods; no create, command, stop, delete or snapshot
method is called. Only explicit NotFound counts as absent. Timeout/auth/permission
failure never counts as cleanup. E2B/Daytona independent inspection is deliberately
unsupported by this helper; use their reviewed adapter/operator verifier before
marking those future cases complete. Test branches stay uncommitted and unpushed.

## Dry verification already performed

Seven Python tests cover offline scope validation, exact GO fences, intent-before-
POST with lost-response/no-replay behavior, typed stable MID/cwd transport, queue
revision/Stop/Resume separation and the RAM-only ticket seam. Python compilation
and Node syntax checks passed. No live API, provider allocation, migration,
configuration switch or deployment was performed while preparing this tooling.
