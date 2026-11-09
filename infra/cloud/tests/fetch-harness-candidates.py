#!/usr/bin/env python3
import base64
import io
import tarfile
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT=Path(__file__).resolve().parents[1]/'fetch-harness-candidates.py'
spec=importlib.util.spec_from_file_location('fetch_candidates',SCRIPT)
tool=importlib.util.module_from_spec(spec);spec.loader.exec_module(tool)

class SignatureTest(unittest.TestCase):
    def test_signed_package_identity_version_integrity_are_bound(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            subprocess.run(['openssl','genpkey','-algorithm','EC','-pkeyopt','ec_paramgen_curve:P-256','-out',str(root/'private')],check=True,capture_output=True)
            public=subprocess.run(['openssl','pkey','-in',str(root/'private'),'-pubout','-outform','DER'],check=True,capture_output=True).stdout
            metadata={'name':'@fixture/native','version':'1.2.3','dist':{'integrity':'sha512-fixture'}}
            signature=subprocess.run(['openssl','dgst','-sha256','-sign',str(root/'private')],input=b'@fixture/native@1.2.3:sha512-fixture',check=True,capture_output=True).stdout
            metadata['dist']['signatures']=[{'keyid':'fixture','sig':base64.b64encode(signature).decode()}]
            keys=[{'keyid':'fixture','key':base64.b64encode(public).decode()}]
            self.assertEqual(tool.verify_signature(metadata,keys),'fixture')
            for field in ('name','version'):
                changed=dict(metadata);changed[field]='tampered'
                with self.assertRaises(ValueError):tool.verify_signature(changed,keys)
            metadata['dist']['integrity']='sha512-tampered'
            with self.assertRaises(ValueError):tool.verify_signature(metadata,keys)
            with self.assertRaises(ValueError):tool.verify_signature(metadata,[])

class DistributionTest(unittest.TestCase):
    def test_exact_vendor_layout_and_missing_companion_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);archive=root/'vendor.tar'
            names=['package/vendor/test/bin/codex','package/vendor/test/bin/codex-code-mode-host','package/vendor/test/codex-resources/bwrap','package/vendor/test/codex-path/rg','package/vendor/test/codex-package.json','package/package.json','package/README.md']
            with tarfile.open(archive,'w') as out:
                for name in names:
                    info=tarfile.TarInfo(name);info.size=4;out.addfile(info,io.BytesIO(b'test'))
            with tarfile.open(archive) as source:
                pins=tool.codex_assets(source,source.getmembers(),root/'valid',names[0])
                self.assertIn('codex-package/bin/codex-code-mode-host',pins)
                self.assertNotIn('codex-package/bin/codex',pins)
            with tarfile.open(archive) as source:
                with self.assertRaisesRegex(ValueError,'incomplete'):
                    tool.codex_assets(source,[m for m in source.getmembers() if not m.name.endswith('/rg')],root/'missing',names[0])
            with tarfile.open(archive) as source:
                extra=tarfile.TarInfo('package/vendor/test/../../escape')
                with self.assertRaisesRegex(ValueError,'unsafe'):
                    tool.codex_assets(source,[extra],root/'unsafe',names[0])

if __name__=='__main__':unittest.main()
