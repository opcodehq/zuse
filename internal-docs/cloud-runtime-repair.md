# Supervised runtime repair

Runtime repair replaces the Zuse process on the **same running computer**, against
its retained native disk. It never allocates a computer, sleeps the provider,
fetches Git, renames a branch or restores a different environment.

## Availability

A task must carry attested `rustRuntimeSupervisorVersion: 1` and
`rustRuntimeRepairVersion: 1`, with committed generation-bound native storage.
The optional manifest flag is published only after an idle resident supervisor
advertises the tested repair protocol. An existing task without that flag is not
silently upgraded. The current first staging candidate has **no repair flag**;
its Restart runtime control remains unavailable.

`GET /v1/cloud/workspaces/{id}` exposes `runtimeRepairAvailable` only to an editor
and exposes `runtimeRepairCanEdit` for mutation controls. It includes a compact
`runtimeRepair` summary for reconnecting clients. Summaries
carry numeric `updatedAt` so late replies cannot replace newer progress. Full
recovery metadata comes from the operation endpoint; receipts never contain a
prompt, tool output or credential.

## Operation

1. `POST .../{id}/runtime-repairs` with `{ "idempotencyKey": "unique-request" }`
   persists the operation, immutable source/generation/disk, attempted command
   inventory and linked Pause. Repeated admission returns the same operation.
2. The worker waits for graceful quiescence, then uses the resident supervisor to
   retire the exact original process tree. A memory-Parked receipt is rejected
   for this Pause because a full engine shutdown is required.
3. The supervisor reconciles native SQLite/WAL receipts and journals before
   starting a fresh child. It retires detached descendants, preserves dirty files
   and installed software, and never resends an original attempted command ID.
4. A supervisor `ready` receipt means the child was started, not that repair is
   complete. Completion requires the linked Resume ACK, online runtime and the
   same committed native storage binding.
5. Interrupted chats or unproved commands enter `attention-required`. New mailbox
   leases remain blocked until the user explicitly resolves the operation.

The API and worker verify current organization access for both the original task
author and the actor requesting repair before dispatching supervisor mutations.
Cleanup/deletion remains a separate lifecycle action. Provider effects occur
outside workspace SQL transactions and are fenced by the job lease and operation
identity when their receipts are committed.

## Uncertain outcomes

Admission or a `processed_commands` entry alone is not proof an action completed.
The helper preserves a real terminal ACK, and records `outcome-unknown` when an
admission has no ACK. The server marks attempted commands unknown before fresh
execution and accepts a terminal local receipt only with its original exact
fingerprint. A late old Applied ACK cannot overwrite a terminal unknown outcome.

`POST .../{id}/runtime-repairs/{operation}/resolve` accepts:

- `continue`: retain original unknown outcomes; permit fresh queued commands.
- `discard`: additionally cancel messages that never received a runtime lease.

Neither choice retries an original command. `POST .../{operation}/retry` retries
the same supervisor operation and phase after a delivery/readiness error; a lost
start reply never causes a speculative second child to be started. An uncertain
supervisor launch fails closed instead of respawning without proof.

## Verification and release status

The focused four-test suite passed against disposable local PostgreSQL and a real
resident supervisor/process tree. It covers an actual child SQLite commit before
ACK, parent SIGKILL with a continuing setsid orphan, orphan retirement before the
new child, lost-start reply reconciliation, retained dirty work, explicit
attention/discard, late ACK fencing and requester-departure denial. The fixture
uses a controlled child, not a coding model or cloud-provider machine. It also
checks that an online metadata snapshot without the linked durable Resume ACK
stays pending, and a later authorized Sleep/Resume pointer change does not strand
the repair. Readiness is checked again while committing under the workspace lock.

Source implementation and API metadata checks pass. Migration0021, a compatible
versioned managed artifact, staging deployment and live-provider repair acceptance
remain separate release gates. Do not advertise provider repair support from
these local tests alone.
