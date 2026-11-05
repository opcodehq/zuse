import importlib.util,tempfile,unittest,json,hashlib,os
from pathlib import Path
s=importlib.util.spec_from_file_location('stage',Path(__file__).resolve().parents[1]/'stage-portable-bundle.py');m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Portable(unittest.TestCase):
 def test_only_two_new_payload_files_and_original_modes_preserved(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);source=root/'old';source.mkdir();names=['runtime','claude','codex','grok','codex-assets.tar','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json']
   for name in names:(source/name).write_bytes(b'synthetic '+name.encode());(source/name).chmod(0o555)
   (source/'prepare-plan.json').write_text(json.dumps(dict(buildId='old',templateVersion='old',configurationDigest='old',managedBase=True,repositories=[])))
   files=[dict(path=name,bytes=(source/name).stat().st_size,sha256=m.digest(source/name)) for name in names];original=dict(schema=1,status='candidate-only',runtimeSha256=m.digest(source/'runtime'),releaseId='old',harnessRelease='a'*64,files=files);(source/'bundle.json').write_text(json.dumps(original))
   installer=root/'installer';installer.write_bytes(b'new staged installer');helper=root/'helper';helper.write_bytes(b'pinned helper');output=root/'new'
   result=m.stage(source,installer,helper,output,m.digest(source/'bundle.json'),m.digest(source/'runtime'),m.digest(installer))
   self.assertEqual(len(result['hardlinkedFiles']),12);self.assertTrue(result['originalFileMetadataPreserved']);self.assertNotEqual(result['releaseId'],'old');self.assertFalse(result['providerEffectsPerformed'])
   for name in result['hardlinkedFiles']:self.assertTrue(os.path.samefile(source/name,output/name));self.assertEqual((source/name).stat().st_mode&0o777,0o555)
   for name in result['replacedFiles']:self.assertFalse(os.path.samefile(source/name,output/name))
   self.assertEqual((output/'managed-base-install.sh').stat().st_mode&0o777,0o444)
   with self.assertRaises(FileExistsError):m.stage(source,installer,helper,output,m.digest(source/'bundle.json'),m.digest(source/'runtime'),m.digest(installer))
if __name__=='__main__':unittest.main()
