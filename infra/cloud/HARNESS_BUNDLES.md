# Cloud harness bundles

A harness bundle is one exact agent CLI version (for one agent and one
architecture) that cloud computers can adopt **without a new machine image**,
the same way they already move to new guest runtimes
([RUNTIME_RELEASES.md](RUNTIME_RELEASES.md)). A CLI patch is a new bundle; it
never requires rebuilding the managed base or anyone's machine image.

Contract: `crates/protocol/src/cloud_harness_updates.rs` (`SignedHarnessBundle`).
Tool: `infra/cloud/harness-bundle.py` (stdlib plus `openssl` and `zstd`).

```text
fetch (npm signature + SHA-512) ─▶ build-artifact (probe, tar, sha256, zstd)
  ─▶ sign ─▶ upload (R2, per GitHub environment)
  ─▶ register (operator DB role) ─▶ promote: staging ─▶ canary ─▶ stable
```

## What a bundle is

```json
{
  "bundle": {
    "sha256": "<sha256 of the uncompressed bundle tar>",
    "agent": "codex",
    "version": "0.165.0",
    "architecture": "x86_64",
    "executable": "codex-package/bin/codex",
    "adapter": "codex-app-server-v1",
    "stateFormat": "codex-sessions-v1",
    "readsStateFormats": ["codex-sessions-v1", "codex-claude-tested-v1"],
    "capabilities": []
  },
  "signature": "<base64 Ed25519 signature, or null before signing>",
  "artifactKey": "runtime/harness/codex/x86_64/<sha256>.tar.zst",
  "compressedSizeBytes": 41234567,
  "files": [{ "path": "codex-package/bin/codex", "sha256": "…", "bytes": 1, "executable": true }],
  "upstream": { "npmIntegrity": "sha512-…", "registrySigningKey": "…" }
}
```

- The signature covers `bundle` only, as `zuse-harness-bundle-v1\n` followed by
  compact JSON with keys in the order above. The domain line differs from the
  runtime's, so the **same release key** signs both without one ever being
  valid as the other. `crates/protocol/fixtures/harness-bundle-v1.json` pins
  the bytes; the Rust and Python tests both check it.
- The tar is deterministic (sorted members, root ownership, mtime 0, modes
  `0555`/`0444`/`0755`, regular files and directories only), so the same tree
  always has the same digest.
- `adapter` is the adapter contract the bundle was tested against. A runtime
  adopts only bundles whose contract it speaks
  (`cloud_harness_updates::adapter_for`). Bump the contract in the source when
  the adapter's expectations of a CLI change incompatibly.
- `stateFormat` is the session-state format the CLI writes;
  `readsStateFormats` lists what it can continue (always including
  `stateFormat`). A computer adopts a bundle only if it reads the format the
  current CLI wrote, so an incompatible CLI stays pinned until a continuation
  path is validated. The CLIs that came with existing machine images report the
  harness release's `stateFormat` (for example `codex-claude-tested-v1`): add it
  with `--reads-state-format` on the first bundle that should replace them.
- `files` and `upstream` are unsigned build evidence; the signed tar digest is
  what the computer checks.

## Fetch the exact upstream version

Never `latest`. `fetch` downloads one exact version from the CLI's official
npm distribution, verifies the registry signature and SHA-512 SRI (shared with
`fetch-harness-candidates.py`), checks it is a native Linux executable that
reports exactly that version, and lays it out as a bundle tree (`bin/<agent>`,
or the complete `codex-package/` tree with every companion executable).

```sh
python3 infra/cloud/harness-bundle.py fetch --agent claude --version 2.1.312 --arch x86_64 --out work
```

## Build, sign, upload

```sh
python3 infra/cloud/harness-bundle.py build-artifact --agent claude --version 2.1.312 --arch x86_64 \
  --tree work/tree --provenance work/provenance.json --out out \
  [--reads-state-format codex-claude-tested-v1] [--capability NAME]
python3 infra/cloud/harness-bundle.py sign --bundle out/bundle.json --key /secure/release-signing.pem
python3 infra/cloud/harness-bundle.py verify --bundle out/bundle.json
python3 infra/cloud/harness-bundle.py upload --bundle out/bundle.json --artifact out
```

`build-artifact` re-runs `--version` with an empty `HOME`, no credentials and
`DISABLE_AUTOUPDATER=1 DISABLE_UPDATES=1`. Upload requires a valid signature,
checks the artifact decompresses to `bundle.sha256`, uploads the artifact
before the manifest (`<sha256>.bundle.json` next to it) and never overwrites
an object. `download --agent A --sha256 S --out DIR` fetches, verifies and
extracts a published bundle for inspection.

## CI

`.github/workflows/cloud-harness-bundle.yml` is a manual `workflow_dispatch`
(agent, exact version, architecture, environment, optional extra state formats
and capabilities). It runs on the target architecture so the version probe can
execute the CLI, then fetches, packages, signs and uploads, and keeps
`bundle.json` as the workflow artifact `cloud-harness-bundle-<agent>-<sha256>`.
Without the signing key it packages only; without the R2 secrets it does not
upload. Pull requests that touch the tool run the offline tests.

It uses the same **environment secrets** as the runtime release workflow
(`ZUSE_RUNTIME_RELEASE_SIGNING_KEY`, `ZUSE_CLOUD_RUNTIME_R2_BUCKET`,
`ZUSE_CLOUD_RUNTIME_R2_ENDPOINT`, `ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID`,
`ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY`) in the `staging` and `production`
environments; no new secret is needed. Bundles live under `runtime/harness/`
in the same bucket, so the server's existing presigned-link settings cover them.

## Register (manual)

Registration needs the operator database role, so CI does not do it. Download
`bundle.json` from the workflow run, then:

```sh
ZUSE_CLOUD_RELEASE_DATABASE_URL=… zuse-cloud-api register-harness-bundle --bundle bundle.json --state staging
```

The server verifies the signature against the release key before it reads
the database URL, and again before it stores the bundle. A registered bundle
is immutable apart from its state; registering the same file again only moves
its state. Schema: `crates/cloud/migrations/0036_harness_bundles.sql`
(`zuse_rust_harness_bundles`; application roles can only read it).

## Promote

```text
candidate ─▶ staging ─▶ canary ─▶ stable ─▶ retired
```

```sh
zuse-cloud-api set-harness-bundle-state --sha256 <sha256> --state canary
zuse-cloud-api set-harness-bundle-state --sha256 <sha256> --state stable
```

| Deployment | `ZUSE_CLOUD_HARNESS_CHANNEL` | Offered states |
|---|---|---|
| staging | `staging` | staging, canary, stable |
| production | `stable` (default) | stable; plus canary for accounts in `ZUSE_CLOUD_HARNESS_CANARY_ACCOUNTS` (comma separated) |

Before `stable`, run the checks the harness release process already requires
on the **same bundle** (protocol fixtures, streaming, tools, Stop, resume,
selected-account identity, credential renewal, old conversation continuation)
on staging and the canary accounts. A failing candidate leaves stable
unchanged.

For each agent the server offers the newest bundle in the computer's channel
that (1) matches its architecture, (2) uses an adapter contract the runtime
speaks, (3) reads the session-state format the computer's current CLI wrote,
(4) has not failed twice on that computer, and (5) is newer than what it runs
(by version, then registration time; a CLI that came with the image never
yields to a bundle of the same version). Computers report their current CLI
on every poll and attest each adoption, so `zuse_rust_harness_bindings`
shows what every computer runs; `zuse_rust_harness_updates` holds the last
offer and its outcome.

## Roll back

Never delete objects. Retire the bad bundle:

```sh
zuse-cloud-api set-harness-bundle-state --sha256 <bad> --state retired
```

Computers running a retired bundle are offered the newest good bundle in
their channel, even an older one, as long as it can continue their session
state; they move at their next safe boundary like any other update. If no
compatible bundle exists, they stay on the retired one (a retired bundle is
never offered to anyone else). Rolling back does not reverse a session-state
change: a bundle that writes a new format must first ship as one that can read
it, and the previous bundle must list the new format in `readsStateFormats`
before it can be a rollback target.

## On the computer

Code: `crates/cloud-runtime/src/harness_updates/`. Runtimes advertise
`rust-harness-update-v1`; older runtimes never poll and keep their image's
CLIs.

```text
poll (every 10 min) ─▶ verify signature, agent, adapter, state format (before download)
  ─▶ wait: a turn finished, or 2 min idle since start/wake (never the first turn)
  ─▶ download (presigned, no bearer, no redirects) on a low-priority thread
  ─▶ zstd ─▶ sha256 == signed digest ─▶ extract (files/dirs only, safe paths)
  ─▶ `--version` with no credentials ─▶ read-only `<data>/harnesses/bundles/<sha256>/`
  ─▶ staged ─▶ adopt at a boundary ─▶ attest
```

- **Side by side.** Each version lives under its own digest. Adoption only
  changes which path *new* harness processes start from: Claude, Codex and
  Grok harnesses resolve their executable through the store at every spawn
  (`with_executable_source`), so a running process keeps its exact file and is
  never killed or replaced.
- **Boundary.** A staged bundle is adopted when no turn is running and every
  leased command has an acknowledged outcome, or at process start (before any
  harness process exists). Nothing downloads at start.
- **Compatibility.** The computer re-checks adapter contract and session-state
  format before download and again at adoption. Agents whose current CLI is
  unknown (no harness release in the image and no bundle yet) are not polled
  for, so a bundle never replaces a CLI whose session format is unknown.
- **Retention.** The active and previous bundles per agent are always kept;
  any other bundle is removed only once no live process (`/proc/<pid>/exe`)
  runs from it, so memory-suspended sessions keep their CLI. If the active
  bundle's files go missing, new processes fall back to the previous bundle,
  then to the image's CLI.
- **Updaters off.** Bundle-launched Claude processes get
  `DISABLE_AUTOUPDATER=1 DISABLE_UPDATES=1`; Grok runs with
  `--no-auto-update`; the Codex app server does not self-update.
- **Attestation.** Every adoption and refusal is reported once
  (`…/runtime/harness-updates/attestation`), and each poll reports the
  current CLI per agent, so the server always knows what each computer runs.
- The task user owns `<data>/harnesses`. Agents already run as that user, so
  integrity rests on the signed digest (the same reasoning as runtime updates
  on bases without root).

## Test

```sh
python3 infra/cloud/tests/harness-bundle.py
cargo test --locked -p zuse-protocol cloud_harness_updates
cargo test --locked -p zuse-cloud --lib harness
cargo test --locked -p zuse-providers --test executable_source
cargo test --locked -p zuse-cloud-runtime --lib harness_updates
# Registry, offers, attestation, channels and rollback under the restricted role:
ZUSE_CLOUD_TEST_DATABASE_URL=postgresql://… cargo test --locked -p zuse-cloud --lib \
  v2_initial_write_is_atomic_under_the_actual_restricted_role -- --ignored
```

The Python tests use throwaway keys, fake CLIs, a fake upstream and a local
fake bucket; they make no network requests. Real upstream downloads are only
exercised by the workflow.
