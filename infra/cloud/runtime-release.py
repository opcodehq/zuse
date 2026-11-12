#!/usr/bin/env python3
"""Build, sign, upload and fetch signed cloud runtime releases.

Contract: `crates/protocol/src/cloud_runtime_updates.rs` (`SignedRelease`).
Design: `internal-docs/cloud-runtime-updates.md` ("Artifact and trust").
Operator steps: `infra/cloud/RUNTIME_RELEASES.md`.

Stdlib only, plus the `openssl` (Ed25519) and `zstd` command line tools.
"""
import argparse
import base64
import datetime
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request

SIGNING_DOMAIN = b"zuse-runtime-release-v1\n"
# Must equal `RELEASE_SIGNING_PUBLIC_KEY` in the Rust contract (checked by tests).
RELEASE_SIGNING_PUBLIC_KEY = "CTEMp+P0mILHcWFjITtV0yipAeHMNdlho7pxbsntKd8="
# `zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT` of the runtime being released.
DEFAULT_STORAGE_FORMATS = ("zuse-rust-engine-v1",)
ARCHITECTURES = ("x86_64", "aarch64")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?\Z")
FORMAT = re.compile(r"[a-z0-9][a-z0-9.-]{0,63}\Z")
# DER SubjectPublicKeyInfo prefix for a raw 32-byte Ed25519 public key.
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")
CAPABILITIES_TIMEOUT_SECONDS = 5
R2_ENV = (
    "ZUSE_CLOUD_RUNTIME_R2_BUCKET",
    "ZUSE_CLOUD_RUNTIME_R2_ENDPOINT",
    "ZUSE_CLOUD_RUNTIME_R2_ACCESS_KEY_ID",
    "ZUSE_CLOUD_RUNTIME_R2_SECRET_ACCESS_KEY",
)


class ReleaseError(Exception):
    pass


# ---------------------------------------------------------------- release ---


def validate_release(release):
    """Checks a `SignedRelease` object; field order is enforced by `signing_bytes`."""
    if not isinstance(release, dict) or set(release) != {"sha256", "version", "architecture", "storageFormats"}:
        raise ReleaseError("release must have exactly sha256, version, architecture, storageFormats")
    if not isinstance(release["sha256"], str) or not DIGEST.match(release["sha256"]):
        raise ReleaseError("release.sha256 must be 64 lowercase hex characters")
    if not isinstance(release["version"], str) or not VERSION.match(release["version"]):
        raise ReleaseError("release.version must look like 1.2.3 or 1.2.3+build")
    if release["architecture"] not in ARCHITECTURES:
        raise ReleaseError(f"release.architecture must be one of {', '.join(ARCHITECTURES)}")
    formats = release["storageFormats"]
    if not isinstance(formats, list) or not formats or len(set(formats)) != len(formats):
        raise ReleaseError("release.storageFormats must be a non-empty list without duplicates")
    if not all(isinstance(value, str) and FORMAT.match(value) for value in formats):
        raise ReleaseError("release.storageFormats entries must be lowercase format names")
    return release


def signing_bytes(release):
    """Byte-identical to Rust `SignedRelease::signing_bytes()`: domain line plus
    compact JSON with keys in declaration order. Validation keeps every value in
    a character set that Python and serde_json encode identically."""
    validate_release(release)
    ordered = {key: release[key] for key in ("sha256", "version", "architecture", "storageFormats")}
    return SIGNING_DOMAIN + json.dumps(ordered, separators=(",", ":"), ensure_ascii=False).encode()


def sha256_file(path):
    result = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def read_json(path):
    with open(path, "rb") as source:
        return json.load(source)


def write_json(path, value):
    path = Path(path)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    os.replace(temporary, path)


def artifact_key(release):
    return f"runtime/{release['architecture']}/{release['sha256']}.zst"


def release_key(release):
    return f"runtime/{release['architecture']}/{release['sha256']}.release.json"


def load_manifest(path):
    manifest = read_json(path)
    if not isinstance(manifest, dict) or "release" not in manifest:
        raise ReleaseError(f"{path} is not a runtime release manifest")
    validate_release(manifest["release"])
    if manifest.get("artifactKey") != artifact_key(manifest["release"]):
        raise ReleaseError("artifactKey does not match release architecture and sha256")
    return manifest


# ------------------------------------------------------------------ build ---


def run_capabilities(binary):
    try:
        completed = subprocess.run(
            [str(binary), "runtime-capabilities"],
            env={"PATH": "/usr/bin:/bin"},
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=CAPABILITIES_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        raise ReleaseError("runtime-capabilities timed out") from None
    if completed.returncode != 0:
        raise ReleaseError(f"runtime-capabilities exited {completed.returncode}")
    if len(completed.stdout) > 8192:
        raise ReleaseError("runtime-capabilities output is larger than the 8 KiB receipt limit")
    try:
        receipt = json.loads(completed.stdout)
    except ValueError:
        raise ReleaseError("runtime-capabilities did not print JSON") from None
    if not isinstance(receipt, dict):
        raise ReleaseError("runtime-capabilities must print a JSON object")
    return receipt


def build_artifact(binary, version, architecture, out, storage_formats):
    binary = Path(binary)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ReleaseError(f"{binary} is not an executable file")
    formats = list(storage_formats or DEFAULT_STORAGE_FORMATS)
    digest = sha256_file(binary)
    release = validate_release({
        "sha256": digest,
        "version": version,
        "architecture": architecture,
        "storageFormats": formats,
    })
    # The receipt is the exact guest probe `runtime-update.py` repeats on install.
    receipt = run_capabilities(binary)
    if receipt.get("runtimeSha256", "").lower() != digest:
        raise ReleaseError("runtime-capabilities runtimeSha256 does not match the binary")
    if receipt.get("storageFormat") not in formats:
        raise ReleaseError(f"runtime storage format {receipt.get('storageFormat')!r} is not in storageFormats")
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    compressed = out / f"{digest}.zst"
    temporary = out / f".{digest}.zst.tmp"
    subprocess.run(["zstd", "-q", "-19", "-T0", "-f", "-o", str(temporary), str(binary)], check=True)
    if decompressed_sha256(temporary) != digest:
        raise ReleaseError("compressed artifact does not decompress to the binary")
    os.replace(temporary, compressed)
    manifest = {
        "release": release,
        "signature": None,
        "artifactKey": artifact_key(release),
        "compressedSizeBytes": compressed.stat().st_size,
        "capabilities": receipt,
    }
    write_json(out / "release.json", manifest)
    return manifest


def decompressed_sha256(compressed):
    result = hashlib.sha256()
    with subprocess.Popen(["zstd", "-q", "-d", "-c", str(compressed)], stdout=subprocess.PIPE) as process:
        for block in iter(lambda: process.stdout.read(1024 * 1024), b""):
            result.update(block)
    if process.returncode != 0:
        raise ReleaseError(f"{compressed} is not a valid zstd stream")
    return result.hexdigest()


# ---------------------------------------------------------------- signing ---


def decode_public_key(value):
    try:
        raw = base64.b64decode(value, validate=True)
    except ValueError:
        raise ReleaseError("public key must be standard base64") from None
    if len(raw) != 32:
        raise ReleaseError("public key must be 32 raw Ed25519 bytes")
    return raw


def public_key_of(private_key_pem):
    der = subprocess.run(
        ["openssl", "pkey", "-in", str(private_key_pem), "-pubout", "-outform", "DER"],
        capture_output=True, check=False,
    )
    if der.returncode != 0 or len(der.stdout) != 44 or not der.stdout.startswith(ED25519_SPKI_PREFIX):
        raise ReleaseError("signing key must be an Ed25519 private key in PEM form")
    return base64.b64encode(der.stdout[len(ED25519_SPKI_PREFIX):]).decode()


def sign_bytes(data, private_key_pem):
    with tempfile.TemporaryDirectory() as directory:
        message = Path(directory) / "message"
        message.write_bytes(data)
        signed = subprocess.run(
            ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(private_key_pem), "-in", str(message)],
            capture_output=True, check=False,
        )
    if signed.returncode != 0 or len(signed.stdout) != 64:
        raise ReleaseError("openssl could not produce an Ed25519 signature")
    return base64.b64encode(signed.stdout).decode()


def verify_bytes(data, signature, public_key):
    raw = decode_public_key(public_key)
    try:
        signature_bytes = base64.b64decode(signature or "", validate=True)
    except ValueError:
        return False
    if len(signature_bytes) != 64:
        return False
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        (directory / "key.der").write_bytes(ED25519_SPKI_PREFIX + raw)
        (directory / "message").write_bytes(data)
        (directory / "signature").write_bytes(signature_bytes)
        verified = subprocess.run(
            ["openssl", "pkeyutl", "-verify", "-pubin", "-keyform", "DER", "-inkey", str(directory / "key.der"),
             "-rawin", "-in", str(directory / "message"), "-sigfile", str(directory / "signature")],
            capture_output=True, check=False,
        )
    return verified.returncode == 0


def sign_manifest(path, private_key_pem, expected_public_key):
    manifest = load_manifest(path)
    actual = public_key_of(private_key_pem)
    if actual != expected_public_key:
        # Never publish a release no computer would accept.
        raise ReleaseError("signing key does not match the expected release public key")
    data = signing_bytes(manifest["release"])
    manifest["signature"] = sign_bytes(data, private_key_pem)
    if not verify_bytes(data, manifest["signature"], expected_public_key):
        raise ReleaseError("fresh signature failed verification")
    write_json(path, manifest)
    return manifest


def verify_manifest(manifest, public_key):
    if not isinstance(manifest.get("signature"), str):
        raise ReleaseError("release is not signed")
    if not verify_bytes(signing_bytes(manifest["release"]), manifest["signature"], public_key):
        raise ReleaseError("release signature is invalid")


# ------------------------------------------------------------- S3 / R2 ------


def r2_config():
    missing = [name for name in R2_ENV if not os.environ.get(name)]
    if missing:
        raise ReleaseError(f"missing environment: {', '.join(missing)}")
    bucket, endpoint, access, secret = (os.environ[name] for name in R2_ENV)
    return {"bucket": bucket, "endpoint": endpoint.rstrip("/"), "access": access, "secret": secret,
            "region": os.environ.get("ZUSE_CLOUD_RUNTIME_R2_REGION", "auto")}


def sigv4_headers(method, url, headers, payload_sha256, access, secret, region, now, service="s3"):
    """AWS Signature Version 4 for a single request (header auth)."""
    parsed = urllib.parse.urlsplit(url)
    amz_date = now.strftime("%Y%m%dT%H%M%SZ")
    day = amz_date[:8]
    signed = {key.lower(): " ".join(str(value).split()) for key, value in headers.items()}
    signed["host"] = parsed.netloc
    signed["x-amz-date"] = amz_date
    signed["x-amz-content-sha256"] = payload_sha256
    names = sorted(signed)
    query = "&".join(
        f"{urllib.parse.quote(k, safe='-_.~')}={urllib.parse.quote(v, safe='-_.~')}"
        for k, v in sorted(urllib.parse.parse_qsl(parsed.query, keep_blank_values=True))
    )
    canonical = "\n".join([
        method,
        urllib.parse.quote(parsed.path or "/", safe="/-_.~"),
        query,
        "".join(f"{name}:{signed[name]}\n" for name in names),
        ";".join(names),
        payload_sha256,
    ])
    scope = f"{day}/{region}/{service}/aws4_request"
    to_sign = "\n".join(["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canonical.encode()).hexdigest()])
    key = ("AWS4" + secret).encode()
    for part in (day, region, service, "aws4_request"):
        key = hmac.new(key, part.encode(), hashlib.sha256).digest()
    signature = hmac.new(key, to_sign.encode(), hashlib.sha256).hexdigest()
    result = {name: signed[name] for name in names if name != "host"}
    result["authorization"] = (
        f"AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={';'.join(names)}, Signature={signature}"
    )
    return result


def object_url(config, key):
    return f"{config['endpoint']}/{config['bucket']}/{urllib.parse.quote(key, safe='/-_.~')}"


def s3_request(config, method, key, body=None, payload_sha256=None, extra_headers=None):
    url = object_url(config, key)
    empty = hashlib.sha256(b"").hexdigest()
    headers = sigv4_headers(method, url, extra_headers or {}, payload_sha256 or empty,
                            config["access"], config["secret"], config["region"],
                            datetime.datetime.now(datetime.timezone.utc))
    request = urllib.request.Request(url, data=body, method=method, headers=headers)
    return urllib.request.urlopen(request, timeout=600)


def remote_size(config, key):
    try:
        with s3_request(config, "HEAD", key) as response:
            return int(response.headers.get("Content-Length", "-1"))
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise ReleaseError(f"HEAD {key} failed with HTTP {error.code}") from None


def put_object(config, key, path, content_type):
    """Content-addressed, idempotent PUT: an existing object of the same size is kept."""
    size = Path(path).stat().st_size
    existing = remote_size(config, key)
    if existing == size:
        return "exists"
    if existing is not None:
        raise ReleaseError(f"{key} already exists with a different size ({existing} != {size}); refusing to overwrite")
    with open(path, "rb") as body:
        try:
            with s3_request(config, "PUT", key, body=body, payload_sha256=sha256_file(path),
                            extra_headers={"content-type": content_type, "content-length": str(size)}):
                pass
        except urllib.error.HTTPError as error:
            raise ReleaseError(f"PUT {key} failed with HTTP {error.code}") from None
    if remote_size(config, key) != size:
        raise ReleaseError(f"{key} did not read back with the uploaded size")
    return "uploaded"


def get_object(config, key, destination):
    try:
        with s3_request(config, "GET", key) as response, open(destination, "wb") as target:
            shutil.copyfileobj(response, target, 1024 * 1024)
    except urllib.error.HTTPError as error:
        raise ReleaseError(f"GET {key} failed with HTTP {error.code}") from None


def upload(path, artifact_dir, public_key):
    manifest = load_manifest(path)
    verify_manifest(manifest, public_key)
    release = manifest["release"]
    artifact = Path(artifact_dir) / f"{release['sha256']}.zst"
    if artifact.stat().st_size != manifest["compressedSizeBytes"]:
        raise ReleaseError("artifact size does not match compressedSizeBytes")
    if decompressed_sha256(artifact) != release["sha256"]:
        raise ReleaseError("artifact does not decompress to release.sha256")
    config = r2_config()
    # Artifact first: a published manifest always points at an existing object.
    results = {
        manifest["artifactKey"]: put_object(config, manifest["artifactKey"], artifact, "application/zstd"),
        release_key(release): put_object(config, release_key(release), path, "application/json"),
    }
    return results


def download(sha256, architecture, out, public_key):
    """Fetches a published release by digest, checks signature and digest, and
    writes the executable runtime to `out`."""
    if not DIGEST.match(sha256) or architecture not in ARCHITECTURES:
        raise ReleaseError("invalid sha256 or architecture")
    config = r2_config()
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=out.parent) as directory:
        directory = Path(directory)
        probe = {"sha256": sha256, "architecture": architecture}
        get_object(config, release_key(probe), directory / "release.json")
        manifest = load_manifest(directory / "release.json")
        if manifest["release"]["sha256"] != sha256 or manifest["release"]["architecture"] != architecture:
            raise ReleaseError("published manifest describes a different release")
        verify_manifest(manifest, public_key)
        get_object(config, manifest["artifactKey"], directory / "runtime.zst")
        subprocess.run(["zstd", "-q", "-d", "-f", "-o", str(directory / "runtime"), str(directory / "runtime.zst")],
                       check=True)
        if sha256_file(directory / "runtime") != sha256:
            raise ReleaseError("downloaded runtime does not match its digest")
        (directory / "runtime").chmod(0o755)
        os.replace(directory / "runtime", out)
    return manifest


# -------------------------------------------------------------------- CLI ---


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)

    build = commands.add_parser("build-artifact", help="hash, probe and compress a runtime binary")
    build.add_argument("--binary", required=True)
    build.add_argument("--version", required=True)
    build.add_argument("--arch", required=True, choices=ARCHITECTURES)
    build.add_argument("--out", required=True)
    build.add_argument("--storage-format", action="append",
                       help=f"repeatable; default {', '.join(DEFAULT_STORAGE_FORMATS)}")

    sign = commands.add_parser("sign", help="Ed25519-sign release.json in place")
    sign.add_argument("--release", required=True)
    sign.add_argument("--key", required=True, help="Ed25519 private key (PEM)")
    sign.add_argument("--public-key", default=RELEASE_SIGNING_PUBLIC_KEY,
                      help="expected raw base64 public key (default: the contract key)")

    verify = commands.add_parser("verify", help="verify release.json's signature")
    verify.add_argument("--release", required=True)
    verify.add_argument("--public-key", default=RELEASE_SIGNING_PUBLIC_KEY)

    up = commands.add_parser("upload", help="upload a signed release to R2 (idempotent)")
    up.add_argument("--release", required=True)
    up.add_argument("--artifact", required=True, help="directory containing <sha256>.zst")
    up.add_argument("--public-key", default=RELEASE_SIGNING_PUBLIC_KEY)

    down = commands.add_parser("download", help="fetch and verify a published runtime by digest")
    down.add_argument("--sha256", required=True)
    down.add_argument("--arch", default="x86_64", choices=ARCHITECTURES)
    down.add_argument("--out", required=True)
    down.add_argument("--public-key", default=RELEASE_SIGNING_PUBLIC_KEY)

    bytes_command = commands.add_parser("signing-bytes", help="print the exact signed bytes of release.json")
    bytes_command.add_argument("--release", required=True)

    args = parser.parse_args(argv)
    try:
        if args.command == "build-artifact":
            manifest = build_artifact(args.binary, args.version, args.arch, args.out, args.storage_format)
            print(json.dumps({"sha256": manifest["release"]["sha256"], "artifactKey": manifest["artifactKey"]}))
        elif args.command == "sign":
            sign_manifest(args.release, args.key, args.public_key)
            print("signed")
        elif args.command == "verify":
            verify_manifest(load_manifest(args.release), args.public_key)
            print("valid")
        elif args.command == "upload":
            print(json.dumps(upload(args.release, args.artifact, args.public_key)))
        elif args.command == "download":
            manifest = download(args.sha256, args.arch, args.out, args.public_key)
            print(json.dumps({"version": manifest["release"]["version"], "out": args.out}))
        elif args.command == "signing-bytes":
            sys.stdout.buffer.write(signing_bytes(load_manifest(args.release)["release"]))
    except (ReleaseError, OSError, subprocess.CalledProcessError) as error:
        print(f"runtime-release: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
