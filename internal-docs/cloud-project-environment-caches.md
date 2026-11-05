# Project environment caches

Project cache preparation is an optional acceleration path. Computer creation
retains the managed base when the optional schema, runtime certification, exact
resolved input or compatible prepared artifact is unavailable. Existing managed
bases have no inferred certification.

The source adds migration `0028_project_environment_caches.sql`; it is deliberately
not in the deployment bootstrap list pending operator review. No shared database
or provider resources were changed for this implementation.

Environment settings save a version and one project-scoped preparation request
in the same transaction. Account linking and recipe editing do not require a
compute entitlement. Allocation of a preparation builder requires entitlement.
Requests retain the initiating actor and original membership metadata; a backend
must reauthorize the actor before effects and after external operations.

Cache identity includes the account, project, repository, provider, architecture,
managed release/artifact/runtime, recipe source/digest, exact source commit and
relevant dependency blob hashes. Credentials, agent connections and models are
not cache identity inputs. Whole-commit identity is conservative: a custom setup
command may read any tracked file. E2B carries its exact operational artifact
binding rather than treating a mutable template name as an immutable build.

The durable worker records each effect identity before calling a provider. A lost
response enters `uncertain`; subsequent work reconciles that original effect.
Provider IO runs outside SQL transactions and separately from the lifecycle
priority loop. Resource receipts accept only bounded opaque IDs and readiness
booleans, not provider stdout, secrets or request bodies. Cleanup continues after
membership revocation using original resource receipts. Publishing requires the
original base and runtime certification, exact preparation receipt, and evidence
that builder and verifier resources are absent. A failure never replaces an
already-ready compatible artifact.

Operator cache certification uses a separate table with app writes revoked. A
narrow fixed-search-path lock function fences certification retirement against
admission without granting registry UPDATE. Cache preparation events use existing
account environment invalidations; lease heartbeats produce no progress events.

The admission path can select an explicit source SHA with a matching recipe.
The allocation resolver now resolves public named refs outside SQL locks, checks
public visibility again and pins the commit under the original job lease and
generation before selecting an artifact. Two HTTP tests and the isolated PostgreSQL admission test passed for this path,
including commit pinning across retries and a withdrawn job lease. A pinned task never resolves its branch on resume.
Private caches remain unavailable until fresh actor-scoped repository authority
is verified before allocation; private tasks use the managed base.

Actual tests passed for scoped settings enqueue, durable original operation
reconciliation, certificate retirement fencing, exact hit and planned fallback,
missing-cache-schema base admission and preservation of an older ready artifact.
Five real guest filesystem tests passed, including offline Git bundle preparation,
setup/refresh exactly once, failed-setup no replay, retained dirty work/dependencies,
symlink protection, managed-base privacy and rejection of pre-import edits.
Anonymous full-history source-bundle generation and durable lost-PUT handling
passed local Git and isolated PostgreSQL tests. Two raw Boxd transport fixtures
passed lost allocation/preparation/capture/delete reply reconciliation and corrupt
artifact cleanup, using only original resource identities.

The real Boxd preparation adapter and API-only actor authority are implemented in
source but have not produced a certified live cache. A bounded preparation
lifetime switches stalled work to cleanup using original resource receipts.
Ready-artifact retirement, observed resource accounting and source-object
retirement passed their isolated PostgreSQL fixtures, including retained task
references, late PUT tombstones and cleanup after entitlement withdrawal. Retirement
fences new hits and waits for all nondeleted task references before deleting an
artifact. Source cleanup retains tombstones and resweeps exact prefixes after
late or uncertain PUTs. Turning off new preparation leaves cleanup enabled for
owned resources and never requires a Boxd key solely for source-object cleanup.

Resource occupancy is authority-observed, with costs explicitly unpriced. These
rows do not invent provider costs or mutate account subscriptions, invoices or
financial identities. A production pricing/reservation policy, actual provider
cache certification, private repository authority before allocation and the other
provider preparation adapters remain required before broad activation. No provider/runtime combination is certified for project cache hits.
Migration0028 is unlisted and unapplied; project preparation remains disabled.

Public named-reference resolution uses GitHub's [commit endpoint](https://docs.github.com/en/rest/commits/commits#get-a-commit) and reads the recipe through [repository contents at the resolved commit](https://docs.github.com/en/rest/repos/contents#get-repository-content). Requests are anonymous, bounded and redirect-free; public visibility is checked before and after resolution.


Preparation capture requires `preparationCaptureVersion: 1` in the exact runtime
certification receipt. Older preparation receipts do not imply this capability.
The Boxd worker now has a separate durable `quiesce` phase before snapshotting.
Its root-owned capture journal binds the original operation, cache key, runtime
and prepared receipt digest. A lost stop reply is inspected through that journal;
an uncertain stop is never replayed. Status reads do not acquire a runtime lock
or create journal files.

The new guest capture command requires the enabled systemd boot unit and an
unbound resident with MainPID greater than one. It uses the supervisor's atomic
publication stop, verifies process exit and lock release, and removes only
verified ephemeral supervision files. Generic empty identity, runtime data,
credentials, history and authority checks remain strict. Prepared checkout and
dependencies remain under `/opt/zuse/project-caches`; their receipt digest must
remain unchanged across capture. PID1 and unknown boot ownership fail closed.
The restored verifier checks the same capture receipt before publication.
Existing published guests lack this capture contract and cannot be certified by
configuration alone; a new guest build and restored-resident proof are required.

A bounded live acceptance requires Root approval of migration0028, an exact
capture-capable Boxd base/runtime certification, public test repository and recipe,
and one scoped owner with entitlement. Keep preparation disabled until those
proofs are reviewed. Schedule one immutable public commit under one original
operation, observe one builder and verifier through original resource IDs, verify
the dependency marker on one cache hit and base fallback after certificate
retirement, then delete the test task and retire the cache. Independently verify
builder/verifier/task/artifact absence and source-object tombstone cleanup. No
private repository cache, provider price claim or other provider support follows
from this canary.

Current published owner-only pilot bases are uncertified and guests78/c448 do not
provide capture-v1. Testing cannot substitute a fabricated certificate or a
production support claim for the missing primitive. After a new guest proves
capture and restore, Root must separately review listing/applying schema0028 and
creating isolated staging certification records for the exact runtime/base. Only
then can the narrowly scoped owner hit/fallback canary run.

Focused capture verification passed: two real filesystem/journal tests preserve
prepared dependencies, reject later task binding and changed receipts, and never
repeat an uncertain stop; three raw Boxd HTTP transport tests cover lost replies,
restored receipt validation, and unsupported/missing/unreadable capture proof with
zero snapshot submissions and cleanup of the original builder. The legacy receipt
capability regression passed. The updated scoped preparation/certification/retirement
PostgreSQL fixture also passed with the explicit quiesce phase. Six real supervisor
IPC/process/lock cleanup tests passed independently. These tests establish the
source contracts; they do not certify a live Boxd cache.

The registry prerequisites are explicit: `cloud_managed_bases` must contain the
exact provider/release/artifact in state `verified`, and
`cloud_project_cache_capabilities` must certify that same runtime SHA and
architecture with capture-v1. A staging candidate pilot alone satisfies neither
condition. Both registry transitions require Root review after actual primitive
proof, separately from applying0028 and enabling preparation. Production registry
support remains unchanged.
