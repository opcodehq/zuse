# Cloud runtime updates

Status: design proposal (2026-11-12). No code changes yet.

## Problem

A computer runs the guest runtime (`apps/cloud-runtime`, `crates/cloud-runtime`)
that was baked into its managed base (`cloud-managed-bases.md`). It keeps that
runtime for its whole life:

- **Sleep/wake.** Boxd memory sleep resumes the same runtime process (it waits
  parked, `crates/cloud-runtime/src/parking.rs`). Boat cold restores relaunch the
  same file from disk.
- **Archive/unarchive and recreation.** Both restore onto the computer's
  *pinned* release, not the newest one. Restore resolves its source with
  `managed_bases::retained_source(provider, managed_release_id)`
  (`crates/cloud/src/runtime/restore.rs` ~91-116). Recreation does the same
  (`crates/cloud/src/runtime/recreation.rs` `recreation_source`). The managed-base
  doc says it directly: "a restore never silently switches to the newest base".
- **Runtime repair** restarts the same binary (`cloud-runtime-repair.md`).

So today the only way to get a newer runtime is a new computer. Meanwhile the
server and app ship every day. The incident: a new app sent a new-style message
(files uploaded straight to the computer, per-message access mode) to an old
runtime. The runtime failed, the command became `outcome-unknown`, and the chat
locked. Short-term fixes are in flight in other PRs: capability names with server
gating, definitive guest rejections, and Retry/Discard. This doc covers the
structural fix: **update the runtime on an existing computer without changing
its machine image.**

## How the runtime is pinned today

| Fact | Where |
|---|---|
| Binary is `/usr/local/bin/zuse-cloud-runtime`. It is owned by root, has `nlink==1`, and is not group- or world-writable | `infra/cloud/managed-base-fresh-boot.py` `verify_image`, `protected_bytes` |
| Fresh boot checks five pins (runtime, supervisor, service unit, manifest, helper) from the release's `initialRuntimeBootstrap` | `crates/cloud/src/managed_bases/bootstrap.rs` `ImagePins` |
| The pins are also checked on Boat `retained-resume`, and the live child's `/proc/<pid>/exe` must be the same file as the installed path | `managed-base-fresh-boot.py` ~210 |
| The installed manifest's `commandAdmission.runtimeSha256` must equal the runtime pin | `bootstrap.rs` `InitialRuntimeBootstrap::validate` |
| The layout receipt and certification record the runtime SHA. Certification fails if it differs from the manifest | `infra/cloud/managed-base-install.sh` (layout), `crates/cloud/src/certification.rs` ~342, `certification/checks.sh` |
| User images and project caches must keep the base runtime byte-identical | `cloud-user-images.md`, `crates/cloud/src/project_caches/selection.rs` |
| The resident supervisor (`zuse-runtime-supervisor.service`) runs as **`User=zuse`**, not root, so it cannot replace the root-owned binary | `managed-base-install.sh` ~196 |
| Root helpers run only through provider exec, via the SHA-verified fixed-path loader | `infra/cloud/managed-base-fresh-boot-loader.py` |
| Prized has no root. The runtime is `/home/prized/.zuse/bin/zuse-cloud-runtime`, owned by the task user | `infra/cloud/prized-base/README.md` |
| Capabilities are sent at bootstrap (`identity.rs` `request()`) and at storage enrollment. Storage enrollment **rejects** any unknown name or more than 10 entries | `crates/cloud/src/runtime.rs` ~820-838 |
| Capability checks read the stored bootstrap/enrollment record. A retained computer never sends it again | `runtime.rs` `native_commands_supported` ~1612 |

Harness releases already have most of the release machinery we need: content-
addressed releases, `staging`/`stable` channels, promotion gated on evidence,
rollback, and per-workspace pins (`infra/cloud/HARNESS_RELEASES.md`). Like the
runtime, a harness is fixed at first launch, and the doc lists "automatic
turn-boundary upgrades" as not implemented. The runtime design below follows the
same model so the two can share code later.

## Proposal

The runtime release becomes a separate, server-pinned artifact. The base
supplies only the **initial** runtime. A computer moves to the pinned runtime
at an idle boundary.

### Runtime release registry

- New table `zuse_rust_runtime_releases`. The key is the runtime SHA-256. Columns:
  version, architecture, `storageFormat` read/write sets, capabilities,
  `minSupervisorVersion`, artifact key, signature, and state
  (`candidate|staging|stable|retired`). Operators register releases with a new
  `zuse-cloud-api register-runtime-release` that uses the same operator database
  role as `register-managed-base`. Application roles can only read.
- Channel pins per deployment (`staging`, `stable`) and a per-account canary
  override. The override reuses the staging-candidate pin shape (owner, cohort,
  expiry). A computer's target is `canary ?? channel`, and never older than the
  runtime it has now (except an explicit rollback, below).
- Per-computer record `request_config.runtimeRelease
  {sha256, version, previousSha256, appliedAt}`. It changes only when a signed
  post-swap receipt arrives. Fresh-boot and retained-resume `imagePins` use
  `runtimeRelease.sha256` instead of the base pin. The other four pins stay as
  the base's pins.

### Artifact and trust

- CI builds the runtime once. It uploads `runtime/<arch>/<sha256>.zst` to a
  dedicated R2 bucket and signs `{sha256, version, arch, storageFormats}` with an
  Ed25519 release key. The key lives in CI, separate from R2 and API
  credentials. Its public key ships in the base, next to the helper.
- Delivery: the API puts `{sha256, size, signature, url}` into the wake
  directive it already sends to the runtime. The URL is a short-lived presigned
  R2 GET. **Not through the API:** about 20 MB times the fleet should not pass
  through the API host. **Not a public URL:** presigned URLs let the server
  control rollout per computer and audit each download. Because of the signature,
  a leaked URL or a compromised bucket cannot install code.
- The guest checks both the digest the authority pinned and the release
  signature. A compromised API alone therefore cannot push an arbitrary binary.

### Layout and swap (root bases)

- Releases live in `/opt/zuse/runtime/releases/<sha256>/zuse-cloud-runtime`
  (root, 0555). `/usr/local/bin/zuse-cloud-runtime` stays a regular file. It is
  updated by `rename()` from a root-owned staged copy, so the existing
  `no_symlink`, `nlink==1`, owner and `samefile` checks still hold unchanged.
  The previous binary is kept in `/opt/zuse/runtime/previous`.
- Download: the runtime (`zuse`) downloads into `~/.zuse-data/runtime-staging/`
  in the background. It cannot install anything.
- Install: a new pinned root helper, `runtime-update.py`, runs through the
  existing loader by provider exec, the same way fresh boot does. It copies the
  staged file into a root-only directory, fsyncs, then hashes and verifies **the
  copy** (no TOCTOU on the task-writable file), decompresses, and runs
  `runtime-capabilities` (`crates/cloud-runtime/src/capabilities.rs`
  `probe_current`, 5 s cap). Only then does it rename. Its SHA becomes a sixth
  `ImagePins` entry, so the helper itself needs a base change, but only once.
- Journal: `/opt/zuse/runtime-update/journal.json`, phases
  `staged → swapped → probation → committed`. If the supervisor or helper starts
  and finds `swapped` or `probation` with no commit, it restores `previous` and
  reports a rollback.
- The manifest is **not** rewritten, because it is base evidence pinned by
  `manifestSha256`. Launch verification changes from "installed runtime ==
  manifest `commandAdmission.runtimeSha256`" to "installed runtime == the
  signed runtime release the authority pinned for this computer". The manifest
  still proves base sanitation.
- **Prized (no root):** use the same journal and rename in `R/bin`, done by the
  task user. Integrity comes from digest plus signature. There is no privilege
  boundary to keep, because agents already run as the user who owns the runtime.

### Base certification

Certification still certifies the base with its initial runtime (`checks.sh`
is unchanged). New: each runtime release is certified **against every
verified base it can land on**. The swap checks run on that base: install,
`runtime-capabilities`, bootstrap, one prompt, sleep/wake. The result is stored
on the release. A release can be pinned for a computer only if it is certified
for that computer's `managed_release_id` and the computer's supervisor version
is at least `minSupervisorVersion`. User images and project caches keep the
base pin for *building*. A computer started from them then updates like any
other computer.

## When to swap (latency)

Targets: fresh start p95 under 5 s (currently failing at 10-13 s,
`cloud-prepared-startup.md`). Memory resume under 1.5 s (Boxd provider resume
measured at 443-606 ms, `cloud-provider-measurements.md`).

Estimated cost of a blocking swap. All numbers are assumptions to measure in
phase 1:

| Step | Estimate | Assumption |
|---|---:|---|
| Download | 250-500 ms | ~60 MB stripped thin-LTO binary, ~20 MB zstd, R2 in-region at 50-100 MB/s plus TLS |
| Verify + decompress | ~100 ms | SHA-256 of 60 MB with hardware SHA, plus zstd |
| Capability probe + rename | 50-100 ms | `runtime-capabilities` subprocess |
| Restart runtime, reconcile storage, gateway connect | 300-600 ms | same as a repair relaunch |
| Agent cold start | 500-1500 ms | the parked agent is lost. Codex `initialize` plus paging in the ~300 MB harness (`runtime-supervisor.py` `warm_agent`) |
| **Total added** | **~1.2-2.8 s** | puts resume at 1.7-3.4 s, over the 1.5 s target |

**Recommendation: never block an ordinary wake.**

1. **Download in the background after wake.** The runtime gets the directive,
   waits until the first turn has been admitted, then downloads at low
   priority. This adds 0 ms to wake.
2. **Swap at the sleep boundary.** The swap runs inside the Pause operation,
   after quiescence (the same precondition as repair: no mailbox lease, no turn,
   no attempted command without a terminal ACK). The new runtime restarts,
   attests itself, parks, and then the provider hibernates. Sleep becomes about
   1-2 s longer. Users don't see sleep (the Boxd hibernate confirmation already
   takes about 15 s). The next wake is an ordinary memory wake on the new runtime.
3. **Swap for free on cold boots.** When memory is lost (Boat cold restore,
   restore, recreation), the runtime starts anyway. If a verified file is
   already staged, renaming before launch costs about 1 ms. Never download on
   this path.
4. **Demand update** only when a send needs a capability that the runtime lacks
   *and* the runtime is outside the compatibility window. The UI shows "Updating
   computer…". This is the only blocking path, about 1.2-2.8 s, and it is rare
   by design.

This is a refinement of "update on wake": if the swap happens on wake, every
update costs the user a cold agent. A swap at sleep costs them nothing. The
trade-off is that a computer runs one more session on its old runtime.
Capability gating covers that session.

## Compatibility window and gating

- The server and app support **the last 3 stable runtime releases or 30 days of
  releases, whichever is larger.** CI enforces this (below). A runtime outside
  the window gets a demand update or "Update computer".
- Capability gating stays as the safety net. The server picks a message shape
  from the stored capabilities. If a capability is missing, it uses the old
  shape or rejects *before admission* with a definitive error, never
  `outcome-unknown`.
- After a swap, the new runtime sends a signed **runtime attestation**
  `{sha256, version, capabilities}` with its resume ACK. The server replaces the
  stored capability set under the workspace lock. Without this, a retained
  computer keeps the capabilities from its bootstrap (`runtime.rs` ~1612).
- Capability parsing has to tolerate unknown names. Today `enroll_storage_tx`
  rejects them, so a newer runtime would fail against a rolled-back server.
  Ignore unknown names, raise the 10-entry cap, and keep validating format and
  length.

## Failure modes

| Failure | Behavior |
|---|---|
| Download fails or times out | Stay on the current runtime. Retry on the next wake with backoff. The session is not affected |
| Digest or signature mismatch | Delete the staged file, report `runtime_update_artifact_invalid`, never retry the same URL. If this repeats across the fleet, halt the channel |
| New runtime fails probe, bootstrap or attestation (20 s budget) | The helper restores `previous`, the supervisor relaunches, and the guest reports `runtime_update_rolled_back`. The server marks that release as bad for this computer. A fleet rollback rate above 1% halts the channel automatically |
| Power loss or crash mid-swap | `rename()` is atomic. On startup the journal rolls back from `swapped` or `probation` |
| Commands in flight | No swap unless quiescent. Pending ACKs defer the swap to the next boundary. A swap never creates `outcome-unknown` |
| Storage or identity format change (`zuse-rust-engine-v1`, `cloud-runtime-identity.json`) | Two-phase rule: release N adds *read* support, release N+1 may *write* the new format. Then rollback by one release is always safe. A release that cannot follow the rule is marked `rollbackBarrier`: it is only pinned after it is certified, and is fixed forward only |
| Operator rollback of a channel | New pins point to the previous release. Computers already on the bad release downgrade at their next boundary, only if no `rollbackBarrier` lies between the two releases. Otherwise the server sends "Update computer" |
| Base supervisor too old for the update protocol | Not eligible. Use "Update computer" |
| Memory-sleep receipts bound to the old process | The swap re-parks under the new process and reissues the parked receipt before the provider sleeps. **Open item:** confirm what `rust-memory-sleep-v1` binds |

## "Update computer" fallback

For computers that cannot update in place (no helper, old supervisor,
`rollbackBarrier`, repeated rollbacks), offer one-click **Update computer**
using recreation. **Recreation today restores onto the same release**, so it
needs a `targetRelease` parameter that selects the current verified base for
the provider. The archive format must be readable by the newer runtime. CI
checks this by restoring old archives. The existing `softwareNotice`
(`recreation.rs` `WORKLOAD_LIMIT`) already warns about software installed
outside setup. Unarchive keeps its pinned release, as the managed-base doc
requires, and then updates in place like any other wake.

## CI

- **Old-runtime matrix:** `.github/workflows/cloud.yml` already points the
  integration suite at a guest binary through `ZUSE_CLOUD_TEST_GUEST_BIN`. Add
  a matrix that downloads the previous 3 stable runtime releases from R2 by
  digest and runs the current server and protocol suite against each. It must
  also cover new-style messages, to prove they are gated and definitively
  rejected, never left unknown.
- **New runtime against the previous server**, to cover a server rollback.
- **Update path:** old runtime, then swap, then new runtime with a retained
  SQLite/WAL. A deliberately crashing candidate must roll back. A crash injected
  at each journal phase must recover.
- The app checks the protocol fixtures against the oldest runtime in the window.

## Rollout

1. **Server only.** Release registry, per-computer `runtimeRelease`, runtime
   attestation, tolerant capability parsing, the CI old-runtime matrix and wake
   timing breakdowns. No guest change.
2. **New base** with `runtime-update.py`, the journal, the releases directory
   and the sixth pin. No automatic updates. An operator triggers swaps on owner
   staging computers.
3. **Staging channel**, automatic sleep-boundary swaps, per-account canary on
   owner accounts.
4. **Production canary accounts**, then stable. Halt automatically on rollback
   rate or digest failures.
5. **Window enforcement**, demand update, and "Update computer" with
   `targetRelease`. Retire old bases once no computer is pinned to their
   runtime.

## What to measure

- Drift: computers by how many releases behind they are, and the oldest runtime
  in use.
- Update funnel: directive → downloaded → swapped → committed, with rollback
  rate and reasons.
- Timing: download, verify and swap-to-attested (ms), and extra Pause duration.
- Wake p50/p95 (`rustLastWake.ms`) with and without a swap. It must not regress.
- Demand updates per day, gated sends, and `outcome-unknown` caused by version
  skew (target 0).
