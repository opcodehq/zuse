import importlib.util,tempfile,unittest,json,sys,io
from pathlib import Path
from unittest.mock import patch
s=importlib.util.spec_from_file_location('unit',Path(__file__).resolve().parents[1]/'e2b-publication-unit.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class UnitPlan(unittest.TestCase):
 def fixture(self,root):
  node=root/'node';node.write_bytes(b'node-pin');publisher=root/'build-e2b-managed-base.mjs';publisher.write_text('// pinned');launcher=root/'run-e2b-publication.py';launcher.write_text('# pinned');sdk=root/'sdk.mjs';sdk.write_text('// pinned SDK');tmp=root/'tmp';tmp.mkdir(mode=0o700)
  path=root/'plan.json';path.write_text(json.dumps(dict(command=[str(node),'--max-old-space-size=256',str(publisher),'--bundle',str(root/'bundle'),'--base-image','ubuntu@sha256:'+'a'*64,'--output',str(root/'report'),'--retain-artifact'],nodeSha256=m.sha(node),sourceFiles={publisher.name:m.sha(publisher),launcher.name:m.sha(launcher)},sdkPins={str(sdk):m.sha(sdk)},sdk=str(sdk),tmpdir=str(tmp))))
  return path
 def test_default_plan_does_not_start_unit_or_read_private_key(self):
  with tempfile.TemporaryDirectory() as d:
   path=self.fixture(Path(d));stdout=io.StringIO()
   with patch.object(sys,'argv',['unit','--launcher-plan',str(path)]),patch.object(sys,'stdout',stdout),patch.object(m.subprocess,'run',side_effect=AssertionError('no effects')):m.main()
   value=json.loads(stdout.getvalue());self.assertEqual(value['properties']['MemoryMax'],'402653184');self.assertEqual(value['properties']['MemorySwapMax'],'0');self.assertEqual(value['properties']['CPUQuota'],'50%');self.assertEqual(value['properties']['TasksMax'],'128');self.assertFalse(value['keyInArgv']);self.assertNotIn('E2B_API_KEY=',json.dumps(value['command']));self.assertFalse(value['providerEffectsPerformed'])
 def test_exact_reduced_cgroup_cap_is_required(self):
  with tempfile.TemporaryDirectory() as d:
   value=m.plan_for(self.fixture(Path(d)));cg='/sys/fs/cgroup/system.slice/'+value['unitName']+'.service';rows={'/proc/self/cgroup':'0::/system.slice/'+value['unitName']+'.service\n',cg+'/memory.max':'402653184',cg+'/memory.swap.max':'0',cg+'/pids.max':'128',cg+'/cpu.max':'50000 100000'}
   with patch.object(Path,'read_text',autospec=True,side_effect=lambda p,*a,**k:rows[str(p)]):m.check_cgroup(value)
   rows[cg+'/memory.max']='536870912'
   with patch.object(Path,'read_text',autospec=True,side_effect=lambda p,*a,**k:rows[str(p)]),self.assertRaisesRegex(ValueError,'limits_missing'):m.check_cgroup(value)
 def test_new_profile_keeps_cpu_runtime_swap_and_no_key_in_argv(self):
  with tempfile.TemporaryDirectory() as d:
   value=m.plan_for(self.fixture(Path(d)));self.assertEqual(value['properties']['MemoryMax'],'402653184');self.assertEqual(value['properties']['MemorySwapMax'],'0');self.assertEqual(value['properties']['CPUQuota'],'50%');self.assertEqual(value['properties']['RuntimeMaxSec'],'1500');self.assertNotIn('SYNTHETIC_PRIVATE_KEY',json.dumps(value['command']))
 def test_changed_operator_source_rejects_before_launch(self):
  with tempfile.TemporaryDirectory() as d:
   path=self.fixture(Path(d));(Path(d)/'build-e2b-managed-base.mjs').write_text('// changed')
   with self.assertRaisesRegex(ValueError,'source_changed'):m.plan_for(path)
 def test_inside_requires_exact_actual_cgroup_before_key_read(self):
  with tempfile.TemporaryDirectory() as d:
   value=m.plan_for(self.fixture(Path(d)))
   with self.assertRaisesRegex(ValueError,'wrong_cgroup'):m.check_cgroup(value)
if __name__=='__main__':unittest.main()
