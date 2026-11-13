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

## Test

```sh
python3 infra/cloud/tests/harness-bundle.py
cargo test --locked -p zuse-protocol cloud_harness_updates
```

The Python tests use throwaway keys, fake CLIs, a fake upstream and a local
fake bucket; they make no network requests. Real upstream downloads are only
exercised by the workflow.
