#!/usr/bin/env python3
import importlib.util
import json
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "harness-release.py"
spec = importlib.util.spec_from_file_location("harness_release", SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class HarnessReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.root = self.directory / "managed"

    def run_tool(self, *args, okay=True):
        result = subprocess.run(["python3", str(SCRIPT), "--root", str(self.root), *map(str, args)], capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, okay, result.stderr)
        return result

    def stage(self, version="1.2.3", grok=False):
        # Offline synthetic executables: no upstream downloads or model credentials.
        binary = self.directory / f"fixture-{version}"
        binary.write_text(f"#!/bin/sh\nprintf '%s\\n' 'test-cli {version}'\n")
        binary.chmod(0o755)
        return self.run_tool("stage", *(["--grok", binary, "--grok-version", version] if grok else []),
                             "--claude", binary, "--codex", binary,
                             "--claude-version", version, "--codex-version", version,
                             "--adapter-revision", "a" * 40, "--runtime-version", "0.25.1",
                             "--state-format", "test-state-v1").stdout.strip()

    def stage_assets(self):
        import hashlib
        assets=self.directory/'assets';pins={}
        for name in release.CODEX_REQUIRED:
            path=assets/name;path.parent.mkdir(parents=True,exist_ok=True)
            path.write_bytes(b'#!/bin/sh\nexit 0\n' if '/bin/' in name or name.endswith('/bwrap') or '/codex-path/' in name else b'{}')
            executable=path.read_bytes().startswith(b'#!');path.chmod(0o555 if executable else 0o444)
            pins[name]={'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'bytes':path.stat().st_size,'executable':executable}
        (assets/'assets.json').write_text(json.dumps(pins))
        binary=self.directory/'primary';binary.write_text('#!/bin/sh\necho test-cli 0.166.0\n');binary.chmod(0o555)
        identifier=self.run_tool('stage','--claude',binary,'--codex',binary,'--grok',binary,'--claude-version','0.166.0','--codex-version','0.166.0','--grok-version','0.166.0','--codex-assets',assets,'--adapter-revision','a'*40,'--runtime-version','0.25.1','--state-format','fixture-v1').stdout.strip()
        return identifier,assets,pins

    def test_complete_distribution_is_pinned_missing_tamper_and_ancestor_symlink_rejected(self):
        identifier,assets,pins=self.stage_assets()
        directory,manifest=release.release(self.root,identifier,verify=True)
        self.assertEqual(manifest['harnesses']['codex']['executable'],'codex-package/bin/codex')
        args=type('Args',(),dict(root=self.root,pin=self.directory/'pin',channel='staging',runtime_version='0.25.1'))()
        args.pin.write_text(json.dumps({'release':identifier}))
        self.assertEqual(release.environment(args)['CODEX_EXECUTABLE'],str(directory/'codex-package/bin/codex'))
        host=directory/'codex-package/bin/codex-code-mode-host';original=host.read_bytes();host.chmod(0o755);host.write_bytes(b'x'*len(original))
        with self.assertRaisesRegex(ValueError,'integrity'):release.release(self.root,identifier,verify=True)
        host.unlink()
        with self.assertRaises(ValueError):release.release(self.root,identifier)
        host.symlink_to(assets/'codex-package/bin/codex-code-mode-host')
        with self.assertRaisesRegex(ValueError,'symlink'):release.release(self.root,identifier)
        with self.assertRaisesRegex(ValueError,'unsafe'):release.asset_path(directory,'codex-package/../../outside')

    def test_distribution_archive_rejects_extra_links_and_missing_members(self):
        import tarfile
        identifier,assets,pins=self.stage_assets()
        archive=self.directory/'assets.tar';release.archive_assets(assets,pins,archive)
        destination=self.directory/'installed';destination.mkdir()
        release.install_assets(archive,destination,pins);release.validate_assets(destination,pins)
        tampered=self.directory/'tampered.tar'
        with tarfile.open(tampered,'w') as target:
            for index,name in enumerate(pins):
                data=(assets/name).read_bytes()
                if index==0:data=bytes([data[0]^1])+data[1:]
                info=tarfile.TarInfo(name);info.size=len(data);target.addfile(info,io.BytesIO(data))
        target=self.directory/'dest-tampered';target.mkdir()
        with self.assertRaisesRegex(ValueError,'integrity'):release.install_assets(tampered,target,pins)
        self.assertEqual(list(target.iterdir()),[])
        for mode in ('symlink','missing','traversal'):
            bad=self.directory/(mode+'.tar')
            with tarfile.open(bad,'w') as target:
                for index,name in enumerate(pins):
                    if mode=='missing' and index==0:continue
                    info=tarfile.TarInfo('../outside' if mode=='traversal' and index==0 else name)
                    if mode=='symlink' and index==0:info.type=tarfile.SYMTYPE;info.linkname='/etc/passwd'
                    target.addfile(info)
            target=self.directory/('dest-'+mode);target.mkdir()
            with self.assertRaises(ValueError):release.install_assets(bad,target,pins)

    def evidence(self, identifier, passing=True):
        evidence = self.directory / "evidence.json"
        evidence.write_text(json.dumps({"release": identifier, "adapterRevision": "a" * 40,
                                       "stateFormat": "test-state-v1", "checks": {name: passing for name in release.CHECKS}}))
        return evidence

    def test_failed_evidence_never_changes_channel_and_binary_tampering_is_rejected(self):
        identifier = self.stage()
        self.run_tool("promote", "--release", identifier, "--evidence", self.evidence(identifier, False), okay=False)
        self.assertFalse((self.root / "channels/staging.json").exists())
        binary = self.root / "releases" / identifier / "codex"
        binary.chmod(0o755)
        binary.write_text("tampered")
        self.run_tool("promote", "--release", identifier, "--evidence", self.evidence(identifier), okay=False)

    def test_resume_keeps_pinned_release_across_promotion_and_rollback(self):
        first = self.stage()
        self.run_tool("promote", "--release", first, "--evidence", self.evidence(first))
        pin = self.directory / "workspace/release.json"
        args = ["exec", "--runtime-version", "0.25.1", "--pin", pin, "--", "python3", "-c",
                "import json,os;print(json.dumps({k:os.environ[k] for k in ['CODEX_EXECUTABLE','CLAUDE_CODE_EXECUTABLE','DISABLE_UPDATES','ZUSE_HARNESS_RELEASE']}))"]
        selected = json.loads(self.run_tool(*args).stdout)
        self.assertEqual(selected["ZUSE_HARNESS_RELEASE"], first)
        self.assertEqual(selected["DISABLE_UPDATES"], "1")
        self.assertTrue(Path(selected["CODEX_EXECUTABLE"]).is_absolute())
        second = self.stage("1.2.4")
        self.run_tool("promote", "--release", second, "--evidence", self.evidence(second))
        self.assertEqual(json.loads(self.run_tool(*args).stdout)["ZUSE_HARNESS_RELEASE"], first)
        # A new computer selects the latest tested channel; the old one keeps its executable.
        args[4] = self.directory / "new-workspace/release.json"
        self.assertEqual(json.loads(self.run_tool(*args).stdout)["ZUSE_HARNESS_RELEASE"], second)
        self.run_tool("rollback")
        self.assertEqual(json.loads((self.root / "channels/staging.json").read_text())["release"], first)
        self.assertTrue((self.root / "releases" / second / "codex").exists())

    def test_runtime_mismatch_and_missing_pin_fail_without_fallback(self):
        identifier = self.stage()
        self.run_tool("promote", "--release", identifier, "--evidence", self.evidence(identifier))
        self.run_tool("exec", "--runtime-version", "0.99.0", "--", "/bin/true", okay=False)
        pin = self.directory / "bad-pin.json"
        pin.write_text(json.dumps({"release": "../outside"}))
        self.run_tool("exec", "--runtime-version", "0.25.1", "--pin", pin, "--", "/bin/true", okay=False)

    def test_partial_candidate_cannot_activate_and_restage_is_idempotent(self):
        identifier = self.stage()
        self.assertEqual(self.stage(), identifier)
        binary = self.root / "releases" / identifier / "claude"
        binary.unlink()
        self.run_tool("promote", "--release", identifier, "--evidence", self.evidence(identifier), okay=False)

    def test_provider_scoped_promotion_does_not_certify_restricted_providers(self):
        identifier = self.stage(grok=True)
        evidence = self.evidence(identifier)
        data = json.loads(evidence.read_text())
        data['checks'].pop('boat')
        data['checks']['grok'] = True
        data['provider'] = 'boxd'
        evidence.write_text(json.dumps(data))
        self.run_tool('promote','--provider','boxd','--release',identifier,'--evidence',evidence)
        pointer = json.loads((self.root/'channels/staging.json').read_text())
        self.assertEqual(pointer['certifiedProviders'], ['boxd'])
        self.assertTrue((self.root/'attestations'/identifier/'boxd.json').is_file())
        self.run_tool('promote','--provider','boat','--release',identifier,'--evidence',evidence,okay=False)
        data['provider'] = 'boat'
        data['checks'].pop('boxd')
        data['checks']['boat'] = True
        evidence.write_text(json.dumps(data))
        self.run_tool('promote','--provider','boat','--release',identifier,'--evidence',evidence)
        pointer = json.loads((self.root/'channels/staging.json').read_text())
        self.assertEqual(pointer['certifiedProviders'], ['boat','boxd'])
        self.assertIsNone(pointer['previous'])
        self.assertTrue((self.root/'attestations'/identifier/'boxd.json').is_file())
        self.assertTrue((self.root/'attestations'/identifier/'boat.json').is_file())

    def test_three_agent_release_requires_all_provider_evidence_and_exports_authority_paths(self):
        identifier = self.stage(grok=True)
        evidence = self.evidence(identifier)
        # The old two-provider receipt cannot certify a three-agent release.
        self.run_tool("promote", "--release", identifier, "--evidence", evidence, okay=False)
        data = json.loads(evidence.read_text())
        data["checks"].update(grok=True, e2b=True, daytona=True)
        evidence.write_text(json.dumps(data))
        self.run_tool("promote", "--release", identifier, "--evidence", evidence)
        result = self.run_tool("exec", "--runtime-version", "0.25.1", "--", "python3", "-c",
                              "import json,os;print(json.dumps({k:v for k,v in os.environ.items() if k.startswith('ZUSE_AUTH_') or k=='GROK_EXECUTABLE'}))")
        selected = json.loads(result.stdout)
        for agent in release.AGENTS:
            self.assertEqual(selected[f"ZUSE_AUTH_{agent.upper()}_BINARY"],
                             str(self.root / "releases" / identifier / agent))
        self.assertEqual(selected["GROK_EXECUTABLE"], selected["ZUSE_AUTH_GROK_BINARY"])
        grok = self.root / "releases" / identifier / "grok"
        grok.unlink()
        self.run_tool("exec", "--runtime-version", "0.25.1", "--", "/bin/true", okay=False)


if __name__ == "__main__":
    unittest.main()
