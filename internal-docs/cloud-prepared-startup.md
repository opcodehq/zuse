# Prepared startup and retained resume

The target is Send → authenticated runtime connected and selected agent accepting
its first prompt, p95 below five seconds. Git completion and first model output are
separate milestones. The staging measurements below **fail** this target.

## Implemented path

- One allocation queue reader dispatches up to four committed workspace jobs at
  once, independently of lifecycle reconciliation. Each job retains its existing
  lease, operation, machine and generation fences. Idle scans are not multiplied
  by the number of allocation slots. Shutdown joins in-flight provider effects.
- Committed creation sends a private allocation-worker hint. Hints never create
  machines or authorize effects; missed hints fall back to durable queue scans.
- Fresh first attempts skip a redundant provider name lookup. Recovery continues
  to reconcile the original name and provider receipt, without replacement creates.
- Selected-account refresh can overlap allocation. Secrets remain in the account
  authority and are delivered only after enrollment, with fresh delivery fences.
- Capable guests acknowledge bootstrap through an authenticated gateway upgrade.
  Storage commitment also records preparation ingress, removing two startup HTTP
  round trips. Older guests retain the existing bootstrap/ACK/ingress sequence.
- Bound-image byte verification and launch share one guest command. Provider
  artifact/source checks precede private boot delivery. Installed runtime/manifest
  hashes must match before the launch helper executes. No software is downloaded.
- A fresh canonical task can start its selected native harness and first turn
  alongside checkout. Agent readiness does not mark checkout ready or delete the
  initial command receipt. Preparation completion cannot resubmit that command.
- Provider delivery adds host instructions without changing the displayed user
  message or durable retry request. The agent runs the local `workspace-wait` command before
  repository operations, then read project instructions. This is an instruction,
  not a hard tool/filesystem barrier. Completed preparation omits the instruction
  on later turns and retained resumes.
- Staged checkout installation preserves the destination directory inode and
  journals entry moves. Native processes do not retain a replaced working folder.
- Required setup finishes before the local readiness marker changes to ready.
  Failure, cancellation, generation mismatch and timeout make the wait command fail.
  Stop during early reasoning cancels preparation; checkout writers are drained.

No warm pool, speculative machine, speculative wake or database notification is
introduced. Computers are created only for accepted tasks or explicit test runs.

## Staging rollout

Default early execution is disabled. Set these on the staging API and worker:

```sh
ZUSE_DEPLOYMENT_ENVIRONMENT=staging
ZUSE_CLOUD_FAST_START_ACCOUNTS=<selected-test-account>
ZUSE_CLOUD_ALLOCATION_CONCURRENCY=4
```

The account flag is stored only on newly admitted managed, broker-authenticated
computers. The API refuses the flag outside staging. Both services must share the
existing `ZUSE_CLOUD_EVENT_SIGNAL_DIRECTORY`; allocation hints use its private
`allocation/wake.sock`, separately from lifecycle/account sockets.

Publish a new managed guest image through the existing verified release workflow
before enabling early execution. Do not alter a running customer's image or bypass
its source, manifest or harness pins. There is no database migration for this path.
Rollback first removes the account flag for new admissions. Drain preparing pilot
computers before reverting binaries; retained computers preserve already-admitted
configuration and identities.

Early execution requires workload secrets to be declared in project settings or
the admitted environment recipe before launch. A repository recipe that introduces
additional undeclared bindings fails environment preparation rather than silently
running a process without them. Keep such projects outside the pilot until their
bindings are declared. Older delivery modes and interrupted startup use the
existing gated execution path.

## Measurements and evidence

On November 2, three isolated copies of the deployed staging Boxd artifact reached
an installed-runtime command in **2575, 2454 and 897 ms** using the Rust provider
adapter. All three were deleted and provider absence was confirmed. These are
individual provider measurements, excluding account enrollment, Git, model access
and first-prompt acceptance. They do not certify application startup percentiles.
Receipts are private, ignored `.context/fast-start-live-provider-baseline*.jsonl`.

A persistent-connection probe from Hetzner to the configured Supabase database
measured SELECT 1 at 40.21 ms p50 and 40.52 ms p95 over 25 reads. SQL round trips
remain material to the application budget; switching implementation language does
not remove them.

Guest logs distinguish identity creation, enrollment, engine assembly, bootstrap
HTTP, storage HTTP, selected-agent grants, gateway connection and first model
output. Allocation logs correlate provider calls with operation, workspace and
attempt. Codex logs its actual native `turn/start` response separately from
`SessionStarted`, which occurs earlier and is not proof of prompt acceptance.
`allocationToBootstrapMs` clarifies the old `enrollmentDurationMs` field; neither
measures just the enrollment HTTP request. Do not use readiness budget constants
as measured results or overwrite initial receipts with resume measurements.

Run a direct managed-image baseline with:

```sh
cargo run --locked -p zuse-cloud --example prepared_start_probe -- \
  --live --snapshot <approved-managed-snapshot> 1
```

The probe writes original create identities before dispatch, reconciles uncertain
creates by that name, deletes only its own computers and records cleanup. It never
enrolls a customer account. An uncertain create or cleanup is an error requiring
inspection of its original receipt, not another benchmark attempt.

## Acceptance still required

The modified API, worker and prepared guest were deployed together to staging on
November 2. They are **not accepted**: the measured latency exceeds both targets,
and fault testing found outstanding correctness issues. See
[the verification report](cloud-prepared-startup-verification.md). Keep these
release gates open:

1. Thirty prepared task starts and thirty memory-preserving resumes per certified
   provider/agent pair; report sample size, failures, request counts, p50 and p95.
2. Ten concurrent task starts, proving a slow allocation does not block lifecycle
   operations and no computer exceeds its original create/operation identity.
3. Deliberately slow/failed Git, required setup failure, expired credentials, Stop
   during checkout and reconnect without duplicate prompt execution.
4. Retained resume reuses the runtime/agent where supported, with unchanged branch,
   uncommitted files and software, no fetch/rebase/install, and working transports.

Prepared-start acceptance requires measured p95 below five seconds. Memory resume
targets p95 below two seconds; disk restores are reported separately. Partial
provider measurements and passing local tests do not close these gates.

## November 3 focused diagnosis

The latest single staging trace reached Git process creation at **10.270 s** and
Codex native first-turn acceptance at **13.243 s**. These remain failures against
the five-second target. They are individual observations, not percentile results.
The machine create call remains roughly half a second; one earlier focused trace
spent 4.452 s in the allocation worker with only 82 ms of active execution.

A staging account flag now overlaps selected-user repository verification with
GitHub scoped-token creation, after checking the existing owner/installation link.
Both operations and the existing fresh credential, scope and revocation checks
must succeed before delivery. The latest Git credential request took 1.687 s,
compared with 3.212 s in the preceding focused trace. Changes in individual traces
must not be interpreted as a certified latency improvement or a p95 result.

Transcript history now writes each bounded page of encrypted messages in one SQL
statement, including removal of oversized cached entries. Canonical tool references
are filtered and inserted in a batch in the same transaction. This limits database
round trips while holding the workspace lock; the fresh-start trace did not show
an agent acceptance improvement from this change alone.

Targeted PostgreSQL verification covered paginated history, replay, oversized-entry
replacement, rollback, reference scoping, paused object upload, credential overlap,
and concurrent unlink. Each live task executed once, and its deletion was checked
against both application state and the provider. Larger acceptance cohorts remain
open; further diagnosis should focus on allocation and bootstrap/storage/readiness
handoffs before repeating those cohorts.
