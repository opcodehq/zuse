#!/usr/bin/env python3
"""Synthetic launcher tests; no credentials, accounts or model calls."""
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
canonical=lambda value:json.dumps(value,sort_keys=True,separators=(',',':')).encode()
class CandidateAuthorityTest(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);self.harness=self.root/'harnesses';self.harness.mkdir()
        binary=b'#!/bin/sh\nexit 0\n'
        manifest=dict(schema=2,platform='linux-'+platform.machine(),runtimeVersion='0.25.1',adapterRevision='a'*40,stateFormat='synthetic',harnesses={a:dict(version='1.2.3',sha256=hashlib.sha256(binary).hexdigest()) for a in ('claude','codex','grok')})
        self.identifier=hashlib.sha256(canonical(manifest)).hexdigest()
        release=self.harness/'releases'/self.identifier;release.mkdir(parents=True)
        for agent in manifest['harnesses']:(release/agent).write_bytes(binary);(release/agent).chmod(0o555)
        (release/'manifest.json').write_bytes(canonical(manifest));(self.harness/'.release-lock').touch()
        self.api=self.root/'api';self.api.write_text('#!/usr/bin/env python3\nimport os,json;print(json.dumps({"release":os.environ["ZUSE_HARNESS_RELEASE"]}))\n');self.api.chmod(0o755)
        self.launcher=self.root/'launch';self.launcher.write_text((ROOT/'launch-authority.sh').read_text().replace('/usr/local/lib/zuse/harness-release.py',str(ROOT/'harness-release.py')).replace('/usr/local/bin/zuse-cloud-api',str(self.api)))
        self.env={'PATH':os.environ['PATH'],'ZUSE_HARNESS_ROOT':str(self.harness),'ZUSE_AUTH_RUNTIME_VERSION':'0.25.1','ZUSE_DEPLOYMENT_ENVIRONMENT':'staging','ZUSE_AUTH_CANDIDATE_RELEASE':self.identifier,'ZUSE_AUTH_CANDIDATE_ACCOUNT_ID':'test-owner','ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS':'test-owner'}
    def run_launcher(self):return subprocess.run(['sh',str(self.launcher),'auth-worker','--check'],env=self.env,capture_output=True,text=True)
    def test_exact_candidate_boots_owner_without_promoted_channel(self):
        result=self.run_launcher();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['release'],self.identifier)
        self.assertFalse((self.harness/'channels').exists())
    def test_production_wildcard_or_broader_cohort_cannot_boot_candidate(self):
        for key,value in [('ZUSE_DEPLOYMENT_ENVIRONMENT','production'),('ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS','test-owner,other'),('ZUSE_AUTH_CANDIDATE_ACCOUNT_ID','*'),('ZUSE_CLOUD_CREATE_ACCOUNTS','other')]:
            previous=self.env.get(key);self.env[key]=value
            self.assertNotEqual(self.run_launcher().returncode,0,key)
            if previous is None:self.env.pop(key)
            else:self.env[key]=previous
    def test_missing_or_tampered_candidate_never_falls_back(self):
        self.env['ZUSE_AUTH_CANDIDATE_RELEASE']='b'*64
        self.assertNotEqual(self.run_launcher().returncode,0)
        self.env['ZUSE_AUTH_CANDIDATE_RELEASE']=self.identifier
        # Installed releases are read-only; an attacker with write access
        # would change that first.
        binary=self.harness/'releases'/self.identifier/'codex';binary.chmod(0o755);binary.write_bytes(b'tampered')
        self.assertNotEqual(self.run_launcher().returncode,0)
    def test_default_authority_does_not_bypass_missing_channel(self):
        self.env.pop('ZUSE_AUTH_CANDIDATE_RELEASE')
        self.assertNotEqual(self.run_launcher().returncode,0)

if __name__=='__main__':unittest.main()
