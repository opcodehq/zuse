#!/usr/bin/env python3
"""Build Zuse's Prized managed base: a prepared sandbox that is left PAUSED.

Prized has no custom images and no root. The base is an ordinary sandbox laid
out under /home/prized/.zuse by install.sh, with an idle runtime supervisor
serving; production computers are forks of the paused parent.

Run on a host that has the bundle (good bandwidth to Prized):

  python3 -I build.py --bundle DIR --key-file FILE

Idempotent: a paused sandbox labelled with this build's release id is reused;
any other sandbox carrying the labels (a failed or interrupted build) is
deleted first. A failed build deletes its sandbox.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import shlex
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.prized.dev/api/v1"
HOME = "/home/prized"
ROOT = HOME + "/.zuse"
INPUT = HOME + "/.zuse-input"
CHUNK = 95 * 1024 * 1024  # Prized refuses uploads over 100 MiB per file.
BUNDLE_FILES = ("runtime", "claude", "codex", "grok", "codex-assets.tar", "harness-manifest.json", "bundle.json")
# The repository's copies support the home-directory relocation; the bundle's are older.
REPO_FILES = ("launch-runtime.sh", "launch-recovery.sh", "runtime-supervisor.py", "runtime-repair.py",
              "runtime-recreation.py", "harness-release.py",
              # Read only for its pinned Node.js release and checksum.
              "managed-base-install.sh")
LOCAL_FILES = ("install.sh", "zuse-launch", "zuse-supervise", "launch-rust-recovery")
RESOURCES = {"vcpu": 2, "memoryMb": 4096, "diskGb": 20}


def log(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", file=sys.stderr, flush=True)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Never follow a redirect: it would resend the bearer key elsewhere."""
    def redirect_request(self, *args, **kwargs):
        return None


class Prized:
    def __init__(self, key):
        self.key = key
        self.opener = urllib.request.build_opener(NoRedirect)

    @staticmethod
    def trusted(url):
        parts = urllib.parse.urlsplit(url)
        host = parts.hostname or ""
        if parts.scheme != "https" or parts.username or parts.password or not (
                host == "prized.dev" or host.endswith(".prized.dev")):
            raise RuntimeError("untrusted_prized_url")
        return url

    def request(self, method, url, body=None, raw=None, timeout=120, retries=3):
        self.trusted(url)
        headers = {"Authorization": f"Bearer {self.key}", "Accept": "application/json"}
        data = None
        if raw is not None:
            data = raw
            headers["Content-Type"] = "application/octet-stream"
        elif body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        for attempt in range(retries):
            request = urllib.request.Request(url, data=data, method=method, headers=headers)
            try:
                with self.opener.open(request, timeout=timeout) as response:
                    payload = response.read()
                    return json.loads(payload) if payload.strip() else {}
            except urllib.error.HTTPError as error:
                detail = error.read()[:2000].decode(errors="replace")
                # Only reads are repeated here; uploads retry in Box.upload.
                if error.code in (429, 502, 503, 504) and attempt + 1 < retries and method == "GET":
                    time.sleep(2 * (attempt + 1))
                    continue
                raise RuntimeError(f"prized {method} {urllib.parse.urlsplit(url).path} -> {error.code}: {detail}") from None
            except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
                if attempt + 1 < retries and method == "GET":
                    time.sleep(2 * (attempt + 1))
                    continue
                raise RuntimeError(f"prized {method} {urllib.parse.urlsplit(url).path} failed: {error}") from None

    def api(self, method, path, body=None, timeout=300):
        return self.request(method, API + path, body=body, timeout=timeout)

    def create(self, labels, max_runtime=60):
        value = self.api("POST", "/sandboxes?wait=running", {"template": "prized-base-v1", "resources": RESOURCES,
                         "labels": labels, "maxRuntimeMin": max_runtime}, timeout=600)
        return value["sandbox"]

    def get(self, sandbox):
        return self.api("GET", f"/sandboxes/{sandbox}")["sandbox"]

    def list(self, label):
        value = self.api("GET", "/sandboxes?" + urllib.parse.urlencode({"label": label}))
        return value.get("sandboxes", value.get("items", [])) if isinstance(value, dict) else value

    def pause(self, sandbox):
        return self.api("POST", f"/sandboxes/{sandbox}/pause?wait=paused", timeout=600)

    def delete(self, sandbox):
        try:
            self.api("DELETE", f"/sandboxes/{sandbox}")
        except RuntimeError as error:
            if "-> 404" not in str(error) and "-> 410" not in str(error):
                raise


class Box:
    """One sandbox's toolbox (process + files) endpoint."""
    def __init__(self, client, sandbox):
        self.client = client
        self.id = sandbox["id"]
        self.base = Prized.trusted(sandbox["toolboxBase"].rstrip("/"))

    def run(self, command, timeout=300, check=True):
        value = self.client.request("POST", self.base + "/process/execute",
                                    body={"command": command, "timeout": timeout}, timeout=timeout + 60)
        if value.get("timedOut") or (check and value.get("exitCode") != 0):
            raise RuntimeError(f"remote command failed ({value.get('exitCode')}, timedOut={value.get('timedOut')}):"
                               f" {command[:200]}\nstdout: {value.get('stdout', '')[-4000:]}\n"
                               f"stderr: {value.get('stderr', '')[-4000:]}")
        return value

    def sh(self, script, timeout=300, check=True):
        return self.run("bash -c " + shlex.quote(script), timeout=timeout, check=check)

    def upload(self, path, data):
        assert path.startswith(HOME + "/") and "/../" not in path
        url = self.base + "/files/upload?" + urllib.parse.urlencode({"path": path})
        # Same bytes to the same path: safe to repeat after a transient failure.
        for attempt in range(4):
            try:
                return self.client.request("POST", url, raw=data, timeout=900, retries=1)
            except RuntimeError:
                if attempt == 3:
                    raise
                time.sleep(3 * (attempt + 1))

    def background(self, script, name, timeout):
        """Run a long script detached, then poll its exit-status file."""
        state = f"{INPUT}/.{name}"
        self.sh(f"rm -f {state}.rc; nohup setsid bash -c {shlex.quote(script + f'; echo $? > {state}.rc.next; mv {state}.rc.next {state}.rc')}"
                f" >{state}.log 2>&1 </dev/null & echo started")
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            time.sleep(5)
            value = self.sh(f"cat {state}.rc 2>/dev/null || true")
            if value["stdout"].strip():
                code = int(value["stdout"].strip())
                output = self.sh(f"cat {state}.log", check=False)["stdout"]
                if code != 0:
                    raise RuntimeError(f"{name} failed with {code}:\n{output[-6000:]}")
                return output
        raise RuntimeError(f"{name} timed out")


def digest(path):
    result = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            result.update(block)
    return result.hexdigest()


def collect(bundle, repo, local):
    """Every input file and its sha256; the build id binds all of them."""
    manifest = json.loads((bundle / "bundle.json").read_text())
    expected = {entry["path"]: entry["sha256"] for entry in manifest["files"]}
    files = {}
    for name in BUNDLE_FILES:
        path = bundle / name
        sha = digest(path)
        if name in expected and expected[name] != sha:
            raise SystemExit(f"bundle_integrity_failed: {name}")
        files[name] = (path, sha)
    if files["runtime"][1] != manifest["runtimeSha256"]:
        raise SystemExit("bundle_runtime_mismatch")
    for name in REPO_FILES:
        files["repo/" + name] = (repo / name, digest(repo / name))
    for name in LOCAL_FILES:
        files["local/" + name] = (local / name, digest(local / name))
    binding = {"bundleRelease": manifest["releaseId"], "harnessRelease": manifest["harnessRelease"],
               "files": {name: sha for name, (_, sha) in sorted(files.items())}, "layout": ROOT, "resources": RESOURCES}
    build_id = "zuse-prized-base-" + hashlib.sha256(json.dumps(binding, sort_keys=True).encode()).hexdigest()[:32]
    return manifest, files, build_id


def plan_for(build_id):
    # Field names follow crates/cloud-runtime/src/image.rs ImagePlan (camelCase).
    return {"buildId": build_id, "templateVersion": build_id, "configurationDigest": build_id,
            "home": HOME, "cacheRoot": HOME + "/.zuse-image/cache",
            "statusDirectory": ROOT + "/preparation/status", "secretsRoot": ROOT + "/preparation/secrets",
            "managedBase": True, "maxCacheBytes": 1048576, "repositories": [], "rustRuntimeSupervisorVersion": 1}


def upload_all(box, files, extra):
    jobs = []
    for name, (path, sha) in files.items():
        size = path.stat().st_size
        count = max(1, -(-size // CHUNK))
        for index in range(count):
            jobs.append((name, path, index, count))
    box.sh(f"rm -rf {INPUT} && mkdir -p -m 700 {INPUT}/repo {INPUT}/local")

    def send(job):
        name, path, index, count = job
        with open(path, "rb") as source:
            source.seek(index * CHUNK)
            data = source.read(CHUNK)
        target = f"{INPUT}/{name}" + (f".part{index:03d}" if count > 1 else "")
        box.upload(target, data)
        return len(data)

    started, sent = time.monotonic(), 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        for size in pool.map(send, jobs):
            sent += size
    log(f"uploaded {sent / 1e6:.0f} MB in {time.monotonic() - started:.0f}s")
    for name, body in extra.items():
        box.upload(f"{INPUT}/{name}", body)
    # Reassemble split files, then check every input against its digest.
    script = ["set -euo pipefail", f"cd {INPUT}"]
    for name, (path, sha) in files.items():
        if path.stat().st_size > CHUNK:
            script.append(f"cat {shlex.quote(name)}.part* > {shlex.quote(name)} && rm -f {shlex.quote(name)}.part*")
    script.append("sha256sum -c --quiet inputs.sha256")
    box.sh("\n".join(script), timeout=900)


def verify(box, harness):
    rel = f"{ROOT}/harnesses/releases/{harness}"
    probe = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {"clientInfo": {"name": "probe", "version": "0"}, "capabilities": {"experimentalApi": True}}})
    checks = {
        "runtimeVersion": f"{ROOT}/bin/zuse-cloud-runtime --version",
        "supervisorPing": f"python3 {ROOT}/lib/runtime-supervisor.py --data-dir {HOME}/.zuse-data ping",
        "gh": f"{ROOT}/bin/gh --version | head -n 1",
        "loginShellGh": "bash -lc 'command -v gh && gh --version | head -n 1'",
        "node": f"env -i HOME={HOME} PATH={HOME}/.local/bin:/usr/bin:/bin node --version",
        "claude": f"h=$(mktemp -d); env -i PATH=/usr/bin:/bin HOME=$h DISABLE_AUTOUPDATER=1 {rel}/claude --version; rc=$?; rm -rf $h; exit $rc",
        "codexAppServer": (f"h=$(mktemp -d); (printf '%s\\n' {shlex.quote(probe)}; sleep 8) | env -i PATH=/usr/bin:/bin HOME=$h "
                           f"timeout 20 {rel}/codex-package/bin/codex app-server 2>/dev/null | head -n 1; rm -rf $h"),
        "workspaceEmpty": f"test -z \"$(ls -A {HOME}/workspace)\" && echo empty",
        "dataDir": f"ls -A {HOME}/.zuse-data",
        "inputRemoved": f"test ! -e {INPUT} && echo removed",
    }
    results = {}
    for name, command in checks.items():
        results[name] = box.sh(command, timeout=60)["stdout"].strip()
    ping = json.loads(results["supervisorPing"])
    assert ping["version"] == 1 and ping["bound"] is False and ping["childAlive"] is False, "supervisor_not_idle"
    assert results["gh"].startswith("gh version 2."), "gh_version_invalid"
    assert results["loginShellGh"].splitlines()[0] == f"{ROOT}/bin/gh", "gh_login_path_invalid"
    assert results["node"].startswith("v"), "node_unavailable"
    reply = json.loads(results["codexAppServer"])
    assert reply.get("id") == 1 and "result" in reply, "codex_app_server_initialize_failed"
    assert results["workspaceEmpty"] == "empty" and results["dataDir"] == ".runtime-supervision", "base_not_clean"
    return results


def main():
    here = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--key-file", type=Path, default=Path.home() / ".prized/key")
    parser.add_argument("--repo-scripts", type=Path, default=here.parent, help="infra/cloud of this checkout")
    parser.add_argument("--keep-on-failure", action="store_true", help="debug only: leave a failed builder running")
    parser.add_argument("--cleanup", action="store_true", help="delete every sandbox carrying this build's labels")
    args = parser.parse_args()
    started = time.monotonic()
    key = args.key_file.read_text().strip()
    client = Prized(key)
    bundle, files, build_id = collect(args.bundle, args.repo_scripts, here)
    labels = {"zuse-role": "managed-base", "zuse-release": build_id}
    log(f"build id {build_id} (bundle {bundle['releaseId']}, harness {bundle['harnessRelease']})")

    existing = client.list(f"zuse-release={build_id}")
    existing = [s for s in existing if s.get("labels", {}).get("zuse-role") == "managed-base"
                and s.get("state") not in ("deleted", "deleting")]
    if args.cleanup:
        for sandbox in existing:
            log(f"deleting {sandbox['id']} ({sandbox.get('state')})")
            client.delete(sandbox["id"])
        return
    # Only a fully verified build is ever paused, so a paused one is the result.
    done = [s for s in existing if s.get("state") == "paused"]
    for sandbox in existing:
        if sandbox not in done[:1]:
            log(f"deleting stale builder {sandbox['id']} ({sandbox.get('state')})")
            client.delete(sandbox["id"])
    if done:
        print(json.dumps({"parent": done[0]["id"], "buildId": build_id, "reused": True}))
        return

    sandbox = client.create(labels)
    box = Box(client, sandbox)
    log(f"created {box.id}")
    try:
        sums = "".join(f"{sha}  {name}\n" for name, (_, sha) in files.items())
        meta = {"buildId": build_id, "harnessRelease": bundle["harnessRelease"], "bundleRelease": bundle["releaseId"]}
        upload_all(box, files, {"inputs.sha256": sums.encode(), "build.json": json.dumps(meta).encode(),
                                "plan.json": json.dumps(plan_for(build_id), sort_keys=True).encode()})
        log("inputs verified; installing")
        output = box.background(f"bash {INPUT}/local/install.sh {INPUT}", "install", timeout=1800)
        manifest = None
        for line in output.splitlines():
            if line.startswith("ZUSE_IMAGE_MANIFEST "):
                manifest = json.loads(line.split(" ", 1)[1])
        assert manifest and manifest["buildId"] == build_id, "image_manifest_missing"
        # install.sh logs into the input directory; remove it only after reading.
        box.sh(f"rm -rf -- {INPUT}")
        results = verify(box, bundle["harnessRelease"])
        log("verified; pausing")
        client.pause(box.id)
        state = client.get(box.id)["state"]
        assert state == "paused", f"pause_failed: {state}"
    except BaseException:
        if args.keep_on_failure:
            log(f"leaving failed builder {box.id} running (--keep-on-failure)")
        else:
            log(f"build failed; deleting {box.id}")
            client.delete(box.id)
        raise
    print(json.dumps({"parent": box.id, "buildId": build_id, "labels": labels, "manifest": manifest,
                      "checks": results, "seconds": round(time.monotonic() - started)}, indent=1))


if __name__ == "__main__":
    main()
