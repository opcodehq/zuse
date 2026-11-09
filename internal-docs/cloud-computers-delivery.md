# Cloud computers delivery

<!-- ROOT_CURRENT_STATE_BEGIN -->

## Picker and metadata availability correction — 2026-11-02

Staging API/worker now run verified ELF `172cd699922a7ade`. An expired compute
candidate previously made account context fail409, which the native engine
incorrectly described as a busy workspace. Read-only discovery now describes
compute unavailability while keeping strict allocation fencing. The sidebar
loads projects and chat metadata independently of compute context, retains rows
through transient failures, and offers status/retry. Unlinked local projects
receive an Add project to Cloud action instead of an unusable computer choice.

Native project/chat indexes now have small scoped protected metadata snapshots;
ordinary transcript cache remains encrypted SQLite. Fresh data supersedes saved
rows; real access denial/revocation clears them. Thirteen focused app/cache checks
and the actual PostgreSQL candidate regression passed. Live context/projects/list
reads return200 with2projects and58chat records; the two sleeping computers remain
hibernated. Actual Mac visual and secure-store persistence acceptance is pending.

Production and existing resources are preserved. The owner-only staging compute
test configuration was renewed until `2026-11-03T04:55:36.273000+00:00`; normal certified release
promotion remains necessary for ongoing compute availability. Metadata/history
reads now survive that test configuration expiring.



## Idle sleep correction — 2026-11-01

The ten-minute idle timeout previously had no worker scheduling implementation.
The staging API and worker now run verified ELF `521ca345d072ff8f` with bounded
idle scheduling through retained lifecycle operations. Meaningful terminal,
file/Git and command activity extends the durable deadline; subscriptions,
history reads and transport liveness do not. Activity coalesces for fifteen
seconds, with an equal scheduler grace. Running, queued, waiting or uncertain
commands and unsupported/setup/transitioning computers are excluded. Row and
ownership locks serialize new activity against sleep admission.

Nine focused checks passed, including real PostgreSQL concurrency, duplicate
admission protection, generation fences and existing RAM pause/wake behavior.
Production, credential authority, edge and retained identities are unchanged;
the staging owner's existing unlimited-computer setting is preserved.
Live Boxd verification passed using the real ten-minute interval: both Ready
computers reached provider `suspended` and database `paused` after 10m24.6s and
10m25.6s, with exactly one automatic pause operation per computer. No live wake
was initiated for this change.
The older failed-setup computer remains excluded and requires separate recovery.


## Current verified state — 2026-11-01 UTC

**The complete release remains unfinished.** Staging API and worker run `1f990553`; their actual binaries, image, health and protected state are independently verified.
Managed runtime `a49bc3980241` passed the core three-agent cases on all four providers.
A corrected project-preparation runtime, `1f2e4b942c8a`, is now published and active
for the staging owner on Boxd; its task/cache acceptance is still pending.
All four core-case computers are deleted with independent provider404 checks.
The previous Boxd pilot remains preserved as rollback with its original expiry. Production, credential authority,
edge services and the original retained computer remain protected.

| Area | Implemented / deployed | Live verification / remaining |
| --- | --- | --- |
| Basic chat and Computer UX | Same-view New Chat, separate plus-tab action, cloud-only projects, model-based sole connection selection, immediate Send activity, preparation stages and lifecycle controls | Shared compiled-WASM checks and component visuals passed. Git and credential preparation overlap; actual agent handshake prewarming is not implemented. Mac user-flow acceptance remains pending |
| Boxd | Runtime `a49bc3980241` core case passed. New preparation runtime `1f2e4b942c8a`, snapshot `snap_fdf7e0152c454788809dc383318773ee`, is published and owner-only active under pin `e0fa0887`; clean builder and restore proof deleted. Original pin `47b314e6` remains rollback with unchanged expiry | On runtime `a49bc3980241`, four real streamed turns across Claude, Grok and Codex passed, with tools, edits, shell checks, terminal/files, reconnect, paused history without wake and atomic follow-up wake. New `1f2e4b942c8a` live task/cache acceptance remains pending. Runtime and native Codex processes, branch, dirty work and installed software survived RAM sleep. Owned test computer deleted and independent404 confirmed |
| E2B | New private template `v5gogdmyrgpmnkk7bmss`, build `675f8aca-a00a-4aff-a94c-c50ff24a0701`, published with runtime `a49bc3980241` | Current four-turn case passed all three agents, real streams/tools, semantic continuation, terminal/files, reconnect, paused history without wake and atomic Codex continuation. Runtime/native Codex process identities survived RAM sleep. Test computer independently confirmed deleted. Final acceptance checkpoint is sealed |
| Boat | Managed runtime `a49bc3980241` published from a clean parent; independent checks confirm builder/restore deletion, artifact version and unrelated-resource preservation | Earlier failed evidence is retained. The new four-turn case passed real streams, semantic continuation, tools, terminal/files, paused history without wake and cold sleep/resume retaining branch, refs, dirty files and installed software. Runtime process restart was observed as expected; owned machine deletion404 is independently confirmed. No RAM-preservation claim |
| Daytona | New `a49bc3980241` base published; clean restore proof and independent deletion passed | Earlier cold Sleep failure remains historical. Owner-only import and activation passed. Corrected four-turn case passed real Claude/Grok/Codex streams, tools, semantic recall, observer reconnect, cold restart with retained branch/refs/dirty files/software, paused history without wake and terminal/files. Exact deletion404 and return to the original Boxd pilot expiry are confirmed. The earlier observer-input failure is retained; no RAM or Mac acceptance claim. Provider restrictions do not block independent work |
| Credentials and GitHub | Encrypted staging credential authority and standalone GitHub integration deployed; selected runtime delivery for three agents exercised; staging App webhook configuration repaired, with real signed GitHub-origin ignored-event delivery204 and durable receipt verified | Private fetch, push and draft PR passed in an earlier Boxd case; temporary PR/branch cleaned without changing main. Authority-changing webhook and organization/approval matrices, renewal/revocation and fault coverage remain incomplete |
| Durable chat and caches | Durable admission/replay, account events, history pagination and scoped native/browser caches implemented | Actual IndexedDB reload with held fresh response, retained older range, cached tool expansion, actor/API clearing and sign-out clearing passed on the preceding compiled WASM. Native23 GPUI checks passed, including provisional opening, fresh and older-page access denial, temporary transport errors and in-place event reset recovery. Shared27 checks passed on one current binary, including same-view replay reset, draft/history preservation, exact401/403 denial and temporary-error controls. Current browser reset/transport/denial checks passed, including exact scroll anchor, preserved draft, cache retention/purge and a ten-second polling-free idle window. An observed hidden denial notice is fixed and browser-verified: access denial now shows sign-in recovery and Settings. Current initial-workspace404 handling is also verified against actual HTTP, native/shared checks and compiled browser code: deleted workspaces clear cached access, while transport and tool/page404 failures retain readable history. Mac acceptance remains pending |
| Performance | Expected provider readiness polling uses a bounded two-second cadence. API grant optimization is retained in deployed `d4880c04`;19 integrated checks cover grant safety plus App identity and bounded webhook payload handling | E2B cleanup and Boxd return passed before rollout. No live speedup claimed. Prepared-cache targets and subsecond application resume are not achieved |
| Environments and hosted web | Recipes and managed bases exist; reusable project-cache migration0028 remains disabled | An actual project-preparation CLI panic is fixed; corrected guest `1f2e4b942c8a` passed real Git preparation twice with setup once and is owner-only published on Boxd. Private source authorization, encrypted source bundles, worker orchestration and the isolated pilot passed focused Rust and isolated-Postgres checks; migration0028 remains disabled and live cache behavior unverified. Native staging API-origin and daemon environment inheritance fixes passed focused source checks. Browser component tests do not establish standalone hosted web: isolated browser sessions/controller code is deployed disabled in API `1f990553`. Current compiled browser entry, session/cache recovery and idle checks passed locally; signed-in hosted/Mac and durable restart-login acceptance remain pending |
| Migration and release | Staging preserves existing production services/data; no customer cutover | Full provider/agent fault matrix, migration rehearsal and production review remain unfinished. Legacy decryption key and narrower object-storage credential scope were skipped external release prerequisites; previews remain deferred |

The additional Boxd controls case passed one staging API restart with live tool
continuation, native steering, interruption of the actual new turn, queue add/edit/
delete/promotion and a real Grok response. Four physical execution counters each
remained one. The original test runner rejected a legitimate model-binding change;
independent reconciliation passed all ten preserved remaining assertions. Its owned
computer was deleted and provider404 confirmed. This does not certify optional RAM
wake timing on the new API or the entire provider fault matrix.

Latest Boxd single-case measurements: Create HTTP **2.163s**, requested-to-repository
ready **30.660s**, first visible text **44.604s** including an intentional three-second
tool hold, and guest wake **2.660s** (authorization469ms, grants1741ms, ACK442ms).
Latest Boat repository readiness took **62.670s**, including **40.204s** allocated-to-enrolled; its cold Sleep was **13.946s** and cold wake-to-ready **10.727s**, with changed runtime PID/start and retained disk work. Latest E2B full application resume was **10.779s**, including guest authorization
**3.739s**, grants **1.796s**, ACK **493ms**, and Core reopening **6.034s** after local
wake. Daytona Create HTTP took **2.163s**, requested-to-repository ready **21.079s**, first observed Claude text **33.579s**, cold Sleep **14.171s** and atomic cold wake **10.945s**. Its allocated-to-enrolled interval was **2.663s**. Intentional tool holds and reconnect are included; these are separate single-case milestones, not percentiles or fulfilled targets.

Evidence: `.context/hosted-cache-release/deployment-independent-final-verification.json`,
`.context/hosted-final-v2-browser-acceptance.json`, `.context/daytona-managed-agents-a49bc3980241/daytona-a49-four-turn-final-acceptance-checkpoint.json` (SHA `ba34fc0afd10ba592a4dfdc575eeea1a906459a70392b65d68e7b53fe4083ba6`),
`.context/boxd-a49-four-turn-final-acceptance-checkpoint.json`,
`.context/browser-cache-current-wasm-final-acceptance-checkpoint.json`,
`.context/native-active-history-provisional-cache-verification.json`,
`.context/grant-latency-integrated-verification.json` `.context/grant-latency-staging-deployment-checkpoint.json`,
`.context/browser-cache-visible-denial-final-acceptance-checkpoint.json`,
and `.context/e2b-a49-four-turn-final-acceptance-checkpoint.json`
(SHA `994fb4104effca2c00494598037d843022139502230aae9e835506566b072103`). Historical failures remain retained; the new
successful cases do not certify untested combinations or fault conditions.
<!-- ROOT_CURRENT_STATE_END -->

Older chronological checkpoints below are retained for audit. They do not describe current running sessions; the state above and `.context/cloud-computers-critical-state.md` take precedence.

Accepted implementation scope: a computer per task; composer creation; Zuse-managed
Boat, Boxd, E2B and Daytona; Claude setup-token/API-key and Codex/Grok device-code/API-key
connections; public and private GitHub organization parity; optional environment
recipes and automatic preparation; native/web history cache and realtime; all twelve
provider/agent combinations required for release acceptance.

## Current implementation boundary

The deployed API and worker are infrastructure, not a certified cloud-computer flow.
Staging repository selection uses the existing GitHub authority. The operator's
staging account has a bounded manual allowance. Production allocation stays disabled.
The unfinished access-token import experiment has been removed; it was never deployed.
No model credentials have been imported from the operator's Mac.

## Work tracking

- [x] Restore cloud compilation after removing the incomplete credential experiment.
- [x] Correct GitHub installation initiation to allow organization members, matching
  the original permission contract. Installation removal remains administrative.
- [x] Define shared computer/provider capabilities and validated environment recipes.
- [x] Add application tables for environment versions, managed bases, agent connections,
  login operations, tasks, computers, lifecycle operations, hot messages/tools,
  replayable events and the command outbox. Staging has19 applied migrations through
  0016; later event, joining, vault and candidate migrations remain unapplied.
- [x] Wire capability/environment APIs and shared settings controls.
- [ ] Implement encrypted credential authority, login/refresh workers and live grants.
- [ ] Port complete standalone GitHub authorization, organizations and webhook parity
  from source commit a0155d2dd11f07495a99d3737c9bd40a5c8b4508.
- [ ] Implement and certify provider-neutral lifecycle adapters for all four providers.
- [ ] Publish verified managed bases and automatic project preparation/fallback.
- [ ] Complete composer creation, preparation activity and Computer controls.
- [ ] Complete Postgres hot history, durable event replay and native/browser caches.
- [ ] Rehearse retained-data migration and certify all twelve live combinations.

## Release gates

Agent/provider success means a real streamed turn and tool edit, continuation,
interruption, background execution, reconnect, sleep/resume, recovery, Git/PR,
terminal/preview, recreation and deletion. API health and provider allocation alone
do not pass a gate. Keep fresh preparation and cold recovery timings separate from
prepared startup. Never claim uncached installations complete in three seconds.

No warm pools, speculative allocation, automatic Git mutation on resume, database
notifications, inherited credentials in snapshots or automatic replay of uncertain
tool actions. Keep staging and production on the existing Hetzner host with external
Supabase Postgres and isolated environment credentials.

## Credential and runtime implementation checkpoint

The new authority stores encrypted selected connections, admits idempotent login
operations, leases verification work and serializes refresh per connection. Tests
exercise encryption, scope isolation, ten concurrent refreshes, revocation during
refresh, cancellation and API admission without compute entitlement. Runtime grant
delivery is bound to the selected connection, runtime generation and recipient key;
refresh tokens stay at the authority. These paths are not enabled on hosted services.

The private Python driver uses isolated profiles and pinned operator-supplied
executables. It supports Claude setup-token/API-key and Codex/Grok device-code/API-key
verification and renewal. This implementation does not replace provider client
registration or commercial hosted-auth prerequisites.

Grok has a guest external-auth socket bridge with access-only responses, bounded
requests and renewal failures that cannot fall back to a rejected token. The guest
registers its selected agent and keeps that identity on resume. Managed releases
can include all three executables and preserve old two-agent release pins. The
authority image and optional same-host auth-worker profile are prepared, not deployed.

Boxd inspection now distinguishes suspended memory, hibernated disk and stopped
machines; retained lifecycle and recovery select Resume, Wake and Start respectively.
Guest environment initialization has a scoped durable receipt, repository recipe
precedence, bounded logs/deadlines and uncertain-outcome protection. Task creation
pins the selected environment revision and requires an image that attests recipe
support. The guest runs initialization once, preserves its receipt on resume and
supervises declared services with port readiness and fenced process-loss recovery.
Authority-enabled Boxd creation can now use a verified platform base without a
project cache. It admits an empty public checkout without a customer build or
snapshot cleanup record, pins the release through allocation, and restores archives
against that same release. The new catalog uses promoted harness manifests and
connected accounts. Legacy deployments retain their existing image path. Managed
base publication/certification, private runtime Git delivery, the other providers'
controller integration and setup recovery actions remain incomplete. See
[managed-base admission](cloud-managed-bases.md).

The shared worker provider interface now has Rust adapters for Boxd, Boat, E2B and
Daytona, including allocation, inspection/reconciliation, lifecycle receipts,
execution and file uploads. The registry isolates invalid/missing provider config.
Transport tests cover Boat's original Stop ID, pending/retrying/superseded results,
E2B's access-scoped envd token and streamed exit receipt, and Daytona's pinned class,
persistent lifecycle configuration and trusted toolbox/file-upload boundaries.
The deployed orchestration still selects Boxd; these new adapters are not yet wired
into complete task allocation, sleep/resume and recovery for all four providers.

Latest read-only provider check: Boat authenticated listing returned HTTP 200.
The available Boat key was saved privately in Hetzner's staging worker file without
restarting services. E2B and Daytona keys are not visible in this running workspace
or the inspected staging file, so their live checks are deferred. No test machines
were created during this implementation checkpoint.

No live agent turn has passed the twelve-combination acceptance matrix. These unit,
transport and isolated-Postgres tests are implementation evidence, not release certification.

## History and preparation checkpoint

Migration `0011_transcript_heads.sql` adds a bounded encrypted opening screen to the
same Postgres row as its authoritative cursor. Matching heads of up to 256 KiB can
be returned without an object read; historical pages and larger heads keep their
object-storage path. Cache tags bind generation, epoch, version and digest. Stale
tags fall back to object storage, corrupt matching heads fail integrity checks,
and normal scope/deletion checks still run before returning a transcript key.
The isolated integration test verifies zero opening-screen object reads and that
archived history reads create no compute jobs. This does not implement the complete
normalized message/tool store, incremental guest publication or browser cache.

The managed creation path also enables discrete, generation-fenced repository and
environment stage reports. The guest reports initialization and service readiness,
and repository completion pins the journaled fetched source commit on the server.
Reports cannot admit agent execution or overwrite lifecycle/ready status. Completed
stage receipts survive restart; resume retains its checkout. Error codes are closed
and contain no vendor stderr. Public workspace summaries expose safe stage facts,
and connection status uses readable stage labels. The complete expandable activity
strip and log/recovery controls remain outstanding.

## Provider-controller checkpoint

The worker source now selects the provider from each workspace's durable identity
for allocation, retained lifecycle and deletion. Boxd, Boat, E2B and Daytona use
the shared adapter; missing or invalid configuration for one provider does not
prevent other providers' independent work. Non-Boxd admission requires the managed
controller version and a compatible operator-certified base. Existing legacy Boxd
admission remains supported. The deployed worker has not been replaced yet.

Migration `0012_provider_receipts.sql` persists submission intent and original
lifecycle receipts. Provider calls run outside Postgres transactions; receipt
storage survives lease loss. Pending, retrying, superseded and uncertain effects
remain distinct. An uncertain sleep is observed rather than resubmitted. A stopped
appearance alone cannot settle an operation that requires a provider receipt.

Allocation reconciles the original name/ID before further action. A pending or
unknown allocation journal cannot be overwritten to submit another create after
an eventually consistent empty lookup. Confirmed IDs survive independent of the
workspace update. Bootstrap credentials travel through private file upload.

Deletion similarly persists its exact target and submission before dispatch,
retains the response after lease loss, observes completion before purging objects,
and performs provider/object I/O outside database row locks. It retains existing
sharing, generation, ownership and deletion fences.

All 57 cloud tests and 9 API integration tests pass, including isolated database
tests across all four provider identities, delayed allocation visibility, lost
responses, original Stop identity, worker replacement, expired leases and row-lock
probes. The schema isolation suite passes. These are simulated provider/controller
tests; no provider/agent combination has gained live certification from them.

Portable archive capture/restoration now has provider-neutral source integration
(see the latest checkpoint below). Memory-preserving guest parking, certified base
publication and full Computer UX remain outstanding. The deployed archive worker
still predates this refactor. Boat's retained-disk expiry requires a tested durable
recovery policy before customer certification.

### Recovery retirement hardening (source and isolated PostgreSQL verified)

The recovery retirement stage now saves its original provider submission/receipt in
migration0013, commits database locks before provider calls and rechecks the durable
backup manifest/hash and exact source before completing. Lost replies and expired
leases reconcile that original operation; an accepted delete is not completion.
The generic recovery error path no longer sends unfenced stop/pause commands.
All58 cloud-library tests and6 isolated schema/role tests pass. The current guest
rebuilt in74seconds. All8 broader recovery/deletion acceptance tests passed in
30.25seconds, including restored conversation continuation. These fixtures share
workspace IDs across schemas, so their provider-effect guard requires a serial
runner. The partial-restore deletion case now explicitly proves a replacement
exists before deletion, and retirement assertions require a completion observation.
This does not complete the remaining recovery/restore stage refactor, registry
recovery dispatch or any provider live certification. Nothing here is deployed.

Standalone GitHub migration cases are inventoried in [cloud-github-parity.md](cloud-github-parity.md)
from the required original commit. The staging signing key is now privately
installed and verified against App5150694 (`zuse-staging`). Authenticated discovery
found three active installations. Worker configuration is prepared but not restarted;
no private-repository or standalone account/organization acceptance is claimed.
The owner deferred production key work after the older supplied file failed verification.

The read-only `infra/cloud/github-app-check.py` preflight can verify a privately
installed key against a numeric App ID without printing the key, JWT or vendor
errors. Four tests verify an actual RSA signature, private-file requirements,
secret error filtering and redirect rejection. It is included in cloud CI, along
with the serial recovery acceptance suite. The live preflight verified only the
staging App identity and installation discovery, not a complete user Git flow.


## Portable recovery controller (source checkpoint)

Archive upload, cancellation and verification preparation now use short, fenced
admissions around provider inspection, helper commands and sleep/wake operations.
The original wake/Stop receipt survives a lost lease or reply. Pending, retrying,
completed, superseded and uncertain results remain distinct; a machine appearing
stopped cannot settle an original Stop that is still pending. The helper runner's
durable operation identity and completion receipts make retrying its launch safe.

Restore similarly commits before provider lookup/allocation, helper installation
checks, restore commands and runtime launch. It inspects a confirmed allocation ID
instead of misusing a name lookup, uses its original allocation operation ID and
pins the original verified archive/base. Migration0014 records replacement wake
receipts. A pending wake cannot be bypassed merely because inspection says running.
Status writes and delivery settlement retain generation/active-lease fences.

The shared adapter and worker registry now route archive/recovery/restore as well
as allocation/lifecycle/deletion for all four provider identities. Credentials
remain private file uploads; no archive capability appears in a command string.
Managed non-Boxd recovery requires the controller version and enrolled full-state
recovery capability. This is source integration, not provider certification.

Verification so far:59 library tests,6 schema/bootstrap tests and28 broader API/
runtime tests passed (the browser-driver test was excluded). The new nine-case
recovery suite passed, including replacement wake receipt ordering and row-lock
probes for every provider interaction. Repetition subsequently exposed a late-ready
race: a retry could overwrite a concurrently resumed runtime with launching.
The fix preserves the enrolled online runtime's transition. Five consecutive real
guest restores with an injected late-ready receipt passed. Final verification on
that fix passed59 library tests, all9 recovery/deletion cases (34.69seconds) and the
API check (9.59seconds). No new release/migration has been deployed and no managed
base has been published.

## Retained engine parking and wake foundation

The engine now has a shared reversible admission barrier across RPC mutations,
agent dispatch/steering, mailbox admission and command/queue drains. It rejects new
work, lets already-admitted nested calls settle, and waits for their permits before
checked snapshots. Cancellation or a wrong-engine token cannot reopen execution.
Parking stops supported turns in place and retains the engine, harness roots and
PTYs. It fails explicitly when an active harness cannot settle without teardown.
A real harness/PTY test verifies the same shell PID and root after parking, blocked
terminal input while parked and exactly one queued followup after reopening.

An owner-only Unix wake listener provides bounded, task/generation/disk-scoped local
notification without API polling. A wake signal is not execution authority. Recovery
excludes this transient control directory, including stale socket nodes after a crash.

Verification:101 relevant engine tests passed; one optional installer download was
ignored. Three wake-socket tests and11 archive/recovery tests passed (one packaged
helper test ignored). API/guest compilation and scoped formatting passed. These are
in-process and isolated filesystem tests. The executable still uses cold shutdown;
parking receipts, Boxd memory suspend, lifecycle wake delivery, long-sleep credential
renewal and actual provider/harness memory preservation still need integration and
live verification. No new release, managed base or customer feature has been deployed.


## Parallel integration checkpoint — 2026-10-30

Three implementation agents are working concurrently on provider/base releases,
standalone GitHub and computer/history UX. The primary agent integrates the guest
retained-process handshake and packaged acceptance. All changes remain on the
existing branch; no customer cutover, production restart or new staging deployment
has occurred in this checkpoint.

- Guest source now retains one EngineCore/IPC/harness/PTY graph through authenticated
  memory parking and explicit local wake. The guest renews expired runtime authority
  before reopening execution. Parked acknowledgement loss does not destroy retained
  processes. The server storage capability allowlist now admits the new sixth
  memory-sleep capability; packaged acceptance exposed the previous rejection.
- Provider controller tests pass original memory-operation reconciliation, lease
  replacement and uncertain response handling. Boxd transport uses SuspendVm, and
  only actual standby proves RAM suspension. Managed opt-in requires matching
  provider/manifest certification; all four providers can contribute certified
  installed model catalogues. These tests simulate providers, not live RAM guarantees.
- Selected credential checks run at the next idle turn boundary. Codex's real
  subprocess fixture verifies re-login without replacing its child/thread. Changed
  static-environment credentials fail explicitly; safe automatic Claude/Grok child
  replacement remains a separate incomplete requirement.
- Standalone GitHub has encrypted actor-bound OAuth operations, explicit installation
  selection, refresh serialization, scoped Git grants and PR routes. Native rollout
  remains off, with the original Worker bridge preserved. Five native fixtures passed;
  the added organization/departure gates await their current targeted run. Private
  guest Git delivery and native installation chooser work is still in progress.
- Composer tasks open immediately from durable admission, and task history now has
  encrypted ordinary message/tool parts with real-cursor indexed pagination. Two
  isolated Postgres cases passed. Complete tool-output sidecars, browser IndexedDB,
  explicit cache purging, archived pagination and rendering performance remain open.
- Computer UI exposes real lifecycle/terminal/files/previews actions and explicit
  destructive confirmation. Restart-runtime/recreation backend contracts are still
  incomplete. Bounded encrypted preparation logs are source-wired and awaiting
  integration checks; they must not expose raw output in public workspace summaries.
- Provider-specific base builders and pinned native harness download/provenance tools
  are in progress. No managed base has been published or provider/agent combination
  newly live-certified. Discovery/version probes are not compatibility certification.

Fresh independent validation: six schema/bootstrap/isolation tests and four GitHub
signing-key preflight tests passed. Packaged memory acceptance is still being
completed; its isolated simulated-provider timing must never be presented as a
live provider latency measurement.

Staging prerequisites: the valid staging signing key is retained privately. A separate
webhook secret was generated at /etc/zuse/cloud/staging/github-webhook-secret with
root0600 permissions; no secret value was printed or put in the workspace. The owner
was asked once to save an additional staging OAuth client secret and register the
new callback. Production signing-key work remains explicitly deferred. API-only
read-only secret copies/configuration are being prepared so provider worker keys
never enter the API container. R2 configuration, migration rehearsal, staging rollout
and the complete real-agent acceptance matrix remain required. E2B/Daytona keys were
still absent from the inspected staging worker file; independent Boxd/Boat work
continues.


### Integrated verification and staging schema applied

Packaged memory acceptance passed2.48s against a real guest and isolated Postgres:
lost Parked acknowledgement, expired bearer renewal, same runtime/PTY process,
retained shell environment and uncommitted files, canonical delta application and
exactly one follow-up. Simulated-provider resume to server Ready measured110ms;
this is not a live provider latency. Tested candidate guest SHA256:
97ca1fa5f6fd57512341e21d807f972b32ea0d9803bea8d625039d9e344ad425.
That candidate predates the new private-Git/tool/log guest integration. The later
combined API/guest metadata check passed52.89s and Git helper3 tests passed0.84s;
its packaged guest must still be rebuilt/tested before release.

The owner saved the staging OAuth secret and confirmed callback registration.
Private-file checks passed; a read-only live GitHub client-secret control verified
the pair without a user token. Individual API-only secret copies are UID10001/0400,
root originals preserved. api.env remains explicit native rollout empty.

The existing staging schema was privately backed up on Hetzner at
/etc/zuse/cloud/staging/backups/before-computers-20261030T071916Z.dump.
Reviewed transactional staging migrations advanced11→19; production remained11.
Restricted staging role can write new history/GitHub tables, cannot access
production, cannot update its migration ledger and cannot publish managed bases.
No service restart or API/auth/provider rollout occurred. Migrations0015/0016 are
now immutable; future schema changes require a new migration. Six schema tests
passed against the complete19-migration bootstrap before this application.

A single isolated Boat builder probe allocated in386.95ms, then failed upload
preparation. The exact builder was deleted and absence verified; no snapshot was
submitted/retained. This is an unresolved live driver failure, not base publication
or successful cloud readiness. Claude2.1.314, Codex0.166.0 and Grok1.0.70 signed
native candidates passed empty-HOME version probes on Hetzner; exact candidate
harness release1066ed1f2bfec220fcc0cfa70d643ac94861978c8280afd9f72c44fe236472d5
is staged without promotion. Candidate-only staging authority bootstrap now has
explicit singleton-account/environment guards so real account onboarding can
collect certification evidence without inventing promoted-provider results.


### Staging service deployment and R2 live verification

Staging API, lifecycle worker and singleton-owner credential worker are running
from cloud-computers-stage-f474475dc5831427 with signed native candidate harnesses.
Production containers and the shared production release pointer were unchanged.
This deployed snapshot predates the newest GitHub consent/organization and guest
supervision changes; native GitHub and customer compute cohorts remain disabled.

The owner saved private R2 credentials. The first real Rust object-store check
failed because configured dedicated bucket zuse-rust-staging did not exist.
Read-only Wrangler evidence matched the configured Cloudflare account; the older
zuse-cloud-transcripts-staging bucket contains data and was not modified. Created
the intended dedicated zuse-rust-staging bucket through authenticated Wrangler.
The actual Rust object-store probe then passed upload, readback, immutable retry
and collision behavior, deletion and absence verification for237568bytes.
cleanupVerified=true. Secret values never entered workspace or logs. Private
normalized storage.env is wired only to staging API/lifecycle worker; authority
has no R2 credentials. Service rollout is tracked separately from this probe.

Provider runtime boot remains unresolved. OCI Boxd command/supervisor probes have
not established a durable boot owner; all completed diagnostic VMs were deleted
and absence verified, no snapshots retained. The pinned original uses native
Boxd/systemd rather than the OCI class. One bounded independent Boat tracked
process probe is in progress; do not claim a published managed base or successful
provider/agent acceptance from allocation/version or fixture checks.

Full Cloud Computers is NOT complete. Required work includes managed-base live
certification, real agent authentication/turns/tools across providers, account
event transport, runtime restart/recreation, workload-secret end-to-end tests,
GitHub private/organization live parity, migration rehearsal and release gates.

R2 Compose rollout completed successfully in staging release
/opt/zuse/cloud/releases/cloud-computers-stage-f474475dc5831427-r2-60f2fa6035.
API/worker now receive the verified storage configuration; credential authority
does not. Production container identities/images remained unchanged. Both public
TLS ready endpoints passed after rollout. This is storage deployment, not full
end-to-end computer acceptance.


### Parallel delivery checkpoint: realtime, storage and provider boot

Four agents are active: primary owns account realtime/deployment integration;
computer_experience owns shared UI/history/cache/workload secrets;
memory_provider_control owns provider boot and the explicit staging candidate base;
standalone_github owns GitHub parity and independent migration rehearsal.
Overlapping Rust edits are now collected into one coherent build checkpoint,
instead of starting builds while a required module is still being written.

Actual new evidence:
- Real server Postgres+R2 tool sidecar test passed on Hetzner:336000bytes in6 encrypted
  chunks, immutable retries, conflicting replay rejection, cross-owner denial,
  complete read while the computer stayed paused, and verified object/schema/tunnel
  cleanup. Credentials stayed on Hetzner. This is not a real guest Publisher/agent test.
- Account events HTTP/WebSocket+isolated Postgres acceptance passed1test/0.05s for
  one-use tickets, Origin rejection, private-task filtering, reconnect cursor replay
  and access revocation without a runtime/provider. Source then expanded to combine
  personal agent/auth/GitHub invalidations with organization status on the SAME socket;
  expanded Rust fixture still awaits the combined build. Opaque actor/scope/API-bound
  cursor pairs advance only after the bounded batch acknowledgement.
- Fresh-device cursor and visible-task baseline now share one repeatable-read DB
  snapshot. Initial connections revalidate current metadata instead of replaying a
  week's old invalidations. Native/browser viewport feed hooks are source-attached;
  ordinary events preserve transcript and scroll, revocations invalidate caches.
- Actual isolated Postgres outbox suite5tests passed2.323s, including transactional
  rollback, late commits, stable cursors, heartbeat/lease exclusion, safe project/
  environment/GitHub phase events and no secret payload/database notifications.
- Vault tests passed real encryption/recipient binding plus isolated Postgres
  owner/project/version rules. Creation/bootstrap secret binding is source-wired,
  but required certification marker defaults absent and prevents false support.
- Real Boat managed-base preparation/snapshot readiness passed; disposable VM and
  snapshot deleted. Native Boxd systemd snapshot restoration AND cold resident
  startup passed; final dirty-file readback hit a probe user's file permissions,
  now fixed in the probe. All12 completed provider VMs and snapshots are absent.
  No base is verified/promoted. Next build will become one explicitly uncertified
  owner-only staging candidate only after complete mechanical proof and cleanup.

Staging deployment still uses API f474475dc5831427 with storage overlay
cloud-computers-stage-f474475dc5831427-r2-60f2fa6035. New realtime, latest GitHub
consent/setup, vault and candidate changes are NOT deployed yet. New migrations
0017/0018/0019/0020 remain source work; staging currently19 applied migrations
through0016, production11 unchanged. No real agent/provider matrix is certified.
Remaining complete-flow gaps include real authority login/model turns, guest tool
sidecar/reconnect acceptance, all provider lifecycle certification, restart/recreate
controls, retained-data rehearsal and the performance/release gates.


### Latest staging deployed: source checkpoint3d17 / true staging identity

Combined validation passed: API+guest metadata14.17s; native viewport --tests
metadata14.54s; full web wasm metadata13.44s. Cloud unit60passed/28ignored;
isolated Postgres acceptance passed dual-room realWSS1/0.11s, nativeGitHub11/1.97s,
candidate1/0.05s, organization worker no-wake-after-departure1/0.52s and vault1/0.34s.
Actual engine secret-overlay PTY regression1passed0.02s and account invalidation
validator1passed. Candidate-only catalogue-metadata refinement then passed2unit
tests and its expanded PG test1/0.05s. Bootstrap6tests and outbox5tests passed
against all23 migrations. Synthetic retained-data rehearsal passed separately;
it is not an original retained-account import.

API SHA3d17bc4953590ac1ff66d31f5092ea31f1bfda63d373ea4662bef0ff00f7fc70;
pre-redaction guest SHA0a6273c9da7c485563f827fd02357d3e1e30c509c3126565d51b8de2c6105eb1.
Staging release /opt/zuse/cloud/releases/cloud-computers-stage-3d17bc4953590ac1
image sha256:cba4ed94f7183d537f735d0cf79299a37e439b9dce4a9e4c4cfd2f9cb3542293.
Staging19→23 migration backup /etc/zuse/cloud/staging/backups/
before-computers-20261030T100109Z.dump. Migrations0017/18/19/20 NOWIMMUTABLE.
Production remains11 migrations, unchanged running container identities/image,
unchanged /opt/zuse/cloud/current pointer. Staging API/lifecycle worker healthy;
account events enabled and private Unix signal socket verified live. New GitHub
setup route exists but standalone write cohort is still off pending owner canary.

Owner supplied WorkOS key privately. HTTP200 authenticated key; comparison with
deliberatelyinvalid OAuth codes proved prodclient invalid_client vs originalstage
client invalid_grant. Pinned TS deployment profiles confirm oldRust pilot mixed
production identity into staging. Corrected STAGINGONLY client/JWKS/legacy bridge
to original stageclient_01KW6ZEZKVMZ0G429A89XZD83Q; root API-only private key config,
no user/financial ID remapping. Old pilot project+entitlement preserved. Stage auth/
compute cohorts empty until freshly verified stageidentity; old authority worker
explicitly stopped, not represented as currently available. API/lifecycle entrypoint
now bypasses agent-authority launcher; authworker alone uses pinnedtool launcher.
Eight deployment tests passed1optionalDocker testskipped; deployment overlay digest
recorded separately from original artifact.

Actual owner device login started remotely at /etc/zuse/cloud/staging/owner-canary.
Macbrowser opened private approvalURL without printing URL/code/JWT; poll SSH
session99148 confirmed live, waitinghuman. DoNOTstart a duplicategrant. All secret
state outsidecheckout. Mac encrypted-cache test background worker PID46574 at
/Users/whizzy/Library/Caches/Zuse/cloud-cache-canary; syncedcheckout exists,19.81GiB
free beforetest, outputs/status private there. Need inspectactualresult/pid, doNOT
restart merely after observationtimeout. No provider VM/snapshot active (12prior
VMs and all temporary snapshots deleted+absence verified).

Current parallel ownership: UIagent critical selectedagent/workload known-secret
redaction before journal/toolsidecar (operational requests must stay original);
provideragent implements missing runtime-repair operation/supervisor receipts and
prepares final candidate snapshot (publishheld until final guest/redactor). GitHub
agent ownerlogin helper+canary and migration rehearsal. No root compiledsource
edits during those active ownership lanes. Next provider artifact is ONE retained
explicitlyuncertified stagecandidate ONLY after mechanics/retention/VMcleanup pass,
then real selectedowner Claude sonnetalias turn; Codex/Grokmodelcatalogues require
actual account/harness discovery, no invented IDs. E2B/Daytona keys remain absent
from both process and stagingworker file despiteearlier Conductor addition report.


### Verified staging checkpoint: real owner auth, native credentials and first base

This entry supersedes the earlier pending-login and cache-process notes. The real
staging WorkOS identity is user_01KW7R9WGJFFSKDNESE7RN00N1, verified against the
staging JWKS/client and WorkOS user endpoint. No production identity was remapped.
Codex device authentication completed through the deployed credential authority;
connection connection_b9c2fe3f35d546fa8e73357fca0239f6 is connected, with renewable
credentials encrypted in server Postgres. Claude/Grok connections and real model
turns are still unverified. Model discovery is using that exact selected account;
a harness handshake failure is being diagnosed rather than inventing model IDs.

Deployed API b3e07665a2d2ef67 fixed native GitHub using the read-only presentation
pool and created isolated native auth profiles before launching device login.
GitHub browser authorization remains pending. User screenshot shows the exact new
callback https://api-rust-stag.zuse.sh/v1/cloud/github/callback; user instructed to
save it under zuse-staging App5150694 and retry. Old callback URLs are preserved.
No private Git or organization acceptance is claimed yet.

One intentionally retained, uncertified Boxd base snapshot exists:
snap_2418165fe97e437dad241a63f00af846. Its guest SHA is
0eb13f724ef0422e48fda7c8da6b91e652d4ab516c7d72b372b9a5791ac9d161.
All temporary test VMs were deleted with absence checked. Mechanical restore and
cold-start tests retained dirty files; these are not real agent readiness timings.
The first base does not attest repair, recreation or workload-secret capabilities.
Candidate activation exposed a FOR SHARE query on a read-only presentation pool;
compute/pin were temporarily disabled while preserving the artifact and auth.
A real read-only Postgres regression passed for the fix. New API9b2f386a8aa14084
was captured and staged for deployment; final deployment result must be inspected.
Staging migration0021 was applied with backup
/etc/zuse/cloud/staging/backups/before-computers-20261030T112308Z.dump:
staging23→24, production11 unchanged, restricted-role isolation checks passed.

Live public-TLS account realtime with the actual staging login passed ready/ACK,
one-use ticket rejection and reconnect from the acknowledged opaque cursor,
without allocating or waking a computer. Observed ready times813.40ms and689.09ms
are two single connection samples, not p95/UI paint. Evidence:
.context/live-account-feed-result.json and .context/live-account-feed.mjs.

Actual Mac tests completed: encrypted projection cache2passed/0.22s;
Keychain read/update/per-account clear/all-clear1passed/0.07s; separately opened
SQLite cache provisional head restored through Keychain1passed/0.15s. The latter
fixture was corrected to use the same canonical origin as the production reader.
These are functional tests, not cached conversation paint measurements.

Managed process-loss policy is implemented in source and passed two actual process
regressions/0.92s: a real ACP child executes a shell side effect, kills the Rust
host with SIGKILL before the terminal ACK, then cold recovery atomically records
Unknown and neither prompt replay nor repeated command delivery executes it again.
Dirty work and exact preexisting terminal ACK/ciphertext remain intact; cross-owner
quarantine is rejected. Evidence .context/cloud-managed-real-crash-tests.log.
The retained base still has the older guest; this policy needs a new built/published
base and live certification. Repair refinement and recreation remain separate
source/deployment gates. No provider×agent combination is fully certified; full
history/tools/terminal/previews/lifecycle/customer migration acceptance remains.


### API9b2 deployed and Codex candidate activated

Staging API/lifecycle/auth worker now run immutable image
zuse-cloud:computers-stage-9b2f386a8aa14084,
sha256:881ae037ced32f76a7b51871dea9a4e25a63ce6d96d3dcb32d4cf60edadb1d12.
API SHA9b2f386a8aa1408492c7e20c738a10259819ec3bb9a10b188fbe956f2781f230.
Host deployment receipt verifies production container IDs/images unchanged.
Public authenticated realtime was retested after deploy: ready832.09ms,
reconnect592.22ms; ACK/reused-ticket rejection/cursor continuation passed.
.context/live-account-feed-9b2-result.json contains only safe result metadata.

Actual selected-account Codex model discovery returned8 models with default
gpt-6.1-sol, using pinned native0.166.0. Initial operator failure was an explicitly
configured CODEX_HOME directory not created before CLI launch. A contained0700
mkdir+exec wrapper proved the cause and returned the genuine account catalogue;
source now creates the private directories directly and adds a regression.
No model credentials were exported. No new provider VM was used for discovery.

A new immutable candidate metadata revision, using the SAME snapshot, is activated:
zuse-base-8d73c6f388eca5579e44123d1a5b8e2e-canary-2e3b2275fa97690d,
candidateSHA6129c24411e57b3e5dc6e284dce847a775a291e0deb05a6d8e88283f0fac08a3.
Only the exact3 candidate/auth/compute keys were merged into current staging env
files; no older prepared file overwrote current config. First candidate presentation
now succeeds with the actual owner JWT. Project-catalog Rust ownership and project
write cohort were enabled only for the freshly verified staging owner; no financial
rows or entitlement mappings were created. Public octocat/Hello-World connected as
project_19387e16813b4df4af20c2ce7fd6d921 in589.61ms.

FIRST REAL TASK IS NOT YET ADMITTED: POST workspaces returned
503 cloud_database_unavailable. The exact request body and idempotency key remain
private on Hetzner in owner-canary/task-canary-request.json. Do not issue a new key
or assume no provider resource exists until authoritative DB/resource reconciliation.
GitHub agent is inspecting exact role/query/root cause without mutating state.
Source fixes are not substitutes for rerunning this same real admission.

New guest build with managed crash policy and latest repair supervisor is next;
old snapshot guest0eb remains an explicitly uncertified happy-path candidate.
Full plan remains active and incomplete.


### Live GitHub consent found native form-origin bug

User saved the exact staging callback and retried. New operation
15d432b5-48da-472f-9468-ec69f783a6e5 reached the installation chooser, but its POST
returned github_browser_origin_mismatch. API deliberately reports authorizing
until the browser confirms an installation, so that state did not prove callback
absence. Root reproduced the actual failure in Chrome: Referrer-Policy:no-referrer
makes native form POST Origin:null; strict-origin sends the exact same origin and
only an origin referrer (no OAuth code/state path/query). Changed only the GitHub
browser HTML response policy to strict-origin, retaining exact Origin and bound
HttpOnly-cookie checks. New explicit real-Chrome test exercises actual Axum helper;
build/verification/deployment pending. Minimal actual Chrome probe passed in
.context/github-origin-probe.py. Do not disable origin checks or permit null.

Managed-base allocation failure is being repaired without granting app UPDATE on
release metadata. Existing FOR SHARE queries need UPDATE privilege despite the
intentional revoke. A narrow SECURITY DEFINER exact-row lock helper is proposed
in0023 with PUBLIC execute revoked and app-only grant; three candidate/certified/
retained readers must recheck current locked data. Live SQLSTATE/root role probes
and actual provisioned-role regressions are pending. Do not retry a new task key.

New guest SHA a7d01bdf402ea801d87a8ff7121dfb0782c37b0a22f2e67116bc1fe21df683dd
(181233024 bytes) built85s, includes managed crash policy. Immutable copy in/tmp;
provider agent has authorization for ONE bounded new Boxd base publication and
mechanical proof with at most one active VM and cleanup before root task retry.
Old0eb snapshot and metadata remain immutable until new artifact is validated.
Native engine+viewport test metadata71s and full web wasm6.67s passed. These are
source/build proofs, not live agent or repair acceptance.


### Live failure diagnosis confirmed (no task was allocated)

The actual staging app role has defaultReadOnly off, managed-base SELECT true and
UPDATE false. A zero-row FOR SHARE probe returns SQLSTATE42501, confirming the
privilege mismatch without mutating database state. Exact failed request has
workspaceCount0/digestCount0; staging providerIdentityCount0. The first task was
not committed. Provider agent separately owns one authorized clean base-builder
VM4a8b5f52-9f65-4465-91d1-0bf49cd8f80b for new artifact preparation; root task
allocation stays paused until that probe cleans up. Do not conflate it with task
allocation. Production remains unchanged.

GitHub operation15d432 actual phase is choosing, browserBoundtrue. Callback and
OAuth exchange succeeded. Public operation masks this as authorizing until browser
installation confirmation. Human reported error at browser/select; real Chrome
proved the no-referrer/null-Origin bug. New browser policy + exact-row lock migration
0023 are source work undergoing full regressions/API build; not deployed yet.
No need to change callback settings or disable CSRF/origin checks.


### Deployed browser/managed-base fix and first real task admission

Staging release /opt/zuse/cloud/releases/cloud-computers-stage-6599535ddf71366f
is deployed: API6599535ddf71366fda2c443326455ad82d9e82df4cf57c20d35f41fb99d61bf1,
image sha256:837fa3747599ddb51ef4cbf8f99d85d03ead25c846f74c89e0d312e4d209b115.
0023 applied with backup before-computers-20261030T114406Z.dump: stage24→25,
prod11 unchanged; application registry UPDATE still revoked. Exact helper execution,
real provisioned-role certified/candidate admission, retained lookup and concurrent
revocation fencing passed. Chrome actual-handler form regression passed0.65s.
Production running containers/images and current pointer remain unchanged.

New candidate is ACTIVE on same verified personal owner:
zuse-base-52f1ef890f1b19e8700c0db399d308d7-canary-4ec10eec446860d7,
snap_3c6c359755514afca39bca4fc8a3328d,
candidateSHAda2fb94eae58b3bb12061047b12b5de7b5d58a5a2f6e43bad5e5fc34a6850f89.
Guest exacta7d01..., actual supervisor/repair/recreation helper versions1, workloadNone.
Mechanical restore/cold-start, dirty0600file and resident-process checks passed;
source and restore proof VMs deleted with absence verified. Still UNCERTIFIED.
Old0eb snapshot retained until new canary acceptance. New pin merged only3keys.

Original idempotent task request now DURABLY ADMITTED:
workspace_1ef490bba47e4e52a220cf32976e7190, observed admission2936.35ms.
This FAILS the300ms target; no percentile claim. Actual public gateway connected
pending then closed after31.15s with no runtime/agent frames. Status is provisioning,
startupPhase allocating, runtimeoffline, statusCode cloud_entitlement_required.
Provider audit: allocation planned/machineIdNULL, pending job8attempts; no task VM.
Admission correctly accepts explicit bounded candidate allowance, but worker capacity
checks still demand entitlement/billing. Provider agent owns matching exact-pin
worker allowance fix and uncertain-allocation reconciliation regression. Do not add
fake financial rows, issue new create keys or blindly allocate again.

Account event socket passed again on6599 with real owner auth: ready755.84ms,
reconnect735.52ms, ACK/one-use rejection/cursor replay, no compute wake.
Evidence .context/live-account-feed-6599-result.json. Prepared actual runtime RPC
stream probe .context/live-task-gateway.mjs prints no tokens or raw agent frames;
first result .context/live-task-gateway-first-result.json honestly records no frames.

GitHub new operation e1e08ea5-7fd3-4090-8c99-f08bc23554e8 was created after fix.
Conductor Mac command handle796 ended 'Timed out waiting for client'; do NOT claim
browser opened. User given SSH→privateURL→webbrowser command. Latest authoritative
status stillcreated, not connected. Original chooser proved callback successful;
selection submission fix is deployed, but human retry is still required.

Additional source-ready, NOT DEPLOYED: final organization membership check after
async credential renewal (realPG race passed); staging-only genuine renewal CLI
(no actual vendor renewal yet); explicit discovery tempHOME0700 regressionGREEN;
UI preparation failures have specific recovery, no fake Retry for missing setup,
and distinguish local rejected-create draft from admitted saved task (3tests and
native/browser metadata GREEN). Guest unchanged froma7. Discovery root0700 fixture
first failed and was actually fixed/retested, not silently ignored.

Read-only WorkOS inventory found a safe existing one-member creator-owned staging
org org_01M3FBPYGA55BA1HVKH01YADSR. Another3member team was left untouched. Native
org rollout/policies off. Distinct member-vs-admin live parity needs an additional
consenting member. Current candidate is personal-only; narrowly actor-bound org
staging pin extension is being drafted, not activated or represented as certified.
Boat new-base preparation has no allocation yet. Full plan remains incomplete.


### Admission performance measurement and pending worker repair

A30sample read-only persistent PostgreSQL session probe measured Hetzner→Supabase
SELECT1 RTT median41.68ms/p9541.93ms, min41.52/max47.99, excluding session/TLS
startup. This is a DB roundtrip measurement, not app latency or a deployment-wide
percentile. Evidence .context/staging-db-roundtrip-result.json and
.context/measure-stage-db-roundtrip.py. New task admission2936.35ms needs fewer
serial roundtrips; do not relabel an idempotent retry as new admission performance.

Exact-pin worker allowance +capacity-withdrawal reconciliation source is coherent;
restricted-role realPG admission→worker create→launch without entitlement/billing,
wronghash/expiry/secondcomputer rejection, provider-midcreate revocation and original
uncertain receipt adoption tests passed1/0.57s. API build pending. Existing live task
still planned/no provider machine; no replay/new allocation by diagnostic agents.
Known provider receipts were already persisted before capacity recheck; missing
original-outcome reconciliation before denied capacity was fixed in source. Full
automatic resource retirement after revoked creation remains a separate acceptance
gate, not claimed by this fixture.

For human GitHub retry, stable private helper is now installed at
/etc/zuse/cloud/staging/owner-canary/open-github-link.py. User invokes it throughSSH
and pipes directly to localwebbrowser, so an expired earlier link is refreshed only
on explicit human invocation. No sign-in URL/cookie/JWT is printed in chat. Mac
RunLocal remains unavailable (last client timeout); do not claim opening succeeds.


### Latest authoritative staging: API345, actual VM failure, cleanup requested

Staging CURRENT deployed release:
/opt/zuse/cloud/releases/cloud-computers-stage-345be8bea3d4330b
API345be8bea3d4330b97414c2a9f71cccf66c06bac30a76fb3de0050bd6e0fb0d4,
image sha256:93760e8687268c46489454104eb34213f6c80cf0def6cd68343e08e24930ee60.
Captured artifact contains EXACT25 migrations through0023 and exacta7 guest;
it was frozen before0024 source attached. Schema remained25, production11,
backup before-computers-20261030T120621Z.dump. Production containers/images unchanged.
Exact bounded staging allowance now works at worker capacity checks; realPG cases
passed without fake entitlement/financial records and uncertainty can't allocate anew.

SAME original task/op really advanced: VM1b9d000b-0280-4343-a9bc-ed81c4219286,
workspace_1ef490bba47e4e52a220cf32976e7190, allocation opb0f61326... launching.
Worker actualbinary345/pin/cohorts verified. Runtime enrolled, but no model/tool
execution happened. Exact failure: runtime_storage_migration_required; storage
freshness allowlist rejects new legitimate supervisor artifacts before native DB
preparation. Supervisor resident/running, child exits; identity0600 present,
storage/repository markers absent, codederror repeated. Provider agent owns narrow
binding/type/symlink-safe storage guard fix +real fixture +newguest/base. Do NOT
bypass legacy guard/delete unknown data/hotreplace active artifact.

Startup metadata observed: allocatedAt1793362091039/enrolledAt1793362095560,
enrollmentDuration4521ms. Current runtimeReadyAt equals enrolledAt even though
runtime was onlyconnecting; do NOT use that mislabeled field as execution-ready
performance evidence. This task was stalled during operator bugfix deployments,
so its totalrequested→elapsed is not a clean startup benchmark.
Actualgateway probes all saw no available runtime/agent frames; pending closes
at30s are expected gateway bound while bootstrapping, not proof of agent readiness.
Reports live-task-gateway-{allowance,345b,live-vm}-result.json are honest failures.

ROOT REQUESTED DELETE through authenticated API using private persisted
owner-canary/task-canary-delete-intent.json (same delete command ID on retry).
Deleteaccepted1301.89ms. Provider absence and terminalDB state still MUSTbe verified;
provider agent tasked with that. No new allocation until cleanupconfirmed. Then
root will temporarily clear only knownbadpin+CREATEcohort, preserving auth/GitHub,
until fixedguest/base is actually published. Root deployment helper currently
hardcodes repair-staging-env-patch.json (A7knownbad); MUSTswitch to newfixedcandidate
patch path before using it for the next compute-enabled deployment.

0024credential journal/receipt source is now final, NOTAPPLIED/notdeployed:
SQLSHA65470664293f0caf74609bbf8785512cfbacf8a913f02e9eab81b1ccc95f60e3.
FILES now26 in mutable worktree; current live/compiled345 schema remains25.
RealPG SIGKILL checkpoint recovery/uncertain-no-replay, concurrent duplicate/lost
Unauthorized sealed-response reuse, digest/revocation/expiry tombstones and org
scope race passed. Actualvendor renewal remains untested. Latest optimization
removes per-grant maintenance(2queries) and skips journal for nondevice secrets;
fresh record/epoch checks remain. New35s grantHTTP/45s Codex callback and new-turn
reset need a new guest beyonda7; do not claim those on currentartifact.

NextUI/source work: Reconnect must retain selected existing connection ID; worker
must reject different vendoraccount on sameID including coldresume, Addaccount
must make newID. UIagent owns this after storage capture. Org candidate actor-bound
extension remains DETACHED (do not attach during storagecompile); no org enables.
Admissionperf inventory is internal-docs/cloud-admission-latency.md:35SQLawaits +
BEGIN/COMMIT, ~33 coldParseDescribe extraRTTs explain2936ms. Fourcommand batched
lockedread/atomicwrite proposal is design only, no performance-path implementation.
Stable human GitHub SSHpipehelper available; no live nativeGitHub connection verified.
All agent/provider combinations remain uncertified. Fullgoalactive/incomplete.


Root authoritative staging checkpoint (2026-10-30, supersedes earlier source-only status):
Stage API138392187b179f13 is deployed with26 migrations through immutable0024;
production11 unchanged. Real selected Codex renewal version2->3/epoch2 unchanged
completed4302.38ms with encrypted journal applied and checkpoint cleared. No
expired-token or customer-hosted-auth certification claim.

Real default first-task workspace3b585ca0349746a6a958fda04c894cb3 on VM
c3e42bd5-8b2d-4f46-917b-d312bb8e0672 admitted2832.46ms (300ms target FAIL),
prepared checkout/enrolled runtime/credentialv3 and produced actual streamed
Codex response. Gateway ready2278.43ms, first assistant observed5382.20ms.
Agent tool failed: native exec could not spawn omitted codex-code-mode-host.
Expected file absent, no agent tool success; combination remains uncertified.
Complete vendor companion package fix ready as badfff harness, not published.

Actual public authenticated WSS controls passed on same computer: listfiles,
read README, terminal open/subscribe/write/execute marker/resize/close. Safe
receipt .context/live-computer-controls-result.json. No model call/allocation
for controls probe. No preview acceptance claim. Root requested computer delete
via stable API key; provider absence/SQL tombstone verification pending. Root
disabling incomplete2f CREATE/base pin, keeps auth/GitHub/prod unchanged.

GitHub strict-origin form fix deployed and actual Chrome regression passed.
Latest human operatione1e08ea5 now expired; callback originally succeeded but
selection failed. No native connected account verified. Human private SSHpipe
helper provided to begin fresh operation without printing secrets.

Actor-bound org candidate and guarded device reconnect source/tests green,
not current138 deploy. Internal-only Codex custom-tool observation gate under
implementation; raw events are upstream internal API and coldresume gap remains
release gate. Admission batching actual integration assigned;0025 reserved for
locked fresh-snapshot read helper, server-admitted-v2 remains detached/future0026.
All four-provider/three-agent combinations remain uncertified; goal incomplete.

Root cleanup/deploy-preflight follow-up: exact VMc3e42bd5 now providerNotFound;
workspace3b deleted/machineNULL/transcriptkeycleared; tombstonedeleted/gen2/targets0
and deletejobcompleted/errorNULL. Current138 CREATE+basepin disabled, onboarding
retained and production IDs unchanged. Private six task evidencefiles archived
under owner-canary/completed-canaries/incomplete-codex-package-workspace_3b585ca0349746a6a958fda04c894cb3.

Root added worker --check source, validates runtime/config and allocation/lifecycle
schema before returning, never starts reconciliation/cleanup/provider requests.
Singlehost up now uses this instead of effectful --once; accepts any configured
provider key. Python deployment isolation suite9tests/8passed1Compose-optin skip.
Rust source pending coordinated check/build; not live138 behavior yet.

Managed preview audit found guest discovery not started and no managed HTTP/WS
tunnel, so terminal/files success cannot certify previews. Dedicated agent now
implementing full tunnel/client wiring, held detached until urgent artifact capture.

Root complete-Codex artifact checkpoint (2026-10-30): full API+guest build
GREEN36.93s after provider45 and internal-gate2tests plus metadata51.52s.
Frozen tar .context/cloud-stage-a7e28b44e1503596.tar.gz (66464951B),
APIa7e28b44e15035967e43d901f303c457cdbe4b2ab3be08a6f6f17ca0c50f0f90,
gueste19240a84bde0d1431626f99ed311ef5f7d1c217697ef3ff8d2c4768a50f627d.
Source thawed for GH admission0025, UI durable-v2/future0026, memory previews.
These newer edits are NOT in captured artifact.

APIa7 deployed staging-only imageIDf6c5b431207044f088a92c89217449af3cff8ab78f77e16028ce8d80e0d419ab,
backupbefore-computers-20261030T133545Z.dump, stage26/prod11 unchanged.
Worker --check preflight actual log confirmed nojobsclaimed. Compute pin/CREATE
remainoff while complete14file base publication runs live in exactsession23959;
DO NOT restart because observationtimeout. Fresh actualowner accountfeed passed
ready774.82ms/ACK/singleuseticketrejected/cursorreconnect569.57ms withoutcompute.
Public receipt .context/live-account-feed-a7e28-result.json.

Complete-Codex sourcearchive4c5abb160ebe54377bb9df3fbf816bd3d7d8338cb4fadc7985be32bf44a3fc58
is onHetznerscratch. Publisheruses badfff release + exacte192guest.
Candidateprefixcomplete-codex-e19240a84bde. Newstage/activate scripts prepared
(.context/stage-complete-codex-candidate.py and activate-complete-codex.py),
NOTexecuted yet. Pin notgenerated until mechanicalproof+cleanup.

UseraskedwhynewpreviewDNS. Root told leaveDNSalone, checkingoldsetup reuse.
OldTS preview-url route indeed delegates providerDNS/TLS (Boxdlockedportproxy,
E2Bdomain, DaytonasignedURL), no priorZusepreviewHTTPproxy. Agent auditingreuse
so wildcardDNS no longertreated a hardprerequisite. Do not keepaskingDNS.

Userpriorityupdate: previews explicitly DEFERRED until coreflowdone. Agent stopped
previewcode at unexported/unwired crates/preview/src/managed.rs only; no main/router
wiring, noDNSneedednow. Goalupdated to first actualBoxd+Codex flow, no invented
percentages, otheragents integratedv2/admission/recreationparallel.

Existing publication23959 terminal0 observed, NOT restarted. CompleteCodex e192
mechanicalproofpassed, snapshot snap_a74f568c3e834f5bbb02ecb0bc328a0b retained
uncertified. SourceVM94e23a34-3b09-43e4-be21-ef9dc5d6bad6 and restoreVM
13035b7e-58af-48a8-992e-0c6fc2b2503d independentlyNotFound verified. Restore
524.51+command124.45+resident170.89ms; coldstart3207.95+resident600.68ms.
Builder168270.51ms/upload128588.93ms. These are mechanical, notagentreadytargets.

Exactcandidate zuse-base-bdb814bc4d32dd07c96a772eb67e5764-canary-57a92a90faaa2074,
SHA535699f0309f70c90e9ee6752aa91e42844d8bca7681049f02c37956b04b22ad staged
and CURRENTenv threepin/cohortkeysmerged; internalCodexobservationownerexplicit
onlyruntimeenv, deploymentguardfromComposeZUSE_ENVstaging. RootactivatedonAPIa7,
productionunchanged. Actualnewtask workspace_7c1af26f79164c858961babbf705796d
admitted3067.18ms (target300msFAIL; newbatch0025 notdeployed). Firstmessage
sentoncedefaultNone, noAPIretry. LiveWSSmodel/toolprobe session77269 running,
receipt .context/live-complete-codex-task-result.json pending. Do not delete or
restart because probeobservationtimeout; inspectexacttask/provider/nativehandles.

0025 actualfour-commandadmissionintegration source+PGtests nowgreen27migration
syntheticrehearsal, NOTcurrentstage26. AgentGHnowowns v2sharedhostproducer; UIowns
0026backend/guest/protocol; memoryowns safeRecreate future0027. No combinedCargo
untilmemorysourcecoherent. Frozen e192/a7 artifacts unaffectedbythese edits.

Root firstcompletepackage task result: workspace7c1af26f79164c858961babbf705796d
on exactVMb7280f71-48d3-4bf1-809d-f2878c8c51af reachedready/online, but native
agent errored BEFORE initialize response. Actual publicDocPart.error.message:
'harness protocol error: initialize: app-server exited before responding'.
Liveprobe77269 terminalFAIL, no assistant/toolframes beforeerrored. Followup
read-only snapshot verifiedoneuser/oneerrorassistant/twochats; snapshotOnlyok is
NOTflowpass. No modelresponse/tool/filetest success. Runtime healthy, no reallocation
or automaticpromptretry. Keep existingVMforboundeddiagnostics, notnewimageyet.
Memoryagent executing credential-free exacttwo-path appserverstartup diagnostic
handle70317 on sameVM; inspectresultbeforefurtherchanges. No customersecretslogged.

Parallel-source status: GH0025 integratedfourSQLcommand path actualPG/role/org
proofsgreen +27migrationsyntheticrehearsal (notstaging26). UIv2 takes0026
backend/guest/protocol; GHsharedCloudRPC/native+webproducer andqueueparity.
Memoryrecreation0027 firstmetadata5compileerrors identified/fixedsource, no
subsequentCargoyet; prioritize actualCodexstartupfix. Previewdeferred/unwired.

### GitHub scoped bearer and authority deadlines

GitHub's repository-scoped user token is a distinct upstream bearer. In the
staging private-clone diagnostic, GitHub issued a fresh eight-hour scoped token
while its parent user credential had about six and a half hours remaining. The
[scoped-token endpoint](https://docs.github.com/en/rest/apps/apps#create-a-scoped-access-token)
does not accept an expiry request or promise that the child expires with the parent.

Runtime Git grants therefore authenticate two different deadlines inside the JWE:
`providerExpiresAtMs` is the unmodified GitHub expiry;
`authorityExpiresAtMs` is the minimum of that expiry, the current parent expiry
(when present), and one hour after issue. The existing v1 `expiresAtMs` field is
an alias of the authority deadline, compatible with retained guests. It bounds
local authorization and cache admission, **not upstream bearer validity**. The
credential helper rechecks current actor/repository/runtime authority at most ten
seconds apart; revocation denies subsequent helper grants. A copied bearer can
remain valid at GitHub until its real expiry or provider revocation. No credentials
are placed in Git configuration, command arguments, disk snapshots or logs.

### Managed GitHub CLI credential boundary

The native Git panel invokes the guest's `GitHubCli`; an API-only PR test does
not verify this path. Managed Core instances now require a shared task launcher
before any GitHub subprocess can run. The board, PR resolver and RPC clones use
the same launcher, and cannot fall back to a configured desktop/guest account.
The launcher carries only a private executable path, not a token.

The runtime's private broker validates the actor, task generation and exact
repository on each credential boundary, with at most ten seconds of coalescing.
Its helper replaces itself with the actual `/usr/bin/gh` process, putting the
scoped token only in that child's `GH_TOKEN`. It overrides `GH_HOST` and
`GH_CONFIG_DIR`, removes inherited alternate tokens/debug settings, and refuses
auth login, token export and alternate hosts. GitHub documents these environment
variables in the [CLI manual](https://cli.github.com/manual/gh_help_environment).
No token enters `ProcessRequest` debug data, arguments, Git configuration or a
profile file. Managed account disk caches remain disabled for this source.

Agents, terminals and services receive the private helper directory through a
non-secret local PATH prefix. A managed `/usr/local/bin/gh` shim also resolves a
private metadata pointer, so login shells that reset PATH still use the broker.
The pointer and isolated CLI configuration are private, removed on owner cleanup
and excluded from reusable image preparation. The real GH package remains at
`/usr/bin/gh`; there is no PATH search or recursive wrapper fallback.

New builders install GH and smoke-test its real version. The optional
`rustGithubCliVersion: 1` capability requires an actual version/SHA-256 receipt
from the installed executable. Historical manifests keep the capability absent.
This source requires a new tested guest and managed base; the retained fe94
canary's API PR proof must not be described as native/agent CLI certification.

### Queue promotion and creation rollback checkpoint

Version-two delivery now has durable per-session drain jobs, direct bounded
postcommit dispatch (32 slots), and one bounded ordinary-worker recovery action.
The original author's scope is reauthorized before SQL locks; the final head,
revision, immutable author metadata, sharing policy, and edit gate are checked
again under the workspace lock. Expired edit leases remain explicit review gates.
Final Run sealing and queue claiming commit together. Referenced assets remain
typed and verified; relative file/skill references and annotations are preserved.
Unsupported `asGoal` execution leaves the encrypted row intact with a specific
error; it is not converted into an ordinary prompt silently.

SendNow seals an independent Interrupt and a priority Run atomically. The Run
waits for that exact control's admitted receipt and factual prior-turn termination.
A rejected/uncertain control cancels only a proven never-dispatched dependent;
it never invents cancellation of the already running model. SteerNow requires an
explicit source turn and rejects unsupported attachment steering. Existing queued
and leased commands retain their identities and are never replayed to reorder work.

New-task rollout is independent of ongoing delivery. Turning off new v2 creation
preserves prior idempotent receipts and existing persisted v2 delivery. Cached
catalog/column readiness prevents missing or partially installed optional schemas
from acknowledging work they cannot drain. Account events report error changes,
not reconciliation ticks; no database notifications or client polling were added.

Verification: combined API/guest metadata passed (34.27s); actual restricted-role
PostgreSQL initial admission/crypto/history plus queue contention, edit expiry,
priority/control dependencies, rollback, direct dispatch and partial-schema refusal
passed (one integrated fixture,0.65s). Current native GitHub race fixture passed
(1.46s), and five GH CLI tests passed including attached host flags and token-export
rejection. These are component/fixture results, not live provider or model latency
measurements. Migration0026 remains unlisted and v2 capability unadvertised pending
complete guest/core/assets/reconnect acceptance. No deployment was performed for
this checkpoint. Native installation-unlink UI and full organization/collaborator
release certification remain separate from the implemented scoped API action.

### Packaged command-admission and GitHub receipt checkpoint

The current source validates a provider-specific command-admission receipt against
the exact runtime binary SHA, storage/RPC formats, and session/progress versions.
New v2 computer admission also requires a verified managed GitHub shim receipt;
older manifests remain readable without acquiring these capabilities. The managed
shim proof checks root ownership, immutable executable permissions, exact script
bytes, and credential-free login-shell resolution to `/usr/local/bin/gh`.

The actual API and guest production-entrypoint build passed in 46.60s. The guest's
`runtime-capabilities` command ran with an empty environment and returned a receipt
matching its binary SHA `642c479b8daac31c6cdd961bcb623f64ff7d25e066335261a5247139cf5ca72e`.
The API SHA is `ef88877fd6d1b0d4c4dfbc35cef4766c4d70ee4b2b03a0db45694cbf34a471a8`.
Metadata proof is in `.context/cap2-gh-packaged-build-proof.json`. This verifies
the packaged entrypoint, not an installed provider base or live v2 operation.

The same source passed API/guest metadata, 83 cloud and 75 runtime unit tests,
restricted-role PostgreSQL queue/rollback/initial crypto/history, exact Stop and
unknown review, asset/control isolation, five managed-base cases, and the native
GitHub scoped-unlink race fixture. New v2 creation remains disabled; migrations
0026/0027 remain outside the deployment manifest. No Mac acceptance or new live
v2/provider/agent combination is claimed by these checks. Root owns immutable
artifact capture, staging deployment, and subsequent live certification.

### Retained-data structural rehearsal

The approved private legacy snapshot has now passed an actual isolated structural
rehearsal, beyond synthetic fixtures. All66 original tables are classified:42
retained with complete source-column/ciphertext hash checks and24 retained inactive
in quarantine. Constraints and owned-sequence last value/`is_called` are preserved;
no native ownership/jobs or active target writer were admitted. Source writes and
customer cutover were not performed, and temporary containers/data were independently
verified removed. Four unsupported procedural trigger definitions remain explicit
quarantine; object payloads and guest conversion remain separate release gates.

The private original-key compatibility probe failed to authenticate its retained
sample with either currently configured key. Original legacy sealing-key/version
availability is an external prerequisite; ciphertext retention does not certify
credential/history decryption. See [migration rehearsal](cloud-migration-rehearsal.md)
for the tooling boundary. Bootstrap's manifest now includes0026/0027 after0025,
without obsolete0022; this source fix does not mutate an existing database.

### Project cache foundation (source only, October 30)

Migration 0028 remains unlisted and unapplied. Environment saves atomically enqueue
one project preparation request with the immutable actor; background effect
intents, leases and original-operation reconciliation preserve previous ready
artifacts. Exact certified public-repository/SHA hits select a project artifact;
missing certification or cache retains the managed base. A retired cache can
fall back before any provider attempt, while uncertain allocations retain their
original source. Private cache admission remains disabled until actor-scoped
repository permission is verified before placing source on a machine.

Isolated PostgreSQL preparation test passed; schema/application-role/realtime
fixture passed (1.244s). API+guest metadata passed in 30.92s; runtime filesystem test
metadata passed in 1m29s. Offline source-bundle preparation and task-bound overlay
consumption passed four actual filesystem tests (1.70s). Missing0028 explicit-SHA
base admission passed under the actual restricted role (0.77s). Expanded cache
retirement/fallback checks need the corrected virtual-clock fixture rerun,
followed by first-import dirty-checkout and guest-capability hardening tests. No cache runtime/provider certification or
live cache support is implied. Actual source resolver/provider preparation
adapters, mutable-ref resolution and repository-managed refresh remain required.

### Current preparation UX verification (October 31)

Staging now runs the f01a5e46 API and worker with the existing Boxd managed base. A fresh isolated Codex task completed streamed output, tools, edits, shell checks and retained sleep/message wake; cleanup was independently confirmed. The canary required a controlled continuation after an operator response-shape error; this was not an uninterrupted driver pass. Single samples remain above the latency targets (2.605s admission,33.198s repository readiness,5.411s message wake). Production is unchanged.

The initial progress UI passed15 native and14 shared checks. A subsequent actual regression exposed a retired-subscription attachment gap after foreground admission. Its native wake policy is fixed; engine/shared attachment and final integration checks remain in progress. These client changes require a Mac rebuild, and visual acceptance is outstanding. Selected-credential/Git overlap passed focused runtime/backend verification but its new guest is not published. Do not infer all-provider or all-agent certification from these checks.


### Preparation UX integration checks completed (October 31)

The current native and shared UI passed43 focused checks:19 native,18 shared and6 engine checks. Send displays immediate activity through durable admission and the first response; preparation shows actual concurrent stages and next steps. Open Computer details follow stage events with coalesced log reads, and failures provide stage-specific guidance. Foreground admission passively replaces a retired connection while preserving the same view, transcript, draft and editor. Held queue input preserves sleep. Explicit Sleep retains the shared request bridge for a subsequent Send.

These are source and component integration results. Client rebuild and Mac/browser visual acceptance are still required. The f01 staging API is deployed; the new runtime with credential preparation overlapping Git is built and locally verified but awaits managed-base publication and live certification. The complete provider, recovery and migration acceptance matrix is not finished; production remains unchanged.

### Private project preparation integration (source only, November 1)

Changes after the immutable `d488` API artifact capture add actor-scoped private
source preparation and private first-allocation source resolution. The API uses
its existing selected-user repository authority; workers receive only a scoped
fetch credential in one isolated Git process environment. Original user/scope
epochs are checked again after source reads. Source chunks use authenticated
object encryption, and Boxd rechecks current authorization after object loading
and before source placement. Tokens, helper state and runtime credentials do not
enter source bundles or reusable images. Repository recipes retain priority over
the selected project recipe revision; task runtime credential renewal is separate.

The final source passed16 focused unit/Boxd wire checks and two explicit isolated
PostgreSQL tests, including private actor/user/epoch fences, first-import refusal
for archived workspaces, encrypted-object retry/tamper/scope checks and revocation
during loading with zero source placement or snapshot. The first new fixture
failure is retained: its legacy workspace was still archived, and the API correctly
refused it before the fixture explicitly entered provisioning. These18 passes are
component/fixture evidence. They do not certify live private Git fetch or caching.

Migration0028 remains unlisted and unapplied; the preparation feature and operator
certification are unchanged. These source changes are undeployed. Boxd has the only
preparation adapter. E2B, Boat and Daytona project-cache preparation remains
unsupported and separately capability-gated. Live miss/build/hit, failed-background
preparation retaining a compatible artifact, restored-image hygiene and bounded
cleanup remain release acceptance work. Current managed candidates do not become
verified cache bases through this source change.


### Project cache staging pilot (source only, November 1)

The post-`d488` source now includes an awaited preparation-worker CLI and
provider-keyed backend dispatch, with only Boxd preparation implemented. An
operator-owned staging pilot adds a separate cache namespace bound to the active
staging-base pin, personal actor, project, exact candidate/runtime/version,
recipe/source/dependency digests and expiry. Normal verified-base certification
and capability lookup remain strict. The pilot never certifies or promotes a base.

The rebuilt cloud test artifact passed18 unit/wire checks and three isolated
PostgreSQL cases. The pilot case exercised miss, capacity deferral before provider
intent, continuation of the same operation after deletion, prepared hit, failed
recipe preserving the compatible artifact, restored hit, and exact-resource
reconciliation/deletion after expiry or revocation. Seven lifecycle, nine grant
and one scoped-Git deadline regression also passed. The API awaited-worker path
passed Cargo checking. The optional migration operator passed a separate isolated
PostgreSQL test covering both strict admission and exact PREPARING pilot admission,
without promotion, including read-only original-intent inspection after expiry.
These are component/fixture results, separate from the earlier18 strict checks.

This source is undeployed; migration0028 remains disabled by default and was not
applied to staging by this work. Live miss/build/hit, restored-image hygiene,
failure fallback and cleanup still require the bounded Boxd rehearsal. Preparation
requires the distinct corrected guest runtime; earlier task-image acceptance does
not establish preparation support. Boat, Daytona and E2B cache preparation remain
unsupported and capability-gated.
