import importlib.util,subprocess,tempfile,unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('launcher',Path(__file__).resolve().parents[1]/'run-e2b-publication.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class Launcher(unittest.TestCase):
 def test_deadline_kills_only_owned_process_group_and_never_reexecutes(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);sibling=subprocess.Popen(['/bin/sleep','30'],start_new_session=True)
   try:
    result=m.bounded(['/usr/bin/python3','-c',f'import time;open({str(root/"once")!r},"x").write("once");time.sleep(30)'],{'PATH':'/usr/bin:/bin'},root/'private.log',.1)
    self.assertTrue(result['timedOut']);self.assertIsNotNone(result['exitCode']);self.assertIsNone(sibling.poll());self.assertEqual((root/'once').read_text(),'once')
   finally:sibling.terminate();sibling.wait()
 def test_nonzero_exit_reaps_original_sdk_observer_before_returning(self):
  import os,time
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);sibling=subprocess.Popen(['/bin/sleep','30'],start_new_session=True)
   try:
    code=f"import subprocess,sys;child=subprocess.Popen(['/bin/sleep','30']);open({str(root/'observer')!r},'w').write(str(child.pid));sys.exit(3)"
    result=m.bounded(['/usr/bin/python3','-c',code],{'PATH':'/usr/bin:/bin'},root/'private.log',2)
    self.assertEqual(result['exitCode'],3);self.assertFalse(result['timedOut']);self.assertIsNone(sibling.poll())
    pid=int((root/'observer').read_text());self.assertFalse(Path('/proc/'+str(pid)).exists(),'original observer was adopted and reaped before reconciliation')
   finally:sibling.terminate();sibling.wait()
 def test_intent_is_private_durable_and_cannot_be_replaced(self):
  with tempfile.TemporaryDirectory() as d:
   path=Path(d)/'intent';m.save_once(path,{'name':'unique','operationId':'original'})
   self.assertEqual(path.stat().st_mode&0o777,0o600)
   with self.assertRaises(FileExistsError):m.save_once(path,{'name':'replacement'})
   self.assertIn('original',path.read_text())
 def test_full_launcher_reconciles_original_uncertain_attempt_without_retry(self):
  import json,os,shutil,sys
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);node=Path(shutil.which('node')).resolve();publisher=root/'build-e2b-managed-base.mjs'
   for name in m.SOURCE_FILES:(root/name).write_text('# synthetic operator dependency\n')
   publisher.write_text("""import fs from 'node:fs';const args=process.argv.slice(2),get=k=>args[args.indexOf(k)+1];const output=get('--output');if(args.includes('--reconcile')){const r=JSON.parse(fs.readFileSync(output));console.log(JSON.stringify({operationId:r.operationId,name:r.name,mutationsPerformed:false,cleanupVerified:false}));}else if(args.includes('--live')){const i=JSON.parse(fs.readFileSync(output+'.intent.json'));if(i.operationId!==get('--operation-id'))throw Error('intent missing');fs.writeFileSync(output+'.once','original',{flag:'wx'});fs.writeFileSync(output,JSON.stringify({...i,status:'cleanup-required',cleanupVerified:false}));process.exitCode=1;}else{console.log(JSON.stringify({status:'plan-only',provider:'e2b'}));}""")
   sdk=root/'sdk/node_modules/e2b/dist/index.mjs';sdk.parent.mkdir(parents=True);sdk.write_text('// synthetic SDK');(sdk.parent.parent/'package.json').write_text('{}');(root/'sdk/package-lock.json').write_text('{}')
   output=root/'report.json';tmpdir=root/'tmp';tmpdir.mkdir(mode=0o700);command=[sys.executable,str(Path(m.__file__).resolve()),'--node',str(node),'--node-sha256',m.digest(node),'--publisher',str(publisher),'--bundle',str(root/'bundle'),'--base-image','ubuntu@sha256:'+'a'*64,'--sdk',str(sdk),'--output',str(output),'--tmpdir',str(tmpdir)]
   result=subprocess.run(command,capture_output=True,text=True,timeout=10);self.assertEqual(result.returncode,0,result.stdout);plan=json.loads(result.stdout);self.assertEqual(plan['plan']['command'][1],'--max-old-space-size=256')
   env=dict(os.environ,E2B_API_KEY='SYNTHETIC_PRIVATE_KEY')
   live=command+['--live','--plan-sha256',plan['planSha256']];result=subprocess.run(live,env=env,capture_output=True,text=True,timeout=10)
   self.assertEqual(result.returncode,1);outcome=json.loads(result.stdout);self.assertEqual(outcome['status'],'cleanup-required');self.assertFalse(outcome['cleanupVerified'])
   original=json.loads(Path(str(output)+'.intent.json').read_text());reconciled=json.loads(Path(str(output)+'.reconcile.json').read_text());self.assertEqual(original['operationId'],reconciled['operationId']);self.assertEqual(original['name'],reconciled['name']);self.assertFalse(reconciled['mutationsPerformed'])
   again=subprocess.run(live,env=env,capture_output=True,text=True,timeout=10);self.assertEqual(again.returncode,1);self.assertEqual(Path(str(output)+'.once').read_text(),'original')
   for path in root.glob('report.json*'):
    if path.is_file():self.assertNotIn('SYNTHETIC_PRIVATE_KEY',path.read_text())
if __name__=='__main__':unittest.main()
