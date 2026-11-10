# Managed base build and certification workflow

`managed-base-release.py` assembles an allowlisted build context and runs a bounded
operator/CI certification driver. It does not implement provider image import,
allocate on its own, register releases, or turn fixture output into live evidence.
Each provider has an independent release and report; a restricted Daytona account
does not block Boxd, Boat or E2B publication.

First build the Rust runtime image with `runtime.Dockerfile` on a dedicated builder,
push it and resolve its immutable registry digest. Stage the three verified,
self-contained native Linux harness executables with `harness-release.py` as
described in [HARNESS_RELEASES.md](HARNESS_RELEASES.md). These are upstream-verified
pins, not whatever happens to be installed in a developer HOME. Assembly accepts
only schema-2 releases and native ELF executables, rehashes all three, and copies
only those files plus their manifest. Extra files in the source tree are excluded.

```sh
python3 infra/cloud/managed-base-release.py context \
  --harness-root /build/harnesses --harness-release "$HARNESS_RELEASE" \
  --runtime-image "registry.example/zuse-runtime@sha256:$RUNTIME_IMAGE_SHA256" \
  --output /build/clean-context

docker build --file /build/clean-context/Dockerfile \
  --tag registry.example/zuse-base:candidate /build/clean-context
```

The resulting `candidate.json` records exact file digests, runtime image digest,
platform, adapter revision and harness pins. Its status is **candidate-only**.
The tree includes no task checkout, authority profiles or provider credentials.
The image layers the harnesses as root-owned read-only files and returns to guest
UID 10001. It does not run customer setup or use a live credential-bearing computer
as an account-wide base.

Import this image through the provider's supported image/template build workflow,
then run the managed guest `prepare-image` sanitation path. Retain the resulting
guest manifest and provider immutable artifact ID. Create an unregistered release
document matching `ManagedBaseRelease` (see
[managed base admission](../../internal-docs/cloud-managed-bases.md)). Building a
container, allocating a machine or probing `--version` does not certify an artifact.
Provider image-import drivers and live acceptance credentials remain deployment
prerequisites; they are not supplied by this runner.

## Driver contract

The executable driver is a trusted operator/CI boundary. It receives one JSON
request on stdin and emits at most 64 KiB of JSON on stdout. No shell command or
secret is passed as an argument. Its environment contains only PATH, TLS roots,
the selected provider's key (and Boxd organization/base URL), and explicitly named
`ZUSE_CERT_*` test configuration. HOME and other providers' keys are not inherited.
Never print credentials or raw tool output in the response. Diagnostics from the
driver are withheld from the release report.

A case request contains:

```json
{
  "schema": 1,
  "action": "case",
  "runId": "zuse-cert-unique-id",
  "provider": "boxd",
  "releaseSha256": "exact unregistered release document digest",
  "release": {"provider": "boxd", "artifactId": "exact artifact"},
  "maxResources": 1,
  "maxLifetimeSeconds": 1800,
  "agent": "codex",
  "case": "stream-tools-edit-test"
}
```

The driver must enforce the resource and lifetime bounds, tag every resource by
run ID **before** allocation, and reconcile original operations after uncertain
responses. It must use the exact artifact and selected test-agent connection.
Passing a case means observing its actual end-to-end behavior, never key presence,
fixture success or API health. The driver responds with the same `runId`,
`provider`, `releaseSha256`, `agent` and `case`, and `status: "passed"`, `"failed"`
or `"blocked"`. Only `"passed"` counts. Any error stops additional cases.

The runner covers eleven scenarios for each of Claude, Codex and Grok: prepared
and uncached creation; streamed inference/tools/edit/test; stop/follow-up; closed
app; reconnect without duplicate execution; retained sleep/resume; process loss
and credential renewal; terminal/files/previews; Git push/PR; recreation/deletion.
The driver owns scenario assertions and private detailed evidence. Milestone
latencies such as durable admission and computer readiness must be recorded
separately there; the runner's `elapsedMs` measures the whole scenario.

A `cleanup` request is always attempted, including after a failed or timed-out
case. It has the same run and release identity, and must enumerate/reconcile all
resources by run ID, including allocations whose response was lost. Respond with
`status: "passed"` and `remainingResources: 0` only after verified absence. Failed
or uncertain cleanup produces `cleanup-required` and forbids publication. The
report keeps the run ID so an operator can repeat cleanup safely. The runner kills
timed-out driver process groups and never retries a possibly executed case.

```sh
python3 infra/cloud/managed-base-release.py certify \
  --release /build/boxd-unregistered.json --driver /build/bin/certify-boxd \
  --timeout 300 --output /build/boxd-report.json

python3 infra/cloud/managed-base-release.py publication \
  --release /build/boxd-unregistered.json --report /build/boxd-report.json \
  --output /build/boxd-review-release.json
```

Publication requires exactly the complete passing matrix and verified cleanup,
bound to the exact artifact, capabilities, model catalogs, manifest and harness
hashes. It writes `certificationReportSha256` from the report bytes and yields a
**review-required** release. It neither signs an attestation nor authorizes a
provider integration. Keep both report and driver digest with the release.
Register only reviewed staging releases using the existing explicit operator DB
role command. Customer production admission remains a separate reviewed cutover.

The tests in `tests/managed-base-release.py` use synthetic drivers to verify
orchestration, report binding, cleanup-on-timeout and context isolation. They do
not certify any provider or agent.

## Native Boxd / Boat builders

The native path builds on one fresh provider VM instead of importing a Docker
image. `fetch-harness-candidates.py` accepts exact upstream versions, verifies the
npm registry ECDSA distribution signature and SHA-512 tarball integrity, extracts
only the native executable, then probes `--version` with an empty HOME. It never
runs npm lifecycle scripts. Grok's platform package ships its native executable as
`grok.br`; a trusted `--node` executable decompresses it through a bounded stream.
The resulting SHA-256 and registry signing-key ID are retained in provenance.
These are **candidate pins**, not compatibility approvals. `--resume` revalidates
the existing candidate digest and signed metadata before skipping a download.

```sh
python3 infra/cloud/fetch-harness-candidates.py \
  --claude-version "$CLAUDE_VERSION" --codex-version "$CODEX_VERSION" \
  --grok-version "$GROK_VERSION" --node /trusted/bin/node \
  --output /build/new-native-candidates
```

Stage that three-agent tree with `harness-release.py`. Then assemble an allowlisted
native bundle from the exact verified runtime binary and pinned harness release:

```sh
python3 infra/cloud/managed-base-release.py bundle \
  --harness-root /build/harnesses --harness-release "$HARNESS_RELEASE" \
  --runtime /build/zuse-cloud-runtime --runtime-sha256 "$RUNTIME_SHA256" \
  --output /build/native-bundle

# Validates the bundle and prints resource bounds; allocates nothing.
node infra/cloud/build-managed-base.mjs --provider boat \
  --bundle /build/native-bundle --output /build/boat-base.json

# One isolated no-environment builder, 30-minute provider lifetime.
node infra/cloud/build-managed-base.mjs --provider boat --live \
  --bundle /build/native-bundle --output /build/boat-base.json
```

Boxd requires the pinned official SDK 0.2.14 (`--boxd-sdk` points to its Node
entry). `--boxd-native-default` selects the provider's native VM and systemd boot
owner, matching the original publisher pinned at `a0155d2dd11f07495a99d3737c9bd40a5c8b4508`.
The driver verifies a real `systemd-run` round trip before uploading and records
the resolved provider image reference. This input is provider-managed; the final
snapshot must still pass artifact-bound certification before registration.
Alternatively, `--boxd-image repository@sha256:digest` selects the OCI primary-command
candidate path. These options are exclusive; successful exec alone does not certify
either boot mechanism. Boat uses
its provider-managed blank Ubuntu image with `noEnv: true`; the captured artifact
and sanitation manifest identify the resulting content. Both paths upload in
2-MiB pieces under `/tmp` (Boat's file API rejects `/opt`), then move the
assembled allowlist through command execution into the installation directory,
verify file digests, install tools/runtime under the dedicated zuse
user, execute the guest's credential-free `prepare-image`, and capture a new unique
provider snapshot. Boxd preparation stays in a bounded foreground exec because
its one-shot executor owns a separate PID namespace. Boat preparation uses the
documented tracked detached-command API, persisting its process identity before
bounded completion checks; lost process state never triggers command replay. Allocation is never mistaken for a prepared base.

The default live probe deletes both builder and snapshot afterward. Add
`--retain-artifact` only when intentionally creating a reusable candidate artifact.
Builders are always deleted and verified absent. Lost create responses are
reconciled by their original unique name, never allocated again; lost deletes are
inspected. Snapshot submission is never blindly retried. A cleanup ambiguity
produces `cleanup-required`, preserving the original machine/snapshot identities
in the private report. `prepared-uncertified` explicitly means no agent acceptance
matrix or customer admission has been certified. Run the separate certification
matrix and reviewed registration after preparing an artifact.

The provider APIs used here are documented in Boat's [saved snapshot API](https://docs.boat.dev/api/reference/snapshots/save-saved-snapshot),
[file upload API](https://docs.boat.dev/api/reference/agent/write-sandbox-file), and
[template guide](https://docs.boat.dev/snapshots/templates), and the official
[Boxd SDK 0.2.14](https://www.npmjs.com/package/@boxd-sh/sdk/v/0.2.14).
Boat may take minutes to save a live source; the driver waits for the saved artifact
itself to become ready. Boxd snapshots capture running machines and also complete
asynchronously. No task credentials are installed on either builder.


## Resident runtime supervision

New native bundles explicitly request `rustRuntimeSupervisorVersion: 1`. The guest
attests this only after checking a private, idle resident supervisor endpoint; a
legacy plan cannot acquire the capability merely by including a Python file.
Preparing a previously enrolled computer is rejected. Runtime/recovery identities,
control sockets and launch receipts are excluded from recovery archives.

Native Boxd and Boat bases enable a resident systemd unit. The alternative Boxd
OCI builder uses a primary command that waits for the installed supervisor, then
executes it with `restartPolicy: always`. SDK 0.2.14 snapshot restores reject
command/restart-policy overrides, so restoration must retain the captured primary
command. The resident owns runtime children; one-shot API execs only submit bounded,
private Unix-socket requests. `nohup`, `setsid` and double forks alone cannot make a
child survive its PID namespace init exiting ([Linux PID namespace semantics](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html)).

Launch requests bind workspace, runtime generation, storage incarnation and API
origin. Repeated operations acknowledge the existing child; conflicting operations
fail. Secrets stay in private token/config files, outside IPC receipts. Cold starts
reuse existing disk and session state; launching never fetches, rebases, renames a
branch or resends a prompt. Retained memory resumes notify the existing child using
the signed lifecycle operation. Recovery uses the same resident owner and fences.
Legacy provider exec startup requires an explicit provider-scoped persistent-exec
certification; Boxd never takes that fallback.

`probe-boxd-supervisor.mjs` is a separate mechanical candidate test. It builds one
isolated machine, deletes that source before restoring its temporary snapshot,
checks the captured systemd/primary-command owner, resident survival across separate
execs and a cold restart, verifies retained
work, and deletes the restored machine and snapshot. It allows at most one active
machine and never registers a managed base or certifies agent acceptance. Its
`--live` flag and exact bundle/SDK/OCI arguments are required to allocate resources.

## Owner-only staging candidates

An internal acceptance candidate is a different record from a certified release.
Migration `0020_staging_managed_base_candidates.sql` adds `candidate_manifest`;
staging inserts it with `state=preparing` and `runtime_manifest=NULL`. Ordinary
verified catalog queries and `register-managed-base` cannot promote that record.
Candidate JSON has runtime/harness/provenance digests and **no certification hash**.

First complete the mechanical probe using the exact final runtime bundle. The
Boxd probe may intentionally retain one snapshot only when
`--retain-staging-candidate` is supplied in a staging operator process, all
mechanical checks pass, and both test machines are verified
absent. Failure deletes the snapshot too. The report explicitly says
`uncertified-staging`; it is never agent acceptance evidence. A clean artifact
can be prepared before login with `candidateOwnerPending=true`; there is no fake
owner, admission or runtime credential. The later stage command and service pin
still require the actual freshly authenticated staging owner.

Generate the candidate document from the retained report and exact native pins.
`catalogs.json` contains a nonempty reviewed subset keyed `claude`, `codex`,
`grok`; start with Claude only when other accounts have not supplied their actual
model catalog yet. Never invent Codex/Grok IDs to populate a picker.
`capabilities.json` describes only the actual tested provider/size. These are
acceptance choices, not compatibility approvals. The tool rehashes every
allowlisted bundle file and matches all installed agent versions/digests.

```sh
python3 infra/cloud/managed-base-release.py candidate \
  --build-report /private/base.builder.json --proof /private/base.proof.json \
  --bundle /private/final-native-bundle --capabilities /private/capabilities.json \
  --catalogs /private/catalogs.json --output /private/staging-candidate.json

# Requires ZUSE_DEPLOYMENT_ENVIRONMENT=staging and the explicit operator DB URL.
# Output is the public-digest pin JSON, never a credential or verified release.
zuse-cloud-api stage-managed-base-candidate \
  --candidate /private/staging-candidate.json --owner ACCOUNT \
  --ttl-seconds 7200 --allow-staging-compute > /private/staging-pin.json
```

Install that JSON as `ZUSE_CLOUD_STAGING_BASE_PIN` on both staging API and lifecycle
worker, with `ZUSE_CLOUD_CREATE_ACCOUNTS` and `ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS` each
containing exactly that same single account. Startup rejects production, extra
accounts, malformed pins and lifetimes exceeding 24 hours. The optional explicit
staging compute allowance bypasses financial entitlement checks for that pinned
candidate only; admission allows one logical candidate computer until it is deleted.

By default only the pinned personal owner sees its candidate catalog/capabilities,
marked `stagingCandidate.status=uncertified`. Other scopes retain the certified
catalog. The explicit organization extension below does not change that default. Allocation, resumed compute and archive restoration check the
current owner/provider/release/artifact/manifest/harness/candidate pins and expiry;
retirement or tampering fails closed. Stopping/deleting test work remains possible.
A pin can be renewed against the same immutable candidate without rebuilding it.
Catalog metadata has its own content-derived candidate release ID; the original
`manifest.buildId`, snapshot, runtime and installed harness release remain unchanged.
After real Codex `model/list` or Grok ACP discovery, stage a new metadata document
and exact owner pin against that same clean artifact. Existing metadata rows are
immutable. Ordinary certified releases retain their stricter base-build identity.
Candidate expiry does not delete a shared artifact; physical cleanup must check
all candidate, certified-release and task references first.

Use this exact retained artifact for real account onboarding, streamed turns,
tools, terminal/files/previews, reconnect and lifecycle acceptance. Do not recreate
a different image and transfer the report. Promotion remains a separate reviewed
operation after complete artifact-bound certification; a healthy staging candidate
alone does not authorize a verified registry entry or customer rollout.

### Actor-bound organization acceptance

An organization canary is a separate explicit staging pin, not certified org
support. The artifact must already attest `rustOrganizationExecutionVersion: 1`
and contain the selected compatible harness/model. The operator may use:

```sh
ZUSE_DEPLOYMENT_ENVIRONMENT=staging \
ZUSE_CLOUD_RELEASE_DATABASE_URL=... \
zuse-cloud-api stage-managed-base-candidate \
  --candidate /private/staging-candidate.json \
  --owner 'organization:ORG_ID' --actor ACTOR_USER_ID \
  --ttl-seconds 7200 --allow-staging-compute > /private/org-staging-pin.json
```

The emitted pin contains immutable `actorId`. Creation cohort must be exactly
`organization:ORG_ID`; auth cohort must be exactly the two scopes
`organization:ORG_ID,ACTOR_USER_ID` (order does not matter). Extra accounts, missing
actor, production deployment, unpaired org runtime capability or expired/changed
artifacts fail closed. An ordinary personal pin omits `actorId` and retains its
previous single-owner cohort and wire format.

The task must have that exact initiating actor and existing native organization
provenance. Actual WorkOS membership, sharing policy and any GitHub-managed
membership/enrollment remain enforced by the normal authority before admission,
worker effects, credential delivery and ongoing access. A pin never supplies or
substitutes membership. Other organization members see allocation blocked for this
actor's canary. A member may select their **own** personal connection or an explicit
organization connection; credentials are not copied between owner records and an
installing admin's personal identity is never borrowed.

The exact allowance also applies to bounded encrypted attachment preparation;
production/paid checks stay intact outside the selected candidate. One logical
candidate computer is allowed for that pinned owner until deletion. Delete prior
isolated candidate computers before handing the rollout to another owner/actor;
a config handoff does not delete work or a machine automatically.

Organization API/GitHub/project write gates must be reviewed and explicitly scoped
before a live run. Registering a pin does not enable them. Use an owned isolated
staging organization; do not mutate another team's membership or billing for a
canary. The full fake-provider organization fixture runs both certified and
uncertified candidate modes, including personal connection selection, uploads,
bootstrap, lifecycle, actor departure and zero invented billing/certified records.
Those fixture results are separate from live organization/provider acceptance.

### Runtime repair and explicit recreation extensions

New bundles include the immutable `runtime-repair.py` and
`runtime-recreation.py` helpers alongside the resident supervisor. Installers
publish these before making the supervisor entrypoint visible. The guest records
`rustRuntimeRepairVersion: 1` or `rustRecreationVersion: 1` only when the private,
unbound, idle resident actually advertises that extension. Older snapshots keep
these fields absent; adding a server endpoint does not upgrade their capability.

Runtime repair retires the owned process tree (including detached and privileged
children), takes the native data lock, retains the SQLite/WAL and existing terminal
receipts, and quarantines interrupted commands as outcome-unknown before starting
the retained runtime. It does not fetch, rebase, rename a branch, replay a prompt,
or allocate another computer. The API separately waits for the signed runtime
resume acknowledgement. Loss of the resident itself during an unfinished repair
fails closed; it does not assume escaped processes stopped.

Explicit recreation applies only after a verified encrypted restore. Its private
fenced prepare step retains the old environment-ready receipt and forces the
recorded setup recipe to run on the replacement computer. Arbitrary unrecorded
system package changes are not part of the workspace archive. Private supervisor,
repair and recreation receipts are excluded from recovery archives.

These source capabilities and local subprocess/SQLite tests are separate from
live provider/agent acceptance. Existing staging candidate artifacts remain
immutable and must not be relabelled to include a newly added extension.
