#!/usr/bin/env python3
"""Fetch, build, sign, upload and download signed agent CLI harness bundles.

Contract: `crates/protocol/src/cloud_harness_updates.rs` (`SignedHarnessBundle`).
Operator steps: `infra/cloud/HARNESS_BUNDLES.md`.

A bundle is one exact upstream CLI version for one agent and architecture:
a deterministic tar of the CLI tree, compressed with zstd, whose SHA-256 and
compatibility metadata are Ed25519-signed with the runtime release key under
the `zuse-harness-bundle-v1` domain. Signing, verification and R2 transport
are shared with `runtime-release.py`; upstream integrity checks (npm registry
signature and SHA-512 SRI) are shared with `fetch-harness-candidates.py`.

Stdlib only, plus the `openssl` and `zstd` command line tools.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runtime = _load("runtime_release", "runtime-release.py")
ReleaseError = runtime.ReleaseError

SIGNING_DOMAIN = b"zuse-harness-bundle-v1\n"
SIGNED_KEYS = ("sha256", "agent", "version", "architecture", "executable", "adapter",
               "stateFormat", "readsStateFormats", "capabilities")
AGENTS = ("claude", "codex", "grok")
# Must equal `adapter_for` in the Rust contract (checked by tests).
ADAPTERS = {"claude": "claude-stream-json-v1", "codex": "codex-app-server-v1", "grok": "acp-v1"}
# Default executable path inside a bundle tree built by `fetch`.
EXECUTABLES = {"claude": "bin/claude", "codex": "codex-package/bin/codex", "grok": "bin/grok"}
ARCHITECTURES = runtime.ARCHITECTURES
PLATFORMS = {"x86_64": "linux-x64", "aarch64": "linux-arm64"}
DIGEST = runtime.DIGEST
VERSION = runtime.VERSION
NAME = re.compile(r"[a-z0-9][a-z0-9._-]{0,63}\Z")
PART = re.compile(r"[A-Za-z0-9._-]+\Z")
MAX_FILES = 512
MAX_FILE_BYTES = 512 * 1024 * 1024
MAX_TREE_BYTES = 1024 * 1024 * 1024
PROBE_TIMEOUT_SECONDS = 20


# ----------------------------------------------------------------- bundle ---


def path_valid(value):
    return (isinstance(value, str) and 0 < len(value) <= 256
            and all(part not in (".", "..") and PART.match(part) for part in value.split("/")))


def validate_bundle(bundle):
    """Mirrors `SignedHarnessBundle::validate` in the Rust contract."""
    if not isinstance(bundle, dict) or set(bundle) != set(SIGNED_KEYS):
        raise ReleaseError("bundle must have exactly " + ", ".join(SIGNED_KEYS))
    if not isinstance(bundle["sha256"], str) or not DIGEST.match(bundle["sha256"]):
        raise ReleaseError("bundle.sha256 must be 64 lowercase hex characters")
    if bundle["agent"] not in AGENTS:
        raise ReleaseError("bundle.agent must be one of " + ", ".join(AGENTS))
    if not isinstance(bundle["version"], str) or not VERSION.match(bundle["version"]):
        raise ReleaseError("bundle.version must be an exact version like 1.2.3")
    if bundle["architecture"] not in ARCHITECTURES:
        raise ReleaseError("bundle.architecture must be one of " + ", ".join(ARCHITECTURES))
    if not path_valid(bundle["executable"]):
        raise ReleaseError("bundle.executable must be a safe relative path")
    for key in ("adapter", "stateFormat"):
        if not isinstance(bundle[key], str) or not NAME.match(bundle[key]):
            raise ReleaseError(f"bundle.{key} must be a lowercase name")
    reads = bundle["readsStateFormats"]
    if (not isinstance(reads, list) or not 0 < len(reads) <= 16
            or not all(isinstance(f, str) and NAME.match(f) for f in reads)
            or bundle["stateFormat"] not in reads):
        raise ReleaseError("bundle.readsStateFormats must list lowercase names including stateFormat")
    caps = bundle["capabilities"]
    if not isinstance(caps, list) or len(caps) > 32 or not all(isinstance(c, str) and NAME.match(c) for c in caps):
        raise ReleaseError("bundle.capabilities must be at most 32 lowercase names")
    return bundle


def signing_bytes(bundle):
    """Byte-identical to Rust `SignedHarnessBundle::signing_bytes()`."""
    validate_bundle(bundle)
    ordered = {key: bundle[key] for key in SIGNED_KEYS}
    return SIGNING_DOMAIN + json.dumps(ordered, separators=(",", ":"), ensure_ascii=False).encode()


def artifact_key(bundle):
    return f"runtime/harness/{bundle['agent']}/{bundle['architecture']}/{bundle['sha256']}.tar.zst"


def manifest_key(bundle):
    return f"runtime/harness/{bundle['agent']}/{bundle['architecture']}/{bundle['sha256']}.bundle.json"


def load_manifest(path):
    manifest = runtime.read_json(path)
    if not isinstance(manifest, dict) or "bundle" not in manifest:
        raise ReleaseError(f"{path} is not a harness bundle manifest")
    validate_bundle(manifest["bundle"])
    if manifest.get("artifactKey") != artifact_key(manifest["bundle"]):
        raise ReleaseError("artifactKey does not match the bundle")
    return manifest


# ------------------------------------------------------------------ fetch ---


def fetch(agent, version, architecture, out, node=None):
    """Download one exact version from its official npm distribution, verifying
    the registry signature and SHA-512 integrity, and lay it out as a bundle
    tree under `<out>/tree`. Never resolves `latest`."""
    if agent not in AGENTS or not VERSION.match(version) or architecture not in ARCHITECTURES:
        raise ReleaseError("exact agent, version and architecture required")
    upstream = _load("fetch_harness_candidates", "fetch-harness-candidates.py")
    out = Path(out)
    if out.exists() and any(out.iterdir()):
        raise ReleaseError(f"{out} must be a new or empty directory")
    work = out / "upstream"
    work.mkdir(parents=True, mode=0o700)
    keys = json.loads(upstream.get("https://registry.npmjs.org/-/npm/v1/keys"))["keys"]
    provenance = upstream.fetch(agent, version, PLATFORMS[architecture], work, keys, node)
    tree = out / "tree"
    executable = tree / EXECUTABLES[agent]
    executable.parent.mkdir(parents=True)
    shutil.copyfile(work / agent, executable)
    executable.chmod(0o555)
    for name in (provenance.get("assets") or {}):
        target = tree / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(work / "codex-assets" / name, target)
        target.chmod(0o555 if provenance["assets"][name]["executable"] else 0o444)
    runtime.write_json(out / "provenance.json", provenance)
    return tree, provenance


# ------------------------------------------------------------------ build ---


def tree_files(tree):
    """Regular files and directories only; returns sorted (relative, path, is_dir)."""
    tree = Path(tree)
    if tree.is_symlink() or not tree.is_dir():
        raise ReleaseError(f"{tree} is not a directory")
    entries, total = [], 0
    for root, dirs, files in os.walk(tree):
        for name in dirs + files:
            path = Path(root) / name
            relative = path.relative_to(tree).as_posix()
            mode = path.lstat().st_mode
            if not path_valid(relative):
                raise ReleaseError(f"unsafe path in bundle tree: {relative!r}")
            if stat.S_ISDIR(mode):
                entries.append((relative, path, True))
            elif stat.S_ISREG(mode):
                size = path.stat().st_size
                if size > MAX_FILE_BYTES:
                    raise ReleaseError(f"{relative} is larger than {MAX_FILE_BYTES} bytes")
                total += size
                entries.append((relative, path, False))
            else:
                raise ReleaseError(f"{relative} is not a regular file or directory")
    if sum(1 for e in entries if not e[2]) > MAX_FILES or total > MAX_TREE_BYTES:
        raise ReleaseError("bundle tree is too large")
    return sorted(entries)


def write_tar(tree, output):
    """Deterministic: sorted members, root ownership, mtime 0, fixed modes."""
    files = []
    with open(output, "wb") as raw, tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for relative, path, is_dir in tree_files(tree):
            info = tarfile.TarInfo(relative)
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mtime = 0
            if is_dir:
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                archive.addfile(info)
                continue
            executable = os.access(path, os.X_OK)
            info.size = path.stat().st_size
            info.mode = 0o555 if executable else 0o444
            with open(path, "rb") as data:
                archive.addfile(info, data)
            files.append({"path": relative, "sha256": runtime.sha256_file(path), "bytes": info.size,
                          "executable": executable})
    return files


def probe_version(executable, version):
    """`--version` in a credential-free environment with updates disabled."""
    with tempfile.TemporaryDirectory() as home:
        try:
            result = subprocess.run([str(executable), "--version"], capture_output=True, timeout=PROBE_TIMEOUT_SECONDS,
                                    stdin=subprocess.DEVNULL, check=False,
                                    env={"PATH": "/usr/bin:/bin", "HOME": home,
                                         "DISABLE_AUTOUPDATER": "1", "DISABLE_UPDATES": "1"})
        except subprocess.TimeoutExpired:
            raise ReleaseError("--version probe timed out") from None
    if result.returncode != 0:
        raise ReleaseError(f"--version probe exited {result.returncode}")
    reported = result.stdout.decode(errors="replace")
    if not re.search(r"(?<![\w.])" + re.escape(version) + r"(?![\w.+-])", reported):
        raise ReleaseError("--version probe does not report the exact version")


def build_artifact(agent, version, architecture, tree, out, executable=None, adapter=None, state_format=None,
                   reads_state_formats=None, capabilities=None, provenance=None, probe=True):
    tree = Path(tree)
    executable = executable or EXECUTABLES.get(agent, "")
    state_format = state_format or f"{agent}-sessions-v1"
    reads = list(dict.fromkeys([state_format] + list(reads_state_formats or [])))
    bundle = {"sha256": "0" * 64, "agent": agent, "version": version, "architecture": architecture,
              "executable": executable, "adapter": adapter or ADAPTERS.get(agent, ""),
              "stateFormat": state_format, "readsStateFormats": reads,
              "capabilities": sorted(set(capabilities or []))}
    validate_bundle(bundle)
    entry = tree / executable
    if entry.is_symlink() or not entry.is_file() or not os.access(entry, os.X_OK):
        raise ReleaseError(f"{executable} is not an executable file in the bundle tree")
    if probe:
        probe_version(entry, version)
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=out) as directory:
        uncompressed = Path(directory) / "bundle.tar"
        files = write_tar(tree, uncompressed)
        digest = runtime.sha256_file(uncompressed)
        bundle["sha256"] = digest
        compressed = out / f"{digest}.tar.zst"
        temporary = out / f".{digest}.tar.zst.tmp"
        subprocess.run(["zstd", "-q", "-19", "-T0", "-f", "-o", str(temporary), str(uncompressed)], check=True)
    if runtime.decompressed_sha256(temporary) != digest:
        raise ReleaseError("compressed artifact does not decompress to the bundle tar")
    os.replace(temporary, compressed)
    manifest = {
        "bundle": validate_bundle(bundle),
        "signature": None,
        "artifactKey": artifact_key(bundle),
        "compressedSizeBytes": compressed.stat().st_size,
        "files": files,
        "upstream": provenance,
    }
    runtime.write_json(out / "bundle.json", manifest)
    return manifest


# ---------------------------------------------------------------- signing ---


def sign_manifest(path, private_key_pem, expected_public_key):
    manifest = load_manifest(path)
    if runtime.public_key_of(private_key_pem) != expected_public_key:
        raise ReleaseError("signing key does not match the expected release public key")
    data = signing_bytes(manifest["bundle"])
    manifest["signature"] = runtime.sign_bytes(data, private_key_pem)
    if not runtime.verify_bytes(data, manifest["signature"], expected_public_key):
        raise ReleaseError("fresh signature failed verification")
    runtime.write_json(path, manifest)
    return manifest


def verify_manifest(manifest, public_key):
    if not isinstance(manifest.get("signature"), str):
        raise ReleaseError("bundle is not signed")
    if not runtime.verify_bytes(signing_bytes(manifest["bundle"]), manifest["signature"], public_key):
        raise ReleaseError("bundle signature is invalid")


# --------------------------------------------------------------- transfer ---


def upload(path, artifact_dir, public_key):
    manifest = load_manifest(path)
    verify_manifest(manifest, public_key)
    bundle = manifest["bundle"]
    artifact = Path(artifact_dir) / f"{bundle['sha256']}.tar.zst"
    if artifact.stat().st_size != manifest["compressedSizeBytes"]:
        raise ReleaseError("artifact size does not match compressedSizeBytes")
    if runtime.decompressed_sha256(artifact) != bundle["sha256"]:
        raise ReleaseError("artifact does not decompress to bundle.sha256")
    config = runtime.r2_config()
    # Artifact first: a published manifest always points at an existing object.
    return {
        manifest["artifactKey"]: runtime.put_object(config, manifest["artifactKey"], artifact, "application/zstd"),
        manifest_key(bundle): runtime.put_object(config, manifest_key(bundle), path, "application/json"),
    }


def safe_extract(tar_path, destination, bundle):
    """Extract a verified bundle tar: regular files and directories only."""
    destination = Path(destination)
    with tarfile.open(tar_path, "r:") as archive:
        members = archive.getmembers()
        if len(members) > MAX_FILES * 2:
            raise ReleaseError("bundle has too many members")
        for member in members:
            if not path_valid(member.name) or not (member.isfile() or member.isdir()):
                raise ReleaseError(f"unsafe bundle member {member.name!r}")
            target = destination / member.name
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as source, open(target, "xb") as output:
                shutil.copyfileobj(source, output, 1024 * 1024)
            target.chmod(0o555 if member.mode & 0o111 else 0o444)
    entry = destination / bundle["executable"]
    if not entry.is_file() or not os.access(entry, os.X_OK):
        raise ReleaseError("bundle executable is missing after extraction")
    return entry


def download(agent, sha256, architecture, out, public_key):
    """Fetch a published bundle by digest, check signature and digest, extract to `out`."""
    if agent not in AGENTS or not DIGEST.match(sha256) or architecture not in ARCHITECTURES:
        raise ReleaseError("invalid agent, sha256 or architecture")
    out = Path(out)
    if out.exists():
        raise ReleaseError(f"{out} already exists")
    config = runtime.r2_config()
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=out.parent) as directory:
        directory = Path(directory)
        probe = {"agent": agent, "sha256": sha256, "architecture": architecture}
        runtime.get_object(config, manifest_key(probe), directory / "bundle.json")
        manifest = load_manifest(directory / "bundle.json")
        bundle = manifest["bundle"]
        if (bundle["sha256"], bundle["agent"], bundle["architecture"]) != (sha256, agent, architecture):
            raise ReleaseError("published manifest describes a different bundle")
        verify_manifest(manifest, public_key)
        runtime.get_object(config, manifest["artifactKey"], directory / "bundle.tar.zst")
        subprocess.run(["zstd", "-q", "-d", "-f", "-o", str(directory / "bundle.tar"),
                        str(directory / "bundle.tar.zst")], check=True)
        if runtime.sha256_file(directory / "bundle.tar") != sha256:
            raise ReleaseError("downloaded bundle does not match its digest")
        staging = directory / "tree"
        staging.mkdir()
        safe_extract(directory / "bundle.tar", staging, bundle)
        os.replace(staging, out)
    return manifest


# -------------------------------------------------------------------- CLI ---


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)

    get = commands.add_parser("fetch", help="download and verify an exact upstream CLI into a bundle tree")
    get.add_argument("--agent", required=True, choices=AGENTS)
    get.add_argument("--version", required=True)
    get.add_argument("--arch", default="x86_64", choices=ARCHITECTURES)
    get.add_argument("--out", required=True)
    get.add_argument("--node", help="trusted Node executable (only for brotli-compressed vendor binaries)")

    build = commands.add_parser("build-artifact", help="probe, tar, hash and compress a bundle tree")
    build.add_argument("--agent", required=True, choices=AGENTS)
    build.add_argument("--version", required=True)
    build.add_argument("--arch", required=True, choices=ARCHITECTURES)
    build.add_argument("--tree", required=True)
    build.add_argument("--out", required=True)
    build.add_argument("--executable", help="path of the CLI inside the tree (default per agent)")
    build.add_argument("--adapter", help="adapter contract (default: this source tree's contract)")
    build.add_argument("--state-format", help="session-state format the CLI writes (default <agent>-sessions-v1)")
    build.add_argument("--reads-state-format", action="append", default=[],
                       help="repeatable: other session-state formats this CLI continues")
    build.add_argument("--capability", action="append", default=[], help="repeatable capability name")
    build.add_argument("--provenance", help="provenance.json written by fetch")

    sign = commands.add_parser("sign", help="Ed25519-sign bundle.json in place")
    sign.add_argument("--bundle", required=True)
    sign.add_argument("--key", required=True, help="Ed25519 private key (PEM)")
    sign.add_argument("--public-key", default=runtime.RELEASE_SIGNING_PUBLIC_KEY)

    verify = commands.add_parser("verify", help="verify bundle.json's signature")
    verify.add_argument("--bundle", required=True)
    verify.add_argument("--public-key", default=runtime.RELEASE_SIGNING_PUBLIC_KEY)

    up = commands.add_parser("upload", help="upload a signed bundle to R2 (idempotent)")
    up.add_argument("--bundle", required=True)
    up.add_argument("--artifact", required=True, help="directory containing <sha256>.tar.zst")
    up.add_argument("--public-key", default=runtime.RELEASE_SIGNING_PUBLIC_KEY)

    down = commands.add_parser("download", help="fetch, verify and extract a published bundle")
    down.add_argument("--agent", required=True, choices=AGENTS)
    down.add_argument("--sha256", required=True)
    down.add_argument("--arch", default="x86_64", choices=ARCHITECTURES)
    down.add_argument("--out", required=True)
    down.add_argument("--public-key", default=runtime.RELEASE_SIGNING_PUBLIC_KEY)

    raw = commands.add_parser("signing-bytes", help="print the exact signed bytes of bundle.json")
    raw.add_argument("--bundle", required=True)

    args = parser.parse_args(argv)
    try:
        if args.command == "fetch":
            tree, provenance = fetch(args.agent, args.version, args.arch, args.out, args.node)
            print(json.dumps({"tree": str(tree), "executable": EXECUTABLES[args.agent],
                              "npmIntegrity": provenance.get("npmIntegrity")}))
        elif args.command == "build-artifact":
            provenance = runtime.read_json(args.provenance) if args.provenance else None
            manifest = build_artifact(args.agent, args.version, args.arch, args.tree, args.out, args.executable,
                                      args.adapter, args.state_format, args.reads_state_format, args.capability,
                                      provenance)
            print(json.dumps({"sha256": manifest["bundle"]["sha256"], "artifactKey": manifest["artifactKey"]}))
        elif args.command == "sign":
            sign_manifest(args.bundle, args.key, args.public_key)
            print("signed")
        elif args.command == "verify":
            verify_manifest(load_manifest(args.bundle), args.public_key)
            print("valid")
        elif args.command == "upload":
            print(json.dumps(upload(args.bundle, args.artifact, args.public_key)))
        elif args.command == "download":
            manifest = download(args.agent, args.sha256, args.arch, args.out, args.public_key)
            print(json.dumps({"version": manifest["bundle"]["version"], "out": args.out}))
        elif args.command == "signing-bytes":
            sys.stdout.buffer.write(signing_bytes(load_manifest(args.bundle)["bundle"]))
    except (ReleaseError, OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"harness-bundle: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
