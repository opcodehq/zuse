# Boat Stop reliability investigation

October 29, 2026. The prior timing run acknowledged Stop in 379 ms but failed to
confirm an accepted sleeping state within 90 seconds. That machine was deleted as
an isolated benchmark resource; no per-operation observations survived. Its precise
provider-side failure cannot be reconstructed from the original receipt.

## Confirmed defect in our verification

The benchmark only read `sandbox.state` and ignored `stop.id` and `stop.status`.
A synthetic response carrying `stop.status=failing` reproduced the blind timeout.
The regression went red with `ProviderStateDeadline` instead of the expected
`BoatStopFailing`, then passed after the operation-aware handling was added.

This is an observability/verification fix. It does not establish that a snapshot
failure caused the original timeout, or repair Boat's snapshot infrastructure.

Boat's current documented contract distinguishes:

| Stop operation status | Meaning | Application behavior |
| --- | --- | --- |
| `pending` | Accepted; archival is in progress | Show pause pending; observe this operation |
| `failing` | Latest save failed; Boat retries automatically | Show saving failure/pause pending; preserve the machine and record the operation for reconciliation |
| `completed` | Stop completed; sandbox archived | Confirm pause after verifying the current workspace intent/generation |
| `superseded` | Resume ended this stop | Reconcile the latest user intent; do not confirm the old pause |

Calling Stop again while pending returns the same operation ID. A Resume can end
an in-progress Stop. A forced Stop keeps the last successful snapshot and loses
subsequent writes. See [Boat's stop guide](https://docs.boat.dev/snapshots/stop.md)
and [operation-by-ID API](https://docs.boat.dev/api/reference/sandboxes/get-stop.md).

## Implemented in the verification harness

- Retain the acknowledged Stop ID and observe that same operation.
- Require a valid operation ID; reject an incomplete acknowledgement explicitly.
- Read `GET /sandboxes/{id}/stops/{stopId}` when the sandbox's latest-operation
  field is absent. Detect an unexpected operation change.
- Report `BoatStopFailing` or `BoatStopSuperseded` immediately. The benchmark exits
  that lifecycle scenario instead of auto-resuming or forcing a Stop. Its cleanup
  only deletes its uniquely created disposable test resource.
- Keep a still-pending operation unconfirmed, even if the state field says archived.
  Require both an accepted sleeping state and completed tracked operation.
- Record bounded state/operation/snapshot-status transitions, error-presence flags
  and recognized network error codes. Do not persist free-text provider errors,
  response bodies, authorization headers or credentials.
- Preserve the diagnostic on timeout. No force or blind mutation retries were added.

`node --test scripts/cloud-provider-benchmark.test.mjs` passes nine tests, including
failing/superseded operations, pending-vs-archived disagreement, completed stop,
operation-by-ID lookup, cleanup and secret redaction.

## Production integration to implement with the Rust Boat adapter

The Rust allocation/retained-lifecycle path currently supports Boxd; this patch does
not introduce or enable a full Boat adapter. That adapter must implement a durable
operation workflow rather than copy the measurement loop into the request handler:

1. Persist the desired pause and its workspace generation. Serialize conflicting
   pause/resume operations for that workspace; use the latest intent as authority.
2. Quiesce agent work, flush durable runtime state and acknowledge that checkpoint
   before asking Boat to Stop. A provider snapshot is separate from the durable
   conversation history and receipt commits.
3. Persist the Stop ID after acknowledgement. If the response is lost, inspect and
   reconcile before retrying a mutation. Do not assume a network error means the
   request was rejected.
4. Complete the UI request promptly with a pause-pending status. Reconcile in durable
   background jobs, using bounded exponential backoff/jitter. Do not keep a spinner
   or an HTTP request open for 90 seconds. An observation timeout is an unresolved
   operation, not proof that the machine stopped.
5. Treat `failing` as retryable snapshot failure while Boat's operation remains
   active; let its automatic retry work and expose actionable status. Do not
   auto-resume, force, delete, or recreate a customer's machine as recovery.
6. Confirm pause only when the same operation completes and the stored intent and
   generation still match. Fence late completion against a newer Resume.
7. On repeated snapshot failures, investigate the provider error, pre-stop work,
   Docker shutdown and snapshot-agent connectivity. Boat documents that blocking
   the snapshot agent's outbound traffic can prevent saving; this is a diagnostic
   avenue, not a finding about the failed benchmark machine. Preserve operation
   identifiers for provider support without sending any message automatically.

This avoids false pause confirmation and data loss; it cannot guarantee a snapshot
provider will always complete within a fixed latency.

## Live verification limit

The fresh live run failed on authenticated-list preflight before a machine was
created. A read-only retry also failed, and a separate request identified
`UND_ERR_CONNECT_TIMEOUT`. No new machine exists to clean up. This only establishes
Boat API connectivity failure from this workspace at the time of the recheck; it
does not establish a global Boat outage or the cause of the earlier Stop timeout.

Receipts: `.context/boat-stop-regression-red.log`,
`.context/boat-stop-investigation/operation-live-1.json`, and
`.context/boat-stop-investigation/auth-recheck.json`. Official stop/operation docs
were fetched separately into that investigation directory. Repeat the isolated
live lifecycle run when connectivity returns to validate actual operation fields.
