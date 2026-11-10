# Live cloud provider measurements

The latest [Boxd/Boat verification](cloud-provider-verification.md) adds current
live lifecycle samples, production Rust Boxd inspect/exec checks before/after wake,
and a clear list of application deployment blockers. Infrastructure passing does
not establish working end-to-end cloud chat.

Measured October 29, 2026 from the Conductor Amazon Linux cloud workspace. This
is live infrastructure research, not a production application latency claim.

## Decision

Do not introduce warm pools, speculative workspace allocation, or keepalive traffic
to extend provider fast paths. Use versioned prepared images, repository object
caches, independent startup tasks, and provider-native resume. Keep the client UI,
history, and durable command admission independent of compute readiness.

A provider's advertised wake time is not the application's execution readiness.
Measure allocation response, actual command execution, runtime enrollment,
credentials, Git readiness, and first agent turn separately.

## Method and safety

`scripts/cloud-provider-benchmark.mjs` authenticates read-only by default. `--live`
creates one uniquely named isolated Boxd VM or one Boat no-env sandbox, measures
its lifecycle, and deletes only the resource created by that invocation. No existing
workspaces or account settings are modified. API response bodies, credentials,
command output, and error messages are not logged. Reports contain IDs, timings,
state names, and continuity booleans. A disk marker and a detached Python server
holding a random response in RAM distinguish durable disk and actual process
continuity; a boot ID provides an independent restart signal.

Each run has a 600-second provider lifetime limit and a 90-second state-readiness
deadline. Mutation requests are not blindly retried. A lost create response is
reconciled by the unique exact resource name. If identity cannot be recovered,
the report marks an unresolved create outcome; do not rerun blindly. Boat TTL
stops a sandbox rather than deleting it, so unresolved identities still need
operator cleanup. Run reports must say `cleanup: confirmed-absent` before declaring
successful cleanup.

The Boxd SDK's create method also rereads machine details, so its `create-response`
measurement includes that read. Authentication includes the initial token exchange.
Creation-to-exec numbers below exclude authentication and include state checks.
State-confirmation loops use a 200 ms interval for measurement only; this is not
the proposed production polling strategy.

## Results

These are individual samples, not p50/p95 measurements. Regions, repository sizes,
snapshot contents, load, and cached authentication were not controlled across
providers. No real repository checkout, Zuse enrollment, model call, or provider
credential grant was exercised.

| Scenario | Create to command-ready | Resume to command-ready | Process memory | Disk marker |
| --- | ---: | ---: | --- | --- |
| Boxd isolated VM, hibernate/wake | 830 ms | 443 ms | Preserved | Preserved |
| Boxd repeat with home-directory marker | 986 ms | 606 ms | Preserved | Preserved |
| Boat small sandbox, immediate stop/resume | 924 ms | 536 ms | Preserved via recent-stop fast path | Preserved |
| Boat small sandbox, 30-second stopped dwell | 933 ms | 733 ms | Preserved via recent-stop fast path | Preserved |
| Boat small sandbox, resume as default size (cold restore) | 880 ms | 1,437 ms | Lost; boot ID changed | Home-directory marker preserved |

The first Boxd sample's hibernate request returned in 107 ms, but confirmed
`hibernated` state took another 14.65 seconds. Boat stop responses returned in
roughly 340 ms, with confirmed `archived` state several seconds later. Do not
report an accepted pause request as completed suspension.

Receipts in the workspace:

- `.context/cloud-provider-live-measurements-v2.json`: Boxd and immediate Boat cycle.
- `.context/cloud-provider-boat-dwell-measurements.json`: Boat 30-second dwell.
- `.context/cloud-provider-boat-durable-cold-restore.json`: forced Boat cold restore.
- `.context/cloud-provider-boxd-final.json`: final harness using the user's home.

All resources allocated for these checks, including failed harness development
runs, were deleted and their absence confirmed. Initial harness development
exposed two issues: shell-backgrounded processes did not reliably detach from
provider exec; a `/var/tmp` file did not survive a Boat cold restore. The final
harness double-forks a session-detached process and stores markers in the
provider command user's home. Failed runs are not included as latency samples.

## Provider behavior and architectural implications

### Boxd

Boxd distinguishes standby (memory retained in RAM), hibernation (memory written
to disk), and stop (memory lost). The live hibernate/wake probe verified actual
process continuation, not merely a successful wake RPC. [Official lifecycle
guide](https://docs.boxd.sh/guides/suspend-resume.md).

An isolated machine omits account integrations and agent logins; this is the
appropriate baseline for workspace-scoped credential grants. Keep credentials
out of reusable image snapshots. [Official isolation
guide](https://docs.boxd.sh/guides/egress.md).

Inference: subsecond compute reconnection is plausible on the tested hibernation
path, but the Zuse runtime still needs measured wake notification, gateway
reconnection, enrollment validation, and expired-credential handling. The numbers
do not justify eliminating ownership or storage fences.

### Boat

Boat's detailed current stop documentation contains an important optimization:
it retains the stopped machine for ten minutes without billing. Plain resume
during that interval returns the same machine with no restore, including when the
API already says `archived`. After that period, resume uses the disk snapshot and
running processes are lost. A resume with changed size/environment starts a new
machine. The immediate and 30-second measurements match that documented fast
path. [Official stop guide](https://docs.boat.dev/snapshots/stop.md).

Do not classify all Boat resumes as cold boots, and do not promise permanent
memory preservation. Treat process continuity as observed per resume; attach to
the old runtime if alive, otherwise start the runtime against existing storage.
Never keep machines awake or repeatedly resume them to prolong the free window.

Cold-restore readiness is staged: commands can work before systemd services,
Docker, and complete file hydration. Runtime readiness must test its own process
and data, not merely trust provider `ready`. Avoid waiting for complete hydration
when the needed files are already readable. [Official readiness
guide](https://docs.boat.dev/readiness.md).

Boat documents durable home directories, including secondary users, and selected
system changes; temporary paths and running memory are not snapshot guarantees.
Use `/home/zuse` for the real runtime database and recovery identity. Our initial
`/var/tmp` probe did not survive a forced cold restore; the home-directory probe
did. [Official snapshot contents](https://docs.boat.dev/snapshots/whats-saved.md).

Boat sends lifecycle webhooks, including ready, archived and hydrated. Use signed
webhooks to wake reconciliation, with bounded repair reads, rather than client
status polling. [Official webhooks](https://docs.boat.dev/webhooks.md).

## Reproduction

Node 20+ is required. Boat needs only `BOAT_API_KEY`. Boxd needs `BOXD_API_KEY` and
the official SDK. Install the pinned research baseline in an ignored directory:

```sh
npm install --prefix .context/cloud-provider-sdk --no-save --ignore-scripts @boxd-sh/sdk@0.2.14
node scripts/cloud-provider-benchmark.mjs --boxd-sdk .context/cloud-provider-sdk/node_modules/@boxd-sh/sdk/dist/web.js
node scripts/cloud-provider-benchmark.mjs --provider boxd --live --boxd-sdk .context/cloud-provider-sdk/node_modules/@boxd-sh/sdk/dist/web.js
node scripts/cloud-provider-benchmark.mjs --provider boat --live --sleep-ms 30000
node scripts/cloud-provider-benchmark.mjs --provider boat --live --boat-resume-type default
CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo build --locked -p zuse-cloud --example provider_auth_probe
node scripts/cloud-provider-benchmark.mjs --provider boxd --live --boxd-sdk .context/cloud-provider-sdk/node_modules/@boxd-sh/sdk/dist/web.js --boxd-rust-probe target/debug/examples/provider_auth_probe
node --test scripts/cloud-provider-benchmark.test.mjs
```

The last live command intentionally changes the isolated test sandbox's size to
exercise cold restore. It is not a production resume policy.

## Remaining measurements

The production **Rust** Boxd token exchange and gRPC-web adapter were also verified
read-only with `cargo run --locked -p zuse-cloud --example provider_auth_probe`.
An authenticated lookup of a unique nonexistent identity returned the expected
absence in 878 ms including exchange, with zero mutations. Receipt:
`.context/cloud-speed-rust-provider-probe.log`. This checks our Rust wire adapter;
the lifecycle measurements above use the pinned provider SDK and Boat HTTP API.

Real application timings remain open: prepared account image allocation, guest
runtime enrollment and ready signal, sealed Claude/Codex credential delivery,
latest-ref checkout, gateway reconnection after memory wake, large conversation
cache opening, and streamed first reply. Validate those with the deployed API,
database, object storage, account authority, and prepared image; API keys alone
cannot prove them. Add multi-sample regional/load runs before publishing percentile
claims. Native Mac and full billing behavior are also outside these checks.

## Worker wakeup alternatives under traffic

Research update after the user's request to reconsider PostgreSQL notifications.
This section is a recommendation, not an implemented infrastructure change.

The current API and workers are separate processes/containers. `jobs::enqueue`
already commits lifecycle jobs with the state change; `jobs::claim` uses leased,
generation-fenced PostgreSQL claims with `SKIP LOCKED`. The workers currently scan
repeatedly. Preserve that durable authority while changing how a worker learns
that committed work exists. A wakeup is a hint to claim work, never authorization
to execute a provider operation without the database lease.

| Mechanism | Guarantees and failure coupling | Fit here |
| --- | --- | --- |
| PostgreSQL `LISTEN/NOTIFY` | Delivered after commit to current listeners, with no disconnected-consumer replay. An overflowing notification queue makes transactions that call `NOTIFY` fail at commit; a listening session left in a long transaction can prevent queue cleanup. | No extra service, but puts signaling capacity inside the database failure boundary. Hold out of the default implementation following user feedback. |
| NATS Core wakeup hints | Ephemeral, at-most-once delivery to connected subscribers. Missed hints need durable catch-up. | Clean separation of signaling from database writes; useful for many workers and realtime fanout when NATS is an accepted deployment dependency. |
| NATS JetStream delivery | Durable streams, consumer acknowledgments and redelivery. Database state and a broker publish are still separate commits. | Appropriate if broker-managed delivery/backlog becomes a requirement; keep PostgreSQL operation IDs, ownership fences, and idempotency. Requires outbox publishing and broker operations. |
| Redis Streams | Consumer groups track delivered/unacknowledged entries; acknowledgments and pending-entry reclaim support recovery. Durability depends on configured persistence and replication, not merely using Streams. | Sensible when Redis is already operated reliably; do not introduce it solely to replace a wakeup hint. |
| Authenticated internal dispatch | After committing a durable job, send a bounded internal request that wakes the worker; the request itself has no durable queue. | Smallest next step for today's API/worker split, with durable fallback reads. Requires internal routing and authentication, not another public execution API. |

PostgreSQL documents delayed delivery during listener transactions, queue-full
commit failures, and `pg_notification_queue_usage` monitoring. The queue's standard
size is substantial; this is a failure mode to account for, not evidence that our
actual traffic has exhausted it. A dedicated autocommit listener avoids one major
hazard but does not make notifications durable. [NOTIFY
semantics](https://www.postgresql.org/docs/current/sql-notify.html), [LISTEN
registration and recovery](https://www.postgresql.org/docs/current/sql-listen.html).

Core NATS has no stored backlog or acknowledgment. JetStream adds persistence and
redelivery, so consumers must expect duplicates. Delivery limits and poison-job
handling need explicit monitoring rather than assuming a dead-letter queue exists.
[Core NATS guarantees](https://docs.nats.io/learn/core-nats/), [JetStream
acknowledgment and redelivery](https://docs.nats.io/learn/jetstream/acknowledgment).

Redis `XREADGROUP` records pending entries until `XACK`; `XAUTOCLAIM` can transfer
idle pending work. Configure persistence intentionally: for example, AOF with
every-second fsync can lose recent writes on a crash. A Redis stream does not
atomically commit a PostgreSQL workspace update. [Consumer
groups](https://redis.io/docs/latest/commands/xreadgroup/), [pending
reclaim](https://redis.io/docs/latest/commands/xautoclaim/), [persistence
tradeoffs](https://redis.io/docs/latest/operate/oss_and_stack/management/persistence/).

### Recommended sequence

For the present deployment, use committed PostgreSQL jobs plus authenticated
internal dispatch, coalesced by job kind, with startup catch-up and a bounded
repair scan. Dispatch runs only after commit and outside the database transaction;
failure must not change an already-accepted workspace operation into an apparent
failure. The receiving worker records a local wake signal, then drains due jobs
through the existing claims. It does not trust request-supplied provider actions.
Duplicate hints are harmless; a crash between commit and dispatch is recovered by
the repair scan. Delayed retries need a next-due timer, not just a creation hint.

If larger horizontal fanout warrants a broker, prefer NATS Core for disposable
coalesced wakeups while the PostgreSQL jobs remain the durable queue. Choose
JetStream instead when offline broker delivery and broker backlog/consumer control
are explicit requirements. In that design, write an outbox in the state-change
transaction and publish it with retries; acknowledge broker delivery only after
the database records the relevant durable result. Never replace the current
operation/idempotency fences with a claim of exactly-once network execution.
[Transactional outbox and duplicate
handling](https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/transactional-outbox.html).

Benchmark the alternatives before adopting a broker: p95/p99 admission-to-claim,
idle SQL queries, backlog recovery, API commit latency during signaling outage,
hint coalescing under bursts, duplicate delivery, and worker restarts. Keep token
streaming and transcript payloads out of the job-wakeup mechanism. None of these
choices requires a warm pool or changes the no-warm-pool constraint.
