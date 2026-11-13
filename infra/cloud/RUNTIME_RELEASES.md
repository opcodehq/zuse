# Cloud runtime releases

`runtime-release.py` packages the guest runtime (`zuse-cloud-runtime`) as a
signed, content-addressed release that existing cloud computers can move to
without a new base image. Design: [cloud runtime updates](../../internal-docs/cloud-runtime-updates.md).
Contract: `crates/protocol/src/cloud_runtime_updates.rs` (`SignedRelease`).

The tool uses only the Python standard library plus the `openssl` and `zstd`
command line tools. It never registers a release with the server and never
changes which release a computer runs.

## What a release is

```json
{
  "release": {
    "sha256": "<sha256 of the uncompressed binary>",
    "version": "0.26.0+g66fab12b1234",
    "architecture": "x86_64",
    "storageFormats": ["zuse-rust-engine-v1"]
  },
  "signature": "<base64 Ed25519 signature, or null before signing>",
  "artifactKey": "runtime/x86_64/<sha256>.zst",
  "compressedSizeBytes": 21474836,
  "capabilities": { "...": "exact output of `zuse-cloud-runtime runtime-capabilities`" }
}
```

- The signature covers `release` only, as `zuse-runtime-release-v1\n` followed
  by compact JSON with keys in the order above (`SignedRelease::signing_bytes`).
  `crates/protocol/fixtures/runtime-release-v1.json` pins these bytes; the Rust
  and Python tests both check it.
- `capabilities` is the guest's command admission receipt (a JSON object). The
  build refuses a binary whose receipt names another digest or a storage
  format missing from `storageFormats`.
- In the bucket, the artifact lives at `runtime/<arch>/<sha256>.zst` and the
  signed manifest next to it at `runtime/<arch>/<sha256>.release.json`. Objects
  are never overwritten.

## Build

Build the runtime the same way bases get it: `cargo build --locked --release
-p zuse-cloud-guest` inside `rust:1.99-bookworm` (the build stage of
`runtime.Dockerfile`, target `x86_64-unknown-linux-gnu`, glibc from bookworm).
Building on a newer distribution can link against a glibc the bases lack.

```sh
python3 infra/cloud/runtime-release.py build-artifact \
  --binary target/release/zuse-cloud-runtime \
  --version "0.26.0+g$(git rev-parse --short=12 HEAD)" --arch x86_64 --out out
```

Add `--storage-format NAME` (repeatable) only when a release can read more than
the default `zuse-rust-engine-v1`. Follow the design's two-phase rule: a
release that writes a new format must first ship as one that can read it.

## Sign

The private key (PEM, Ed25519) lives only in the CI secret
`ZUSE_RUNTIME_RELEASE_SIGNING_KEY`. Its public half is
`RELEASE_SIGNING_PUBLIC_KEY` in the contract. `sign` refuses any other key, then
verifies its own signature.

```sh
python3 infra/cloud/runtime-release.py sign --release out/release.json --key /secure/release-signing.pem
python3 infra/cloud/runtime-release.py verify --release out/release.json
```

## Upload

```sh
export ZUSE_CLOUD_RUNTIME_R2_BUCKET=... ZUSE_CLOUD_RUNTIME_R2_ENDPOINT=https://<account>.r2.cloudflarestorage.com
export ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID=... ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY=...
python3 infra/cloud/runtime-release.py upload --release out/release.json --artifact out
```

Upload requires a valid signature and checks that the artifact decompresses to
`release.sha256`. It uploads the artifact first, then the manifest. Rerunning
it is safe: objects that already exist with the same size are skipped, and an
object with the same key but a different size is an error.

## CI

`.github/workflows/cloud-runtime-release.yml` runs on pushes to `main` that
touch the runtime or protocol, and on manual dispatch. It builds in
`rust:1.99-bookworm` (and fails if `runtime.Dockerfile` moves to another
builder image), packages, signs, uploads, and keeps `release.json` as the
workflow artifact `cloud-runtime-release-<sha256>`. Without the signing key it
packages only; without the R2 secrets it does not upload.

Repository secrets:

| Secret | Used by |
| --- | --- |
| `ZUSE_RUNTIME_RELEASE_SIGNING_KEY` | release workflow, signing (Ed25519 PEM) |
| `ZUSE_CLOUD_RUNTIME_R2_BUCKET`, `ZUSE_CLOUD_RUNTIME_R2_ENDPOINT` | release workflow and old-runtime matrix |
| `ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID`, `ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY` | release workflow (write) |
| `ZUSE_CLOUD_RUNTIME_R2_READ_ACCESS_KEY_ID`, `ZUSE_CLOUD_RUNTIME_R2_READ_SECRET_ACCESS_KEY` | old-runtime matrix (read only) |

Keep the R2 write token scoped to the runtime bucket and separate from the
API's R2 credentials. Pull request jobs only ever get the read token.

## Register (manual)

Registration needs the operator database role, so CI does not do it. Download
`release.json` from the workflow run, then:

```sh
zuse-cloud-api register-runtime-release --release release.json --state staging
```

The server verifies the signature again before it stores the release.

## Promote

Certify the release against every verified base it can land on (install,
`runtime-capabilities`, bootstrap, one prompt, sleep and wake; see "Base
certification" in the design). Then promote it in the registry from `staging`
to `stable`. Only certified releases can be pinned for a computer.

After a release reaches `stable`, add it to the top of
`infra/cloud/runtime-compat.json` and drop the oldest entry beyond three:

```json
{ "releases": [{ "sha256": "<sha256>", "version": "0.26.0+g66fab12b1234", "architecture": "x86_64" }] }
```

The `old-runtime` job in `.github/workflows/cloud.yml` then downloads each
listed runtime (checking signature and digest) and runs the current server
suite against it through `ZUSE_CLOUD_TEST_GUEST_BIN`. It is `continue-on-error`
until the list is populated and green; make it blocking after that.

## Roll back

Rolling back never deletes objects. Take the bad release out of `stable` in the
registry and pin the previous stable release for new and existing computers;
computers move back through the same signed update path. Keep every release
that any computer may still run, including those in `runtime-compat.json`.

## Test

```sh
python3 infra/cloud/tests/runtime-release.py
cargo test --locked -p zuse-protocol cloud_runtime_updates
```

The Python tests use throwaway keys and a local fake bucket. They make no
network requests.

## Capability names

`release.json` carries two capability views:

- `capabilities`: the command-admission receipt object from `runtime-capabilities` (what `runtime-update.py` re-probes on install).
- `bootstrapCapabilities`: the capability **names** the runtime advertises at bootstrap, from `bootstrap-capabilities`. The server uses these to decide whether updating a computer to this release gives it a feature a waiting message needs. A runtime without the `bootstrap-capabilities` subcommand predates in-place updates and cannot be released through this pipeline.

## GitHub environments (staging and production)

The release workflow runs in a GitHub environment: pushes to `main` publish to
**staging**; production is a manual `workflow_dispatch` with `environment: production`.
Each environment holds the same secret names with its own values:

| Secret | Purpose |
|---|---|
| `ZUSE_RUNTIME_RELEASE_SIGNING_KEY` | Ed25519 private key (PEM) matching `RELEASE_SIGNING_PUBLIC_KEY`. One key signs both channels, because the public key is compiled into runtimes and installed in bases. |
| `ZUSE_CLOUD_RUNTIME_R2_BUCKET`, `ZUSE_CLOUD_RUNTIME_R2_ENDPOINT` | That environment's release bucket. |
| `ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID`, `ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY` | Write token, used only by the release job. |
| `ZUSE_CLOUD_RUNTIME_R2_READ_ACCESS_KEY_ID`, `ZUSE_CLOUD_RUNTIME_R2_READ_SECRET_ACCESS_KEY` | Read-only token (staging), used by the old-runtime CI matrix. |

The server reads `ZUSE_CLOUD_RUNTIME_R2_*` for presigned downloads and falls back
to `CLOUD_TRANSCRIPT_R2_*` with the `runtime/` prefix when they are unset.
