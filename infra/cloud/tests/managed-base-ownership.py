import json,subprocess,unittest
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'managed-base-ownership.py'
class Ownership(unittest.TestCase):
 def child(self,case):
  code='''import importlib.util,json,tempfile,os,stat
from pathlib import Path
s=importlib.util.spec_from_file_location('ownership',os.environ['OWNERSHIP_SCRIPT']);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
with tempfile.TemporaryDirectory(prefix='zuse-owned-ancestor-') as d:
 root=Path(d);path=root/'opt';path.mkdir(mode=0o755);inside=path/'program';inside.write_text('unchanged');inside.chmod(0o555);os.chown(path,1000,1000)
 case=os.environ['CASE'];rejected=False
 if case=='writable':path.chmod(0o775)
 elif case=='wrong-owner':os.chown(path,10001,10001)
 elif case=='symlink':path.rename(root/'actual');path.symlink_to(root/'actual')
 try:receipt=m.harden(path,anchor=root)
 except (ValueError,OSError):rejected=True
 if case=='valid':
  assert not rejected and path.stat().st_uid==0 and path.stat().st_gid==0
  assert receipt['before']['uid']==1000 and receipt['after']['mode']=='0o755' and not receipt['recursive']
  assert inside.read_text()=='unchanged' and stat.S_IMODE(inside.stat().st_mode)==0o555
  assert m.harden(path,anchor=root)['after']['uid']==0
 else:assert rejected
 print(json.dumps(dict(case=case,rejected=rejected)))
'''
  # An actual privileged isolated filesystem fixture; never touches host /opt.
  r=subprocess.run(['sudo','-n','env','OWNERSHIP_SCRIPT='+str(SCRIPT),'CASE='+case,'python3','-I','-c',code],capture_output=True,text=True,timeout=10)
  self.assertEqual(r.returncode,0,r.stderr);return json.loads(r.stdout)
 def test_full_parent_preflight_matches_provider_owned_ancestors_before_any_payload(self):
  fixture=SCRIPT.parent/'tests/managed-base-derive.py'
  code="""import importlib.util,os,tempfile,json
from pathlib import Path
s=importlib.util.spec_from_file_location('fixture',os.environ['FIXTURE_SCRIPT']);f=importlib.util.module_from_spec(s);s.loader.exec_module(f)
s=importlib.util.spec_from_file_location('ownership',os.environ['OWNERSHIP_SCRIPT']);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
with tempfile.TemporaryDirectory(dir='/root',prefix='zuse-parent-parity-') as d:
 root=Path(d);x=f.fixture(root);image=x['image'];(x['input']/'runtime').unlink()
 paths=['opt','usr/local','usr/local/bin','usr/local/lib']
 for name in paths:os.chown(image/name,1000,1000)
 args=dict(filesystem=image,owner=0,ping=dict(version=1,bound=False,childAlive=False,recoveryAlive=False))
 before=f.D.verify_parent(x['input'],x['expected'],**args)
 assert before['status']=='rejected' and {v['path'] for v in before['failures']}=={'/'+name for name in paths}
 for name in paths:m.harden(image/name,anchor=image)
 after=f.D.verify_parent(x['input'],x['expected'],**args)
 assert after['status']=='passed' and after['completeCodexAssetsVerified'] and not (x['input']/'runtime').exists()
 os.chown(image/'usr/local/lib/zuse',1000,1000)
 unknown=f.D.verify_parent(x['input'],x['expected'],**args)
 assert unknown['status']=='rejected' and {v['path'] for v in unknown['failures']}=={'/usr/local/lib/zuse'}
 print(json.dumps(dict(beforeProtected=False,knownAncestorsProtected=True,completeAssetsVerified=True,unknownAncestorRejected=True,newRuntimeUploaded=False)))
"""
  r=subprocess.run(['sudo','-n','env','OWNERSHIP_SCRIPT='+str(SCRIPT),'FIXTURE_SCRIPT='+str(fixture),'python3','-I','-c',code],capture_output=True,text=True,timeout=15)
  self.assertEqual(r.returncode,0,r.stderr);self.assertTrue(json.loads(r.stdout)['unknownAncestorRejected'])
 def test_actual_root_claim_is_nonrecursive_and_idempotent(self):self.assertFalse(self.child('valid')['rejected'])
 def test_reject_writable_ancestor(self):self.assertTrue(self.child('writable')['rejected'])
 def test_reject_unexpected_owner(self):self.assertTrue(self.child('wrong-owner')['rejected'])
 def test_reject_symlink(self):self.assertTrue(self.child('symlink')['rejected'])
if __name__=='__main__':unittest.main()
