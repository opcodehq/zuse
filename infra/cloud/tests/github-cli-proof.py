import subprocess,unittest
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'github-cli-proof.py'
class GithubProof(unittest.TestCase):
 def fixture(self,case):
  code='''import importlib.util,tempfile,os,json,hashlib,subprocess
from pathlib import Path
s=importlib.util.spec_from_file_location('proof',os.environ['PROOF_SCRIPT']);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
with tempfile.TemporaryDirectory(prefix='zuse-gh-proof-') as d:
 root=Path(d);global_path=root/'opt/zuse/bin/gh';global_path.parent.mkdir(parents=True);global_path.write_bytes(m.WRAPPER);global_path.chmod(0o755)
 profile=root/'etc/profile.d/zz-zuse-managed-bin.sh';profile.parent.mkdir(parents=True);profile.write_bytes(m.PROFILE);profile.chmod(0o644)
 system=root/'usr/bin/gh';system.parent.mkdir(parents=True);system.write_bytes(b'fixture native gh bytes');system.chmod(0o755)
 expected=dict(version='gh version 2.45.0 (2025-01-01)',sha256=hashlib.sha256(system.read_bytes()).hexdigest());case=os.environ['CASE'];calls=[]
 if case in ('attested','wrapper-hash','mixed','false-resolution','unknown'):
  expected.update(wrapperSha256=hashlib.sha256(m.WRAPPER).hexdigest(),loginShellResolvedManaged=True)
 if case=='wrapper-hash':expected['wrapperSha256']='0'*64
 elif case=='mixed':del expected['loginShellResolvedManaged']
 elif case=='false-resolution':expected['loginShellResolvedManaged']=False
 elif case=='unknown':expected['extra']='untrusted'
 if case=='missing':global_path.unlink()
 elif case=='symlink':global_path.unlink();global_path.symlink_to(system)
 elif case=='content':global_path.write_bytes(b'wrong executable script')
 elif case=='writable':global_path.chmod(0o777)
 elif case=='system-hash':expected['sha256']='0'*64
 elif case=='ancestor-writable':global_path.parent.chmod(0o777)
 elif case=='ancestor-private':global_path.parent.chmod(0o700)
 elif case=='profile-missing':profile.unlink()
 elif case=='profile-content':profile.write_bytes(b'export PATH=/bad')
 elif case=='profile-writable':profile.chmod(0o666)

 def run(argv,**kwargs):
  calls.append(argv);assert kwargs['env']==m.ENV and 'GH_TOKEN' not in kwargs['env']
  assert kwargs['timeout']==5
  if case=='timeout-login' and argv[-1]=='command -v gh':raise subprocess.TimeoutExpired(argv,5,output='PRIVATE_OUTPUT',stderr='PRIVATE_ERROR')
  if case=='timeout-version' and argv[-1]=='--version':raise subprocess.TimeoutExpired(argv,5,output='PRIVATE_OUTPUT',stderr='PRIVATE_ERROR')
  if case=='os-error' and argv[-1]=='--version':raise OSError(13,'PRIVATE_ERROR')

  output=('/usr/bin/gh\\n' if case=='login-path' else '/opt/zuse/bin/gh\\n') if argv[-1]=='command -v gh' else expected['version']+'\\n'
  if case=='version' and argv[-1]=='--version':output='wrong version\\n'
  return subprocess.CompletedProcess(argv,0,stdout=output,stderr='')
 result=m.inspect(expected,filesystem=root,run=run)
 if case in ('valid','attested'):assert result['status']=='passed' and len(calls)==2 and result['globalShim']['contentMatches']
 else:assert result['status']=='rejected'
 if case in ('missing','symlink','content','writable','system-hash','wrapper-hash','mixed','false-resolution','unknown','ancestor-writable','profile-missing','profile-content','profile-writable'):assert not calls,'unsafe/missing shim must never be executed'
 assert 'PRIVATE_' not in json.dumps(result)
 print(json.dumps(dict(case=case,status=result['status'],code=result.get('code'),commands=len(calls),observations=result.get('probeObservations'))))
'''
  result=subprocess.run(['sudo','-n','env','PROOF_SCRIPT='+str(SCRIPT),'CASE='+case,'python3','-I','-c',code],capture_output=True,text=True,timeout=10)
  self.assertEqual(result.returncode,0,result.stderr);return __import__('json').loads(result.stdout)
 def test_exact_wrapper_and_system_receipt_with_fixed_user_probes(self):self.assertEqual(self.fixture('valid')['status'],'passed')
 def test_attested_receipt_matches_actual_fixed_shim(self):self.assertEqual(self.fixture('attested')['status'],'passed')
 def test_attested_mismatch_or_incomplete_metadata_never_executes(self):
  for case in ('wrapper-hash','mixed','false-resolution','unknown'):self.assertEqual(self.fixture(case)['commands'],0)
 def test_missing_shim_is_rejected_without_execution(self):self.assertEqual(self.fixture('missing')['commands'],0)
 def test_unsafe_files_are_not_executed(self):
  for case in ('symlink','content','writable','system-hash'):self.assertEqual(self.fixture(case)['commands'],0)
 def test_login_shell_and_version_must_match(self):
  for case in ('login-path','version'):self.assertEqual(self.fixture(case)['status'],'rejected')
 def test_controlled_ancestor_and_profile_are_mandatory(self):
  for case in ('ancestor-writable','ancestor-private','profile-missing','profile-content','profile-writable'):self.assertEqual(self.fixture(case)['commands'],0)
 def test_failure_stage_and_category_remain_factual_without_output(self):
  for case,stage,outcome in [('timeout-login','login-path','timeout'),('timeout-version','managed-version','timeout'),('os-error','managed-version','os-error')]:
   r=self.fixture(case);fact=r['observations'][-1];self.assertEqual((fact['stage'],fact['outcome']),(stage,outcome));self.assertGreaterEqual(fact['elapsedMs'],0)
   if case=='os-error':self.assertEqual(fact['errno'],'EACCES')
 def test_profile_preserves_task_prefix_and_repairs_login_path(self):
  import importlib.util,tempfile,os
  spec=importlib.util.spec_from_file_location('proof',SCRIPT);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);managed=root/'managed';private=root/'task';managed.mkdir();private.mkdir()
   for folder in (managed,private):
    (folder/'gh').write_text('#!/bin/sh\nexit 0\n');(folder/'gh').chmod(0o755)
   profile=root/'profile';profile.write_bytes(m.PROFILE.replace(b'/opt/zuse/bin',str(managed).encode()))
   for prefix,expected in [(str(private)+':'+str(managed),private),(str(managed),managed),('/usr/bin:/bin',managed)]:
    r=subprocess.run(['/bin/bash','--noprofile','--norc','-c','. "$1"; command -v gh','test',str(profile)],env={'PATH':prefix},capture_output=True,text=True,timeout=2)
    self.assertEqual(r.returncode,0,r.stderr);self.assertEqual(r.stdout.strip(),str(expected/'gh'))
 def test_actual_installer_payload_creates_exact_controlled_files(self):
  code=r"""import ast,importlib.util,os,tempfile,subprocess
from pathlib import Path
spec=importlib.util.spec_from_file_location('proof',os.environ['PROOF_SCRIPT']);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
source=Path(os.environ['INSTALLER']).read_text().split("python3 - <<'MANAGED_GH'\n",1)[1].split('\nMANAGED_GH',1)[0]
with tempfile.TemporaryDirectory() as d:
 root=Path(d);(root/'opt').mkdir();(root/'etc/profile.d').mkdir(parents=True)
 class Sandbox(ast.NodeTransformer):
  def visit_Call(self,node):
   self.generic_visit(node)
   if isinstance(node.func,ast.Name) and node.func.id=='Path' and node.args and isinstance(node.args[0],ast.Constant) and isinstance(node.args[0].value,str) and node.args[0].value.startswith('/'):
    node.args[0]=ast.Constant(d+node.args[0].value)
   return node
 tree=ast.fix_missing_locations(Sandbox().visit(ast.parse(source)))
 os.umask(0o077)
 exec(compile(tree,'actual-installer-payload','exec'),{})
 shim=root/'opt/zuse/bin/gh';profile=root/'etc/profile.d/zz-zuse-managed-bin.sh'
 assert shim.read_bytes()==m.WRAPPER and profile.read_bytes()==m.PROFILE
 assert shim.stat().st_uid==0 and shim.stat().st_mode&0o777==0o755 and profile.stat().st_mode&0o777==0o644
 for directory in (root/'opt/zuse',root/'opt/zuse/bin'):
  assert directory.stat().st_mode&0o777==0o755,'managed command directory must be traversable under installer umask077'
 root.chmod(0o755)
 assert subprocess.run(['sudo','-n','-u','nobody','/usr/bin/test','-x',str(shim)],capture_output=True,timeout=2).returncode==0,'unprivileged task user must reach the installed command'
"""
  installer=SCRIPT.parent/'managed-base-install.sh'
  r=subprocess.run(['sudo','-n','env','PROOF_SCRIPT='+str(SCRIPT),'INSTALLER='+str(installer),'python3','-I','-B','-c',code],capture_output=True,text=True,timeout=10)
  self.assertEqual(r.returncode,0,r.stderr)
if __name__=='__main__':unittest.main()
