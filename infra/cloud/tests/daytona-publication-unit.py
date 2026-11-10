import importlib.util,unittest,tempfile,os,json,shutil,subprocess,hashlib
from pathlib import Path
from unittest.mock import patch
from types import SimpleNamespace
p=Path(__file__).resolve().parents[1]/'daytona-publication-unit.py';s=importlib.util.spec_from_file_location('unit',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Unit(unittest.TestCase):
 def test_limits_checked_before_private_key_is_opened(self):
  with patch.object(m,'check_cgroup',side_effect=ValueError('limits')),patch.object(m,'selected_key') as key:
   with self.assertRaisesRegex(ValueError,'limits'):m.inside_environment({'privateKeySource':'/never-read'})
   key.assert_not_called()
 def test_only_selected_provider_key_is_injected(self):
  with patch.object(m,'check_cgroup'),patch.object(m,'selected_key',return_value='synthetic-private'):
   self.assertEqual(m.inside_environment({'privateKeySource':'/private'}),{'PATH':'/usr/bin:/bin','DAYTONA_API_KEY':'synthetic-private'})
 def test_actual_cgroup_requires_exact_unit_and_all_caps(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);member=root/'membership';member.write_text('0::/system.slice/zuse-test.service\n');cg=root/'system.slice/zuse-test.service';cg.mkdir(parents=True)
   values={'memory.max':'536870912','memory.swap.max':'0','pids.max':'128','cpu.max':'50000 100000','memory.oom.group':'1'}
   for n,v in values.items():(cg/n).write_text(v)
   call=lambda:m.check_cgroup({'unitName':'zuse-test'},membership=member,hierarchy=root)
   call()
   for n,bad in [('memory.max','max'),('memory.swap.max','max'),('pids.max','max'),('cpu.max','max 100000'),('memory.oom.group','0')]:
    (cg/n).write_text(bad)
    with self.assertRaises(ValueError):call()
    (cg/n).write_text(values[n])
   member.write_text('0::/other.service\n')
   with self.assertRaisesRegex(ValueError,'wrong_cgroup'):call()
 def test_key_file_selects_one_key_and_rejects_duplicate_or_symlink(self):
  with tempfile.TemporaryDirectory() as d:
   path=Path(d)/'private.env';path.write_text('DATABASE_URL=synthetic-db\nBOAT_API_KEY=synthetic-other\nDAYTONA_API_KEY="synthetic-daytona"\n');path.chmod(0o600)
   real_stat=os.fstat
   def root_stat(fd):
    st=real_stat(fd);return SimpleNamespace(st_uid=0,st_mode=st.st_mode,st_nlink=st.st_nlink)
   with patch.object(m.os,'fstat',side_effect=root_stat):
    self.assertEqual(m.selected_key(path),'synthetic-daytona')
    path.write_text('DAYTONA_API_KEY=first\nDAYTONA_API_KEY=second\n')
    with self.assertRaisesRegex(ValueError,'key_missing'):m.selected_key(path)
    link=Path(d)/'link';link.symlink_to(path)
    with self.assertRaises(OSError):m.selected_key(link)
 def test_context_mode_is_bound_into_actual_unit_command(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);node=Path('/usr/bin/python3');path=root/'plan.json';command=[str(node),'--max-old-space-size=192',str(root/'build-daytona-managed-base.mjs'),'--bundle',str(root/'bundle'),'--base-image','ubuntu@sha256:'+'a'*64,'--output',str(root/'report'),'--context-mode','verified-bundle-v1']
   path.write_text(json.dumps({'command':command,'sourceFiles':{},'sdkPins':{},'nodeSha256':m.sha(node),'sdk':str(root/'sdk')}));result=m.plan_for(path);self.assertEqual(result['command'][-2:],['--context-mode','verified-bundle-v1']);self.assertFalse(result['providerEffectsPerformed'])
   command[-1]='unverified';path.write_text(json.dumps({'command':command,'sourceFiles':{},'sdkPins':{},'nodeSha256':m.sha(node),'sdk':str(root/'sdk')}))
   with self.assertRaisesRegex(ValueError,'context_mode_invalid'):m.plan_for(path)
 def test_actual_dry_launcher_binds_direct_context_and_complete_import_closure(self):
  node=shutil.which('node')
  if not node:self.skipTest('node unavailable')
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);bundle=root/'bundle';bundle.mkdir();files=[]
   for name in ('runtime','claude','codex','grok','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json','codex-assets.tar'):
    body=('# synthetic validator; never certification'if name=='harness-release.py'else'all|layout|prepare layout-receipt.json'if name=='managed-base-install.sh'else'synthetic '+name).encode();(bundle/name).write_bytes(body);files.append({'path':name,'bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()})
   (bundle/'bundle.json').write_text(json.dumps({'schema':1,'status':'candidate-only','platform':'linux-x86_64','releaseId':'zuse-base-'+'a'*32,'harnessRelease':'b'*64,'runtimeSha256':files[0]['sha256'],'files':files}))
   sdk=root/'dependencies/node_modules/@daytona/sdk';sdk.mkdir(parents=True);(sdk/'esm').mkdir();(sdk/'package.json').write_text('{"name":"@daytona/sdk","version":"0.227.0"}');(sdk/'esm/index.js').write_text('// offline synthetic pin only');(sdk.parents[2]/'package-lock.json').write_text('{}');output=root/'report.json';source=p.parent
   command=['python3',str(source/'run-daytona-publication.py'),'--node',node,'--node-sha256',m.sha(Path(node)),'--publisher',str(source/'build-daytona-managed-base.mjs'),'--bundle',str(bundle),'--base-image','ubuntu@sha256:'+'c'*64,'--sdk',str(sdk),'--output',str(output),'--context-mode','verified-bundle-v1']
   result=subprocess.run(command,capture_output=True,text=True,timeout=15,env={'PATH':os.environ['PATH']});self.assertEqual(result.returncode,0,result.stdout+result.stderr);value=json.loads(result.stdout);self.assertFalse(value['providerEffectsPerformed']);self.assertEqual(value['plan']['providerPlan']['contextMode'],'verified-bundle-v1');self.assertIn('daytona-publication-context.mjs',value['plan']['sourceFiles']);self.assertIn('base-supervisor-hygiene.mjs',value['plan']['sourceFiles']);unit=m.plan_for(Path(str(output)+'.plan.json'));self.assertEqual(unit['command'][-2:],['--context-mode','verified-bundle-v1']);self.assertFalse(output.exists())
 def test_no_whole_environment_file_or_secret_arguments(self):
  source=p.read_text();self.assertNotIn('EnvironmentFile',source);self.assertNotIn("os.environ['DAYTONA_API_KEY']",source)
if __name__=='__main__':unittest.main()
