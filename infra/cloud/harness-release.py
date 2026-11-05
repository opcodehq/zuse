#!/usr/bin/env python3
"""Offline, content-addressed cloud harness releases. No installation at chat time."""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import tarfile
import struct
import select
import signal
import time

CHECKS = {"protocol", "streaming", "tools", "stop", "resume", "auth", "boxd", "boat"}
AGENTS = ("claude", "codex", "grok")
CHANNELS = {"staging", "stable"}
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][a-zA-Z0-9.-]+)?\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def digest(path):
    with path.open("rb") as source:
        result = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
        return result.hexdigest()


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(dir=path.parent, prefix=".write-")
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(canonical(value) + b"\n")
            output.flush()
            os.fchmod(output.fileno(), 0o644)
            os.fsync(output.fileno())
        os.replace(name, path)
        directory = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(name).unlink(missing_ok=True)


@contextlib.contextmanager
def locked(root, read_only=False):
    if not read_only:
        root.mkdir(parents=True, exist_ok=True)
        root.chmod(0o755)
    with (root / ".release-lock").open("r" if read_only else "a") as lock:
        if not read_only:
            os.fchmod(lock.fileno(), 0o644)
        fcntl.flock(lock, fcntl.LOCK_SH if read_only else fcntl.LOCK_EX)
        yield


CODEX_REQUIRED={'codex-package/bin/codex-code-mode-host','codex-package/codex-resources/bwrap','codex-package/codex-path/rg','codex-package/codex-package.json','codex-package/npm-package.json','codex-package/README.md'}

def asset_path(root, name):
    parts=Path(name).parts
    if not isinstance(name,str) or not name.startswith('codex-package/') or Path(name).is_absolute() or '..' in parts or str(Path(name))!=name:
        raise ValueError('unsafe_distribution_asset_path')
    current=root
    for part in parts:
        current=current/part
        if current.is_symlink():raise ValueError('distribution_asset_symlink')
    return current


def validate_assets(root, assets, verify=True):
    if not isinstance(assets,dict) or not CODEX_REQUIRED.issubset(assets) or len(assets)>128:
        raise ValueError('codex_distribution_incomplete')
    for name,pin in assets.items():
        path=asset_path(root,name)
        if not isinstance(pin,dict) or set(pin)!={'sha256','bytes','executable'} or not DIGEST.fullmatch(pin.get('sha256','')) or type(pin['bytes']) is not int or not 0<pin['bytes']<=128*1024*1024 or type(pin['executable']) is not bool:
            raise ValueError('invalid_distribution_asset_pin')
        if not path.is_file() or path.stat().st_size!=pin['bytes'] or (pin['executable'] and not os.access(path,os.X_OK)) or (verify and digest(path)!=pin['sha256']):
            raise ValueError('distribution_asset_integrity_failed')


def copy_assets(source, destination, assets):
    validate_assets(source,assets)
    for name,pin in assets.items():
        target=asset_path(destination,name);target.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(asset_path(source,name),target);target.chmod(0o555 if pin['executable'] else 0o444)


def archive_assets(source, assets, output):
    validate_assets(source,assets)
    with tarfile.open(output,'w') as archive:
        for name in sorted(assets):
            info=archive.gettarinfo(str(asset_path(source,name)),arcname=name)
            info.uid=info.gid=0;info.uname=info.gname='';info.mtime=0
            with asset_path(source,name).open('rb') as data:archive.addfile(info,data)


def verify_asset_archive(archive_path, assets):
    if not isinstance(assets,dict) or not CODEX_REQUIRED.issubset(assets) or len(assets)>128:raise ValueError('codex_distribution_incomplete')
    with tarfile.open(archive_path,'r:') as archive:
        members=archive.getmembers()
        if len(members)!=len(assets) or {m.name for m in members}!=set(assets):raise ValueError('distribution_archive_members_mismatch')
        for member in members:
            asset_path(Path('/unused-asset-validation'),member.name)
            pin=assets[member.name]
            if not member.isfile() or member.issym() or member.islnk() or member.size!=pin.get('bytes') or member.size>128*1024*1024:raise ValueError('distribution_archive_member_invalid')
            sha=hashlib.sha256()
            with archive.extractfile(member) as data:
                while block:=data.read(1024*1024):sha.update(block)
            if sha.hexdigest()!=pin.get('sha256'):raise ValueError('distribution_archive_integrity_failed')


def install_assets(archive_path, destination, assets):
    verify_asset_archive(archive_path,assets)
    if not isinstance(assets,dict) or not CODEX_REQUIRED.issubset(assets):raise ValueError('codex_distribution_incomplete')
    with tarfile.open(archive_path,'r:') as archive:
        members=archive.getmembers()
        if len(members)!=len(assets) or {m.name for m in members}!=set(assets):raise ValueError('distribution_archive_members_mismatch')
        for member in members:
            if not member.isfile() or member.issym() or member.islnk():raise ValueError('distribution_archive_links_rejected')
            target=asset_path(destination,member.name);pin=assets[member.name]
            if member.size!=pin['bytes']:raise ValueError('distribution_archive_size_mismatch')
            target.parent.mkdir(parents=True,exist_ok=True)
            with archive.extractfile(member) as source,target.open('xb') as output:shutil.copyfileobj(source,output,1024*1024)
            target.chmod(0o555 if pin['executable'] else 0o444)
    validate_assets(destination,assets)


def complete_codex(root, manifest):
    entry=manifest['harnesses']['codex'];assets=entry.get('assets')
    if assets:
        validate_assets(root,assets)
        target=asset_path(root,'codex-package/bin/codex');target.parent.mkdir(parents=True,exist_ok=True)
        if not target.exists():os.link(root/'codex',target)
        if target.is_symlink() or digest(target)!=entry['sha256']:raise ValueError('codex_distribution_executable_mismatch')


def release(root, identifier, verify=False):
    if not isinstance(identifier, str) or not DIGEST.fullmatch(identifier):
        raise ValueError("invalid harness release ID")
    directory = root / "releases" / identifier
    manifest = json.loads((directory / "manifest.json").read_text())
    if hashlib.sha256(canonical(manifest)).hexdigest() != identifier:
        raise ValueError("harness manifest digest mismatch")
    if manifest["schema"] not in (1, 2) or manifest["platform"] != f"{platform.system().lower()}-{platform.machine()}":
        raise ValueError("incompatible harness release platform/schema")
    agents = AGENTS if manifest["schema"] == 2 else AGENTS[:2]
    if set(manifest["harnesses"]) != set(agents):
        raise ValueError("incomplete harness manifest")
    for provider in agents:
        entry = manifest["harnesses"][provider]
        binary = directory / provider
        if not VERSION.fullmatch(entry["version"]) or binary.is_symlink() or not binary.is_file():
            raise ValueError("incomplete harness release")
        if not os.access(binary, os.X_OK) or (verify and digest(binary) != entry["sha256"]):
            raise ValueError("harness binary integrity check failed")
    entry=manifest['harnesses']['codex']
    if entry.get('assets'):
        if entry.get('executable')!='codex-package/bin/codex':raise ValueError('codex_distribution_executable_invalid')
        validate_assets(directory,entry['assets'],verify)
        primary=asset_path(directory,entry['executable'])
        if not primary.is_file() or primary.stat().st_size!=(directory/'codex').stat().st_size or (verify and digest(primary)!=entry['sha256']):raise ValueError('codex_distribution_executable_mismatch')
    return directory, manifest


def smoke_code_mode(args):
    with locked(args.root,read_only=True):
        directory,manifest=release(args.root,args.release,verify=True)
        assets=manifest['harnesses']['codex'].get('assets')
        if not assets:raise ValueError('complete_codex_distribution_required')
        host=asset_path(directory,'codex-package/bin/codex-code-mode-host')
        with tempfile.TemporaryDirectory(prefix='zuse-code-mode-smoke.') as home, tempfile.TemporaryFile() as error:
            os.chmod(home,0o700)
            child=subprocess.Popen([str(host),'--listen','stdio'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=error,bufsize=0,start_new_session=True,env={'PATH':'/usr/bin:/bin','HOME':home})
            deadline=time.monotonic()+15
            def send(value):
                raw=canonical(value);child.stdin.write(struct.pack('<I',len(raw))+raw);child.stdin.flush()
            def read(size):
                result=b''
                while len(result)<size:
                    remaining=deadline-time.monotonic()
                    if remaining<=0 or not select.select([child.stdout],[],[],remaining)[0]:raise ValueError('code_mode_smoke_timeout')
                    block=os.read(child.stdout.fileno(),size-len(result))
                    if not block:raise ValueError('code_mode_smoke_host_exited')
                    result+=block
                return result
            def receive():
                size=struct.unpack('<I',read(4))[0]
                if size>1048576:raise ValueError('code_mode_smoke_response_too_large')
                return json.loads(read(size))
            def operation_frame():
                for _ in range(8):
                    frame=receive()
                    if frame.get('type')=='cell/closed' and frame.get('sessionId')=='zuse-smoke':continue
                    return frame
                raise ValueError('code_mode_smoke_unexpected_frames')
            try:
                send(dict(type='connection/hello',supportedVersions=[1],requiredCapabilities=[],optionalCapabilities=[]))
                ready=receive()
                if ready.get('type')!='connection/ready' or ready.get('selectedVersion')!=1:raise ValueError('code_mode_smoke_protocol_mismatch')
                send(dict(type='operation/request',id=1,request=dict(method='session/open',sessionId='zuse-smoke')))
                opened=receive()
                if opened.get('id')!=1 or opened.get('result',{}).get('status')!='ok':raise ValueError('code_mode_smoke_session_failed')
                for identifier,source,negative in [(2,"const value=6*7;if(value!==42)throw new Error('ZUSE_BAD_ARITHMETIC')",False),(3,"throw new Error('ZUSE_EXPECTED_SMOKE_ERROR')",True)]:
                    send(dict(type='operation/request',id=identifier,request=dict(method='session/execute',sessionId='zuse-smoke',request=dict(tool_call_id='smoke-'+str(identifier),enabled_tools=[],source=source,yield_time_ms=1000,max_output_tokens=100))))
                    started=operation_frame()
                    if started.get('id')!=identifier or started.get('result',{}).get('status')!='ok':raise ValueError('code_mode_smoke_execute_failed')
                    result=operation_frame()
                    if result.get('type')!='execute/initialResponse' or result.get('id')!=identifier or result.get('result',{}).get('status')!='ok':raise ValueError('code_mode_smoke_result_invalid')
                    outcome=result.get('result',{}).get('value',{}).get('Result')
                    if not isinstance(outcome,dict) or (not negative and outcome.get('error_text') is not None) or (negative and 'ZUSE_EXPECTED_SMOKE_ERROR' not in str(outcome.get('error_text'))):raise ValueError('code_mode_smoke_evaluation_failed')
            finally:
                if child.poll() is None:os.killpg(child.pid,signal.SIGTERM)
                try:child.wait(timeout=3)
                except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=3)
        print(json.dumps(dict(status='passed',scope='credential-free-code-mode-local-evaluation',release=args.release,hostSha256=assets['codex-package/bin/codex-code-mode-host']['sha256'],positive=True,negative=True,modelCalled=False)))


def stage(args):
    if bool(args.grok) != bool(args.grok_version):
        raise ValueError("Grok executable and exact version must be supplied together")
    agents = AGENTS if args.grok else AGENTS[:2]
    for version in (getattr(args, f"{agent}_version") for agent in agents):
        if not VERSION.fullmatch(version):
            raise ValueError("exact harness versions required")
    if not re.fullmatch(r"[0-9a-f]{40}", args.adapter_revision):
        raise ValueError("exact adapter git revision required")
    if not VERSION.fullmatch(args.runtime_version) or not args.state_format:
        raise ValueError("runtime version and state format required")
    with locked(args.root):
        parent = args.root / "releases"
        parent.mkdir(exist_ok=True)
        parent.chmod(0o755)
        with tempfile.TemporaryDirectory(prefix=".candidate-", dir=parent) as temporary:
            directory = Path(temporary)
            manifest = {"schema": 2 if args.grok else 1, "platform": f"{platform.system().lower()}-{platform.machine()}",
                        "adapterRevision": args.adapter_revision, "runtimeVersion": args.runtime_version,
                        "stateFormat": args.state_format, "harnesses": {}}
            for provider in agents:
                source = getattr(args, provider).resolve(strict=True)
                if not source.is_file():
                    raise ValueError("harness executable must be a regular file")
                binary = directory / provider
                shutil.copyfile(source, binary)
                binary.chmod(0o555)
                version = getattr(args, f"{provider}_version")
                # No account, API key or HOME from the builder enters this probe.
                with tempfile.TemporaryDirectory() as home:
                    result = subprocess.run([str(binary), "--version"], check=True, capture_output=True,
                                            timeout=20, env={"PATH": "/usr/bin:/bin", "HOME": home,
                                            "DISABLE_AUTOUPDATER": "1", "DISABLE_UPDATES": "1"})
                reported = result.stdout.decode().strip()
                if not re.search(r"(?<![\w.])" + re.escape(version) + r"(?![\w.+-])", reported):
                    raise ValueError(f"{provider} version probe mismatch")
                manifest["harnesses"][provider] = {"version": version, "sha256": digest(binary)}
            assets_source=getattr(args,'codex_assets',None)
            if assets_source:
                assets=json.loads((assets_source/'assets.json').read_text())
                copy_assets(assets_source,directory,assets)
                manifest['harnesses']['codex'].update(assets=assets,executable='codex-package/bin/codex')
                complete_codex(directory,manifest)
            identifier = hashlib.sha256(canonical(manifest)).hexdigest()
            atomic_json(directory / "manifest.json", manifest)
            directory.chmod(0o755)
            # A release is visible only after every binary and the manifest exist.
            destination = parent / identifier
            if destination.exists():
                release(args.root, identifier, verify=True)
            else:
                os.rename(directory, destination)
            print(identifier)


def promote(args):
    with locked(args.root):
        _, manifest = release(args.root, args.release, verify=True)
        evidence = json.loads(args.evidence.read_text())
        provider = getattr(args, "provider", None)
        required = ((CHECKS - {"boxd", "boat"}) | {provider}
                    | ({"grok"} if manifest["schema"] == 2 else set())) if provider else (
                    CHECKS | ({"grok", "e2b", "daytona"} if manifest["schema"] == 2 else set()))
        if (evidence.get("release") != args.release
                or evidence.get("adapterRevision") != manifest["adapterRevision"]
                or evidence.get("stateFormat") != manifest["stateFormat"]
                or (provider is not None and evidence.get("provider") != provider)
                or any(evidence.get("checks", {}).get(check) is not True for check in required)):
            raise ValueError("passing compatibility/provider evidence required for this exact release")
        (args.root / "channels").mkdir(mode=0o755, exist_ok=True)
        pointer = args.root / "channels" / f"{args.channel}.json"
        current = json.loads(pointer.read_text()) if pointer.exists() else None
        previous = current["release"] if current else None
        certified = {provider} if provider else ({"boxd", "boat", "e2b", "daytona"} if manifest["schema"] == 2 else {"boxd", "boat"})
        if previous == args.release:
            certified.update(current.get("certifiedProviders", []))
            previous = current.get("previous")
        # A scoped receipt certifies only that provider, never an unavailable
        # provider. Keep each original receipt rather than overwriting another.
        receipt = (args.root / "attestations" / args.release / f"{provider}.json") if provider else (
                   args.root / "attestations" / f"{args.release}.json")
        atomic_json(receipt, evidence)
        atomic_json(pointer, {"release": args.release, "previous": previous,
                              "certifiedProviders": sorted(certified)})
        print(args.release)


def verify(args):
    with locked(args.root, read_only=True):
        release(args.root, args.release, verify=True)
        print(args.release)


def rollback(args):
    with locked(args.root):
        pointer = args.root / "channels" / f"{args.channel}.json"
        current = json.loads(pointer.read_text())
        previous = current.get("previous")
        release(args.root, previous, verify=True)
        atomic_json(pointer, {"release": previous, "previous": current["release"]})
        print(previous)


def environment(args):
    with locked(args.root, read_only=True):
        if args.pin and args.pin.exists():
            identifier = json.loads(args.pin.read_text())["release"]
        else:
            identifier = json.loads((args.root / "channels" / f"{args.channel}.json").read_text())["release"]
        directory, manifest = release(args.root, identifier)
        if args.runtime_version != manifest["runtimeVersion"]:
            raise ValueError("harness release/runtime version mismatch")
        # Bind a workspace across suspend/resume, including channel rollback.
        if args.pin and not args.pin.exists():
            atomic_json(args.pin, {"release": identifier})
        selected = {"CODEX_EXECUTABLE": str((directory / manifest["harnesses"]["codex"].get("executable","codex")).resolve()),
                "CLAUDE_CODE_EXECUTABLE": str((directory / "claude").resolve()),
                "DISABLE_AUTOUPDATER": "1", "DISABLE_UPDATES": "1",
                "ZUSE_HARNESS_RELEASE": identifier}
        if "grok" in manifest["harnesses"]:
            selected["GROK_EXECUTABLE"] = str((directory / "grok").resolve())
        for agent in manifest["harnesses"]:
            selected[f"ZUSE_AUTH_{agent.upper()}_BINARY"] = str((directory / manifest["harnesses"][agent].get("executable",agent)).resolve())
        return selected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path("/opt/zuse/harnesses"))
    commands = parser.add_subparsers(dest="action", required=True)
    smoke=commands.add_parser('smoke-code-mode')
    smoke.add_argument('--release',required=True)
    asset_check=commands.add_parser('verify-asset-archive')
    asset_check.add_argument('--manifest',type=Path,required=True)
    asset_check.add_argument('--archive',type=Path,required=True)
    asset_check.add_argument('--release',required=True)
    candidate = commands.add_parser("stage")
    for provider in ("claude", "codex"):
        candidate.add_argument(f"--{provider}", type=Path, required=True)
        candidate.add_argument(f"--{provider}-version", required=True)
    candidate.add_argument("--codex-assets", type=Path, help="signature/SRI-verified complete vendor asset tree")
    candidate.add_argument("--grok", type=Path)
    candidate.add_argument("--grok-version")
    candidate.add_argument("--adapter-revision", required=True)
    candidate.add_argument("--runtime-version", required=True)
    candidate.add_argument("--state-format", required=True)
    verifier = commands.add_parser("verify")
    verifier.add_argument("--release", required=True)
    promote_parser = commands.add_parser("promote")
    promote_parser.add_argument("--release", required=True)
    promote_parser.add_argument("--evidence", type=Path, required=True)
    promote_parser.add_argument("--provider", choices=("boxd", "boat", "e2b", "daytona"),
                                help="certify only this provider; requires a matching provider-scoped receipt")
    for command in (promote_parser, commands.add_parser("rollback"), commands.add_parser("exec")):
        command.add_argument("--channel", choices=sorted(CHANNELS), default="staging")
    launch = commands.choices["exec"]
    launch.add_argument("--pin", type=Path)
    launch.add_argument("--runtime-version", required=True)
    launch.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action=='smoke-code-mode':
        smoke_code_mode(args);return
    if args.action=='verify-asset-archive':
        manifest=json.loads(args.manifest.read_text())
        if hashlib.sha256(canonical(manifest)).hexdigest()!=args.release:raise ValueError('harness_manifest_digest_mismatch')
        verify_asset_archive(args.archive,manifest['harnesses']['codex'].get('assets'))
        print('distribution-archive-verified');return
    args.root = args.root.resolve()
    if args.action == "exec":
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command:
            parser.error("exec requires a command")
        env = os.environ.copy()
        env.update(environment(args))
        os.execvpe(command[0], command, env)
    else:
        {"stage": stage, "verify": verify, "promote": promote, "rollback": rollback}[args.action](args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        # Never print captured executable output or the builder's environment.
        raise SystemExit(f"harness release failed: {type(error).__name__}") from None
