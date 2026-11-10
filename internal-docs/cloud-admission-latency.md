# Cloud task admission round trips

The batched admission path described below is now deployed on staging. The serial
query audit records the earlier implementation and explains the change; it is not
the current request count. The first successful admission before batching took
**2936.35 ms**. Recent individual live task admissions ranged from roughly305 ms
to543 ms, including the declared-environment case at543.34 ms. These are individual
samples across different flows, not a controlled latency distribution or proof of
the under-300 ms p95 target. Provider startup and model response are separate.

Thirty persistent-session `SELECT 1` requests measured
by the parent agent from Hetzner to Supabase had median **41.68 ms**, p95 **41.93 ms**;
TLS startup was excluded. No database-region change is proposed.

## Original serial fresh-task path

The inspected private canary request uses the public Hello-World repository,
`master`, an explicit branch and selected connection, no environment revision,
attachments or explicit secret bindings. No prompt, key, token or request digest
is copied into this document.

For a fresh selected-connection, managed-candidate task with the standalone GitHub
write cohort enabled and current additive tables present, `create_workspace`
awaits **35 SQL query executions plus BEGIN and COMMIT: 37 client commands**.
Function/trigger SQL runs on the server and does not add client network requests.

| Sequential group | Queries | Source |
|---|---:|---|
| Account advisory lock, tombstone-schema probe, tombstone lookup, prior idempotency lookup, snapshot shared advisory lock, snapshot-schema probe | 6 | runtime/creation.rs; images/deletion.rs::snapshot_read_guard |
| Candidate-computer count, entitlement, selected connection owner, locked project | 4 | runtime/creation.rs |
| Legacy project-build lookup, whose result the selected-connection path then discards | 1 | runtime/creation.rs |
| Candidate-column probe and exact locked managed-base record | 2 | managed_bases/candidate.rs; admission_lock.rs |
| Initiating user's connected native GitHub identity | 1 | runtime/creation.rs |
| Latest environment version | 1 | runtime/creation.rs |
| Launch-staging schema, staged upload identity, branch conflict | 3 | runtime/creation.rs |
| Workspace insert, launch-intent insert, history-schema probe, encrypted admission insert, control-owner insert | 5 | runtime/creation.rs; task_history.rs |
| Re-read control owner and workspace, prior allocation, repeated snapshot advisory/schema, snapshot deletion check, launch-intent existence | 7 | allocation.rs::queue_allocation_tx; locked_workspace; snapshot_read_guard |
| Allocation insert, workspace update, job enqueue, job max-attempts update | 4 | runtime/allocation.rs; jobs.rs |
| Response workspace read | 1 | runtime/creation.rs |
| **Queries** | **35** | |
| BEGIN + COMMIT | **2** | SQLx transaction manager |

These are awaited serially inside one transaction. The first message is stored in
`api_cloud_workspace_launch_intents` and encrypted `zuse_rust_task_admissions`.
**Creation does not insert an initial command mailbox row.** Guest take-launch /
mailbox operations happen later and must not be double-counted in admission.

Variants add explicit work:

- Normal paid admission, without staging allowance: two billing-period/ledger
  queries; keep both checks or move equivalent checks into the batched read.
- Workload bindings: vault schema + one locked set read (2 queries), then bounded
  in-memory decrypt/validation. The current empty-binding request skips these.
- Attachments: two queries per attachment in `verify_launch_assets`; the second
  read repeats `state='ready'` already required by the first. Fetch the validated
  locked set once instead; preserve metadata/digest/owner checks.
- Explicit environment revision0 skips the latest-environment query; a specific
  nonzero revision performs its exact version read.
- Org admission additionally performs authoritative membership/enrollment checks
  before the creation transaction. Do not conceal their time in provider timing.
- Idempotent replay exits earlier and does not represent fresh-task performance.

## Why the measured time is plausible

Warm prepared statements still require roughly `37 × 41.68 = 1542.16 ms` of serial
network time before crypto, JSON and database work. Index tuning alone cannot
meet 300 ms at this command count.

The pinned local SQLx PostgreSQL driver also explains the cold path. In
`sqlx-postgres/src/connection/executor.rs`, `prepare` sends Parse/Describe/Sync,
flushes and waits for ReadyForQuery; `run` subsequently sends Bind/Execute/Sync.
`sqlx-core/src/query.rs::query` supplies an argument buffer even for zero-bound
`sqlx::query` calls. A statement not in that connection's cache therefore adds a
preparation round trip. The 35 executions contain about33 distinct SQL strings;
the repeated snapshot lock/probe reuse their cache entries within the transaction.
The simple network model `(37 + 33) × 41.68 = 2917.60 ms` is close to the observed
2936.35 ms. This is a **source-backed explanation to validate with safe counters**,
not a packet trace or a claim every prepare/cache state has been measured.

## Batched transaction design

Target **four network commands on an existing connection**: BEGIN, locked read
batch, atomic write batch/RETURNING, COMMIT. At the observed median RTT that is
about167 ms of network time. Cold preparation, membership APIs, TLS/pool acquisition
and server CPU remain separately measured; no sub-300 ms guarantee is implied.

1. Keep request/body validation, verified actor/scope checks and digest generation
   in Rust. External membership/GitHub checks remain outside held SQL locks.
2. Batch the account/snapshot and candidate-computer locks in a deterministic
   dependency order, then return one bounded admission snapshot: tombstones,
   idempotent prior record, project, selected connection metadata, allowance /
   entitlement and billing totals, exact environment, GitHub identity, staged
   uploads, branch, and locked base records. Preserve the0023 helper's narrow
   privilege boundary. For certified selection, return the **fresh records read
   under its row locks**, not stale manifests selected before a lock wait.
3. Rust validates the returned model/harness, recipe, secret references and
   immutable actor/connection owner; computes transcript wrapping, encrypted first
   message/admission, IDs and boot envelope. This is bounded local work while the
   admission transaction retains its locks; it must not call providers or refresh
   credentials while holding those locks.
4. A single data-modifying CTE writes the workspace directly with its final
   allocation config, launch intent, encrypted admission, native owner, allocation
   identity and recovery job, and returns the actual inserted workspace. Set the
   job's max-attempts at insertion. Keep all writes atomic with event-outbox
   triggers. A failure rolls back the whole message/task/job set.
5. Preserve the generic `queue_allocation_tx` revalidation for existing/public
   callers. Add a private fresh-admission path accepting a validated new-task
   plan, rather than paying to re-read and revalidate rows just inserted by the
   same transaction. It must not accept arbitrary retained workspace IDs.

This does not move allocation into the HTTP transaction, loosen base revocation,
change resume behavior, skip billing or borrow an unselected connection. Stable
idempotency and the final unique branch/index constraints remain authoritative.
Only a genuinely new task gets a new allocation identity. A lost HTTP reply must
return the same durable row and original operation on retry.

## Incremental implementation and verification

- Cache required schema capabilities at service readiness/startup for the managed
  standalone lane; do not issue repeated `to_regclass` on every new task. Preserve
  the old compatibility lane separately until its clients migrate.
- Skip the discarded legacy-build lookup when the selected-connection path is
  necessarily managed. Do **not** make old customer images eligible as a shortcut;
  they lack a compatible selected-connection/harness cache attestation.
- Batch read/lock work, then batch write/RETURNING. Do not interpolate parameters
  into raw SQL or trade away row-lock semantics for fewer calls.
- Prepare the small fixed batch statements on real API pool connections during
  their normal setup, or measure cold first-use explicitly. This is ordinary DB
  connection setup, not speculative sandbox allocation or artificial VM keepalive.
- Add counters for static stage labels, DB-execution count, prepare/cache state,
  pool wait, transaction time and total admission. Never log SQL parameters,
  query text containing credentials, prompt contents, ciphertext or DSNs.
- Re-run actual provisioner-restricted-role creation, exact candidate allowance,
  base revoke/retire races, simultaneous same-key requests, changed-body/actor
  rejection, branch collisions, secret/attachment revocation, and simulated
  response loss. Assert exactly one task, first-message admission, allocation and
  job. History must remain visible while provider allocation is disabled.
- Report fresh versus idempotent; cold versus warm statement cache; personal vs
  org; attachments/secrets separately. Measure returned HTTP time separately from
  provider allocation, runtime enrollment, agent acceptance and first model token.

The managed, explicitly selected-connection creation path uses
`0025_admission_read_batch.sql` when installed and executable by the actual API
role. Readiness caches its availability; legacy schemas retain their existing
path. Migration0025 and this path are deployed on staging; production has not been
cut over. Later server-admitted command delivery adds its own atomic admission
records, so the original four-command design is not a measured count for every
current request variant.

The transaction makes four SQL executions: explicit READ COMMITTED `BEGIN`,
`zuse_rust_admission_read_batch`, the dependent write CTE, and `COMMIT`. The read
function is VOLATILE and SECURITY INVOKER with a fixed schema search path. It
acquires the original account/snapshot/project/base locks, then reads facts with
fresh internal statement snapshots after advisory-lock waits. Rust still validates
idempotency, actor/scope, entitlement, billing, connection owner, exact managed
release/candidate, environment, secrets, assets and branch admission. Necessary
encrypted secret/asset records are returned only for the selected project and
requested bindings, locked against revocation during admission. No OAuth or agent
credential secret is returned by this function.

The write CTE atomically creates the workspace with its final generation/storage
identity, sealed launch, encrypted initial history, Rust control owner, allocation
and job. Existing/retained allocation paths retain their generic reconciler. The
first-message helper seam remains before commit for future server-admitted-v2
integration. Existing None/mailbox-v1 delivery semantics remain unchanged.

Actual isolated Postgres evidence:

- SQL atomicity and concurrent-lock test: final-job uniqueness failure and explicit
  rollback leave no partial rows; a caller waiting on account admission observes
  the competing committed task and returns its identity.
- Actual provisioner-restricted-role personal/candidate admission + worker: pass.
  Registry UPDATE remains denied and narrow helper execution remains granted.
- Full certified organization and actor-bound uncertified candidate scenarios:
  pass, including actor/selected-personal-connection/lifecycle/departure fences.
- Schema isolation and migration rehearsal remain green with the new function.

See `.context/admission-batch-{tests,build,api-check}.log` and
`infra/cloud/tests/admission-batch.py`. These are correctness checks, **not** a
production admission latency measurement. SQLx cold prepares, pool acquisition,
request authentication and scope refresh can add time outside the four SQL
executions. Measure fresh/warm and personal/organization paths separately after
staging deployment before claiming the 300 ms target.
