#!/usr/bin/env python3
"""Real subprocess observer regressions; no provider or service effects."""
import json,subprocess,sys,tempfile,unittest,socket
from pathlib import Path
PROBE=Path(__file__).resolve().parents[1]/'boat-socket-diagnostic.py'
class Probe(unittest.TestCase):
 def run_probe(self,address):
  script="import importlib.util,json,sys;s=importlib.util.spec_from_file_location('observer',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);print(json.dumps(m.observe(sys.argv[2])))"
  result=subprocess.run([sys.executable,'-I','-B','-c',script,str(PROBE),str(address)],capture_output=True,text=True,timeout=5,env={'PATH':'/usr/bin:/bin'})
  self.assertEqual(result.returncode,0,result.stderr);return json.loads(result.stdout)
 def test_stale_supervisor_pid_returns_explicit_metadata_from_actual_subprocess(self):
  with tempfile.TemporaryDirectory() as directory:
   address=Path(directory)/'control.sock';address.with_name('supervisor.json').write_text(json.dumps({'residentPid':2147483647}))
   observed=self.run_probe(address);self.assertEqual(observed['residentPid'],2147483647);self.assertEqual(observed['residentProcessError'],'ENOENT');self.assertEqual(observed['path']['error'],'ENOENT');self.assertEqual(observed['sockets'],[])
 def test_stale_pid_preserves_real_canonical_socket_metadata(self):
  with tempfile.TemporaryDirectory() as directory:
   address=Path(directory)/'control.sock';address.with_name('supervisor.json').write_text(json.dumps({'residentPid':2147483647}))
   with socket.socket(socket.AF_UNIX) as listener:
    listener.bind(str(address));listener.listen(1);observed=self.run_probe(address)
    self.assertTrue(observed['path']['socket']);self.assertEqual(observed['path']['inode'],address.stat().st_ino);self.assertIn('ctimeNs',observed['path']);self.assertEqual(observed['residentRootError'],'ENOENT');self.assertEqual(observed['residentProcessError'],'ENOENT')
 def test_cold_missing_supervisor_record_returns_metadata_without_failing_command(self):
  with tempfile.TemporaryDirectory() as directory:
   observed=self.run_probe(Path(directory)/'control.sock');self.assertIsNone(observed['residentPid']);self.assertEqual(observed['recordError'],'ENOENT');self.assertEqual(observed['sockets'],[])
if __name__=='__main__':unittest.main()
