# Managed cloud harnesses

To update one agent CLI on existing computers without a new machine image, use
signed harness bundles instead: [HARNESS_BUNDLES.md](HARNESS_BUNDLES.md). This
page covers the release tree baked into managed bases and the authority image.

`harness-release.py` is the offline release manager used by the Rust guest launcher.
It does not implement account onboarding. Existing device authentication stays at
the account authority; the Rust Codex adapter consumes sealed execution credentials.
The auth resolver is separate from the login ceremony so a future approved browser
integration can supply credentials without changing the session runner.

## Build and stage

Use a dedicated build machine. Obtain **native, self-contained** Linux executables
for exact Claude/Codex versions through their official distributions. Verify vendor
signatures/checksums before staging. Do not stage npm launchers whose node_modules
are outside the release, authenticated home directories, or a user's live computer.
The local SHA-256 manifest detects subsequent changes; it is not a substitute for
upstream signature verification.

```sh
python3 infra/cloud/harness-release.py --root /opt/zuse/harnesses stage \
  --claude /build/verified/claude --claude-version 2.1.312 \
  --codex /build/verified/codex --codex-version 0.165.0 \
  --grok /build/verified/grok --grok-version 1.0.13 \
  --adapter-revision "$(git rev-parse HEAD)" \
  --runtime-version 0.25.1 --state-format codex-claude-tested-v1
```

These versions are discovery examples, **not approved compatibility pins**. The
command verifies each executable's `--version` in a credential-free environment,
records its digest/platform/runtime/adapter/state format, and atomically publishes
a content-addressed directory. A failed or partial build never selects a channel.

The `Harness release candidates` GitHub workflow checks official npm package
metadata hourly and uploads a report. It never installs or automatically promotes
an upstream release. Scheduled workflows start running when merged to the default
branch. Discovery can be run locally with:

```sh
python3 scripts/check-harness-releases.py --output /tmp/harness-candidates.json
```

## Test and promote

Run the Rust protocol fixtures first:

```sh
cargo test --locked -p zuse-providers --lib codex::auth::tests
cargo test --locked -p zuse-providers --test codex
cargo test --locked -p zuse-providers --test claude
cargo test --locked -p zuse-cloud-runtime --lib grants::tests
python3 infra/cloud/tests/harness-release.py
```

Then test the **same release** on isolated Boxd, Boat, E2B and Daytona machines with supported
test credentials: real inference, streaming/tools, selected-account identity,
unauthorized renewal, Stop during renewal, thread resume and previous-version
conversation continuation. The existing provider benchmark verifies allocation
and wake, not those model/auth cases. Record the actual results, logs and their
release ID in a CI/operator-owned evidence file:

```json
{
  "release": "<64-character stage output>",
  "adapterRevision": "<exact 40-character adapter commit>",
  "stateFormat": "codex-claude-tested-v1",
  "checks": {
    "protocol": true, "streaming": true, "tools": true, "stop": true,
    "resume": true, "auth": true, "boxd": true, "boat": true,
    "grok": true, "e2b": true, "daytona": true
  }
}
```

Only record `true` after the respective check passes. Evidence is a trusted
operator/CI input, not a cryptographic attestation. Promotion validates the binding
and rehashes the executables before atomically updating the pointer:

```sh
python3 infra/cloud/harness-release.py --root /opt/zuse/harnesses promote \
  --channel staging --release "$RELEASE_ID" --evidence /build/results.json
# After staging/canary validation, use the same operation with --channel stable.
```

## Distribute and launch

Copy the complete release directory, its `manifest.json`, the selected channel
JSON and `.release-lock` into `/opt/zuse/harnesses` in a managed provider base, or
publish that same tree through a separately authenticated artifact deployment.
Directories must be readable/searchable and files readable by UID 10001. Treat the
release root as operator-owned and read-only to workspace users. Do not remove old
releases still referenced by suspended workspaces. Project dependency snapshots
and credentials have separate lifecycles.

Set `ZUSE_HARNESS_ROOT=/opt/zuse/harnesses` and
`ZUSE_HARNESS_CHANNEL=stable` (or staging). The guest launcher checks the compiled
runtime version and executes with absolute `CODEX_EXECUTABLE` and
`CLAUDE_CODE_EXECUTABLE` paths. Claude autonomous updates are disabled. The release
manager performs **no network requests or installation during launch**.

The first launch writes `cloud-harness-release.json` in the workspace data
directory. Resume reuses that exact release. Channel changes affect new computers;
active processes and suspended computers keep their executable. Missing/incompatible
pinned versions fail explicitly rather than silently switching. Existing images
without `ZUSE_HARNESS_ROOT` keep their current executable discovery behavior.

```sh
python3 infra/cloud/harness-release.py --root /opt/zuse/harnesses rollback --channel stable
```

Rollback selects the previous tested release for new computers and retains both
versions. It does not reverse a state migration. Moving an existing workspace to
a different release requires a validated state compatibility path and a stopped
runtime; automatic turn-boundary upgrades and fleet artifact delivery are not yet
implemented. Nothing in this workflow fetches/rebases the user's checkout.

Three-agent releases use manifest schema 2 and require Grok plus all four provider
checks in promotion evidence. Existing schema-1 Claude/Codex releases and computer
pins remain readable. Missing Grok in a schema-2 release fails explicitly. The guest
also receives an absolute `GROK_EXECUTABLE`; no runtime npm installation is needed.

## Authority image

`authority.Dockerfile` layers Python, the private auth driver and an operator-owned
promoted harness tree onto an immutable API image. Its build context contains only
`harnesses/`, `auth-driver.py`, `harness-release.py` and `launch-authority.sh`. Never
copy a developer HOME or authority profile into it. Build away from the small host:

```sh
docker build --file infra/cloud/authority.Dockerfile \
  --build-arg ZUSE_API_IMAGE=registry.example/zuse-api@sha256:EXACT_DIGEST \
  --tag registry.example/zuse-authority:EXACT_RELEASE /build/authority-context
```

Use that image for the API and auth-worker because the API also performs serialized
credential renewal. Set `ZUSE_AUTH_RUNTIME_VERSION` to the tested runtime version
and select the tested staging/stable harness channel. The launcher binds all three
authority binary paths before starting Rust. `auth-worker --check` validates the
installed tooling and database columns without claiming an operation or starting
a login. Source code and fixture tests do not certify actual provider registration.

## Credential implementation status

The existing authority remains the sole OAuth refresh writer. Codex's original
access-token envelope and generic API-key envelope are supported. Grants remain
workspace/key/account/generation bound and memory cached. Unauthorized renewal
bypasses the cache and concurrent requests coalesce; a failed renewal cannot fall
back to a cached rejected credential. App-server runs with the ephemeral credential
store; broker login errors and stderr never copy credentials into diagnostics.
The authority's rotating refresh token is never delivered to a workspace.

Local device auth remains available. Hosted subscription deployment still depends
on the supported commercial integration documented in
[the auth design](../../internal-docs/cloud-auth-and-harness-updates.md); implementing
legacy wire compatibility does not register or authorize a hosted OAuth client.
The new Rust authority has encrypted database records, isolated login/verification,
serialized refresh and runtime delivery behind an account cohort. The additive
schema and authority image are not deployed. Retained-account import, supported
hosted registration, browser sign-in and full live agent certification remain
outstanding. No production auth cutover has been performed.

## Independent provider promotion

A restricted provider need not block another provider's tested release. Use
`promote --provider boxd` (or boat/e2b/daytona) with evidence containing a matching
`"provider": "boxd"`. Protocol, streaming, tools, stop, resume and auth checks remain
mandatory; schema-2 also requires Grok. The selected provider's check must be true.
No other provider is treated as tested. Scoped receipts are retained separately at
`attestations/<release>/<provider>.json`, and channel metadata lists only the
providers actually certified. Re-promoting the same binary for another provider
retains both receipts and preserves the previous-release rollback pointer.

Without `--provider`, the existing full-provider evidence requirement remains.
Harness promotion selects tooling; each provider still needs its independent,
artifact-bound managed-base certification before customer admission. Native build
and candidate tooling is described in [MANAGED_BASE_RELEASES.md](MANAGED_BASE_RELEASES.md).

## Owner-only staging candidate authority

Before promoting a new harness release, staging can run its **exact immutable
candidate** so its owner can connect real agent accounts and collect certification
evidence. This avoids requiring successful inference before the authority can
start. Build the authority image with the complete candidate release tree and
`.release-lock`; a channel pointer is not required for this mode.

Only staging may configure:

```dotenv
ZUSE_AUTH_CANDIDATE_RELEASE=<exact 64-character content-addressed release ID>
ZUSE_AUTH_CANDIDATE_ACCOUNT_ID=<one staging owner account ID>
ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS=<the same single staging owner account ID>
ZUSE_AUTH_RUNTIME_VERSION=<exact compatible runtime version>
```

Compose injects `ZUSE_DEPLOYMENT_ENVIRONMENT` from its staging/production
configuration. The candidate launcher rejects production, wildcard/multi-account
auth cohorts and a compute cohort that extends beyond that owner. It fully
rehashes the candidate files, checks runtime compatibility and creates an ephemeral
exact release pin under the container's tmpfs. Missing/tampered candidates fail;
there is no channel or system-CLI fallback. Candidate startup does not promote a
channel, mark any provider supported or grant compute entitlement.

Default production startup retains the certified-channel path. Remove the
candidate variables after real staging evidence passes and promote the **same
bits** with the scoped receipt. Customer managed-base admission remains gated by
its independent provider artifact certification. Launcher tests use synthetic
executables and do not count as authority/provider acceptance evidence.

Complete Codex distributions
---------------------------

Recent Codex versions include required companion executables and resources. A
successful `codex --version` or account login does not certify tool execution.
`fetch-harness-candidates.py` verifies the npm distribution signature and SHA512
SRI before extracting the full vendor tree into `codex-assets/`. Its `assets.json`
records every companion's hash, size and executable permission, including the
code-mode host, bubblewrap, ripgrep, package metadata and bundled notices/licenses.

Pass `--codex-assets /candidate/codex-assets` to `harness-release.py stage` to
produce a new immutable release. The manifest pins both the primary executable
and all companion files. Runtime and auth executable selection uses the original
`codex-package/bin/codex` layout; the primary is hardlinked to avoid duplicating
its bytes. Existing releases without an asset map remain readable for pinned
legacy authentication; do not silently relabel them as tool-capable releases.

`harness-release.py --root /candidate/harnesses smoke-code-mode --release HASH`
starts the pinned code-mode host with an empty private HOME and no credentials,
opens a protocol session, evaluates local arithmetic and checks an intentional
exception. This invokes no model. The managed base installer repeats this check
before capture. Real streamed tool execution remains a separate acceptance gate.

New managed bundles carry `codex-assets.tar`. The native builder verifies the
archive's exact member set, regular-file types and per-file hashes before any
allocation. Installation repeats verification and rejects symlinks, hardlinks,
traversal and unlisted members. Changing any companion creates a new harness
release and base artifact; never patch a running computer or an existing release.
