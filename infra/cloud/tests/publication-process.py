import importlib.util,unittest,subprocess,tempfile,os,time,sys,json
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'publication-process.py';s=importlib.util.spec_from_file_location('owner',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Ownership(unittest.TestCase):
 def test_parent_failure_retires_its_inherited_sdk_observer_but_not_sibling(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);marker=root/'original';sibling=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'],start_new_session=True)
   child="import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(30)"
   code="import subprocess,sys,json;from pathlib import Path;p=subprocess.Popen([sys.executable,'-c',%r]);Path(%r).write_text(json.dumps({'child':p.pid}));raise SystemExit(7)"%(child,str(marker))
   try:
    result=m.bounded([sys.executable,'-c',code],{'PATH':'/usr/bin:/bin'},root/'private.log',2,grace=.1)
    self.assertEqual(result['exitCode'],7);self.assertTrue(result['groupRetired']);pid=json.loads(marker.read_text())['child']
    with self.assertRaises(ProcessLookupError):os.kill(pid,0)
    os.kill(sibling.pid,0);self.assertIsNone(sibling.poll());self.assertEqual((root/'private.log').stat().st_mode&0o777,0o600)
   finally:sibling.terminate();sibling.wait()
 def test_deadline_reaps_term_ignoring_group_and_one_side_effect(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);marker=root/'effect'
   code="import signal,time;from pathlib import Path;Path(%r).write_text('1');signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(30)"%str(marker)
   result=m.bounded([sys.executable,'-c',code],{'PATH':'/usr/bin:/bin'},root/'private.log',.1,grace=.1)
   self.assertTrue(result['timedOut']);self.assertTrue(result['groupRetired']);self.assertEqual(marker.read_text(),'1')
 def test_aggregate_memory_guard_rejects_unbounded_and_swap_scopes(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);child=root/'operator';child.mkdir()
   (root/'memory.oom.group').write_text('1');(root/'memory.max').write_text('max');(root/'memory.swap.max').write_text('max');(child/'memory.max').write_text(str(512*1024*1024));(child/'memory.swap.max').write_text('0')
   self.assertEqual(m.memory_ceiling(child,hierarchy_root=root)['maxBytes'],512*1024*1024)
   (child/'memory.max').write_text('max')
   with self.assertRaisesRegex(RuntimeError,'512m_scope'):m.memory_ceiling(child,hierarchy_root=root)
   (root/'memory.max').write_text(str(512*1024*1024));self.assertEqual(m.memory_ceiling(child,hierarchy_root=root)['maxBytes'],512*1024*1024)
   (child/'memory.swap.max').write_text('max')
   with self.assertRaisesRegex(RuntimeError,'512m_scope'):m.memory_ceiling(child,hierarchy_root=root)
if __name__=='__main__':unittest.main()
