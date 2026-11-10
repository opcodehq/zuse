import unittest,importlib.util,tempfile,subprocess,hashlib,os
from pathlib import Path
s=importlib.util.spec_from_file_location('diag',Path(__file__).resolve().parents[1]/'managed-gh-diagnostic.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Diagnostic(unittest.TestCase):
 def test_pinned_runtime_and_all_fixed_uid_command_cases(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);path=root/'usr/local/bin/zuse-cloud-runtime';path.parent.mkdir(parents=True);path.write_bytes(b'fixture');path.chmod(0o755)
   expected=hashlib.sha256(b'fixture').hexdigest()
   for probe in m.COMMANDS:
    for user in ('root','zuse'):
     def run(argv,**kwargs):
      self.assertEqual(argv[:6],['sudo','-n','-u',user,'--','/usr/bin/env']);self.assertIn('-i',argv);self.assertEqual(argv[-len(m.COMMANDS[probe]):],m.COMMANDS[probe]);self.assertEqual(kwargs['timeout'],5);self.assertEqual(kwargs['stdin'],subprocess.DEVNULL)
      return subprocess.CompletedProcess(argv,0,stdout=b'gh version 2.0\nPRIVATE',stderr=b'PRIVATE')
     r=m.diagnose(expected,probe,user,filesystem=root,run=run);self.assertEqual(r['exitCode'],0);self.assertNotIn('PRIVATE',str(r))
   with self.assertRaises(AssertionError):m.diagnose('0'*64,'system','root',filesystem=root,run=lambda *a,**k:self.fail('must not execute changed runtime'))
 def test_timeouts_os_errors_and_symlink_fail_closed(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);path=root/'usr/local/bin/zuse-cloud-runtime';path.parent.mkdir(parents=True);path.write_bytes(b'fixture');path.chmod(0o755);expected=hashlib.sha256(b'fixture').hexdigest()
   for error,outcome in [(subprocess.TimeoutExpired('fixture',5,output=b'PRIVATE'),'timeout'),(OSError(13,'PRIVATE'),'os-error')]:
    def fail(*a,**k):raise error
    r=m.diagnose(expected,'runtime','zuse',filesystem=root,run=fail);self.assertEqual(r['outcome'],outcome);self.assertNotIn('PRIVATE',str(r))
   path.unlink();path.symlink_to('/dev/null')
   with self.assertRaises(OSError):m.diagnose(expected,'runtime','zuse',filesystem=root)
if __name__=='__main__':unittest.main()
