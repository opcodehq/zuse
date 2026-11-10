"""Execute the actual installer receipt blocks in an isolated root-owned tree."""
import subprocess,unittest
from pathlib import Path
SCRIPT=Path(__file__).resolve().parents[1]/'managed-base-install.sh'
class Stages(unittest.TestCase):
 def exercise(self,case):
  code=r'''import ast,json,hashlib,os,tempfile,pathlib,stat
source=pathlib.Path(os.environ['INSTALLER']).read_text()
with tempfile.TemporaryDirectory(prefix='zuse-layout-receipt-') as temporary:
 root=pathlib.Path(temporary)
 def mapped(value):return str(root/value.lstrip('/'))
 paths=['/opt/zuse-base-input/bundle.json','/usr/local/bin/zuse-cloud-runtime','/usr/local/lib/zuse/managed-base-install.sh','/opt/zuse/preparation/plan.json']
 for value in paths:
  p=pathlib.Path(mapped(value));p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b'isolated '+value.encode());p.chmod(0o444)
 os.chown(mapped(paths[-1]),10001,10001)
 runtime=hashlib.sha256(pathlib.Path(mapped(paths[1])).read_bytes()).hexdigest()
 pathlib.Path(mapped(paths[0])).write_text(json.dumps(dict(runtimeSha256=runtime,harnessRelease='a'*64)))
 class Rewrite(ast.NodeTransformer):
  def visit_Constant(self,node):
   if isinstance(node.value,str) and node.value.startswith(('/opt/','/usr/')):return ast.copy_location(ast.Constant(mapped(node.value)),node)
   return node
 def block(name):
  text=source.split("<<'"+name+"'\n",1)[1].split('\n'+name,1)[0]
  exec(compile(ast.fix_missing_locations(Rewrite().visit(ast.parse(text))),name,'exec'),{})
 block('LAYOUT_RECEIPT');receipt=pathlib.Path(mapped('/opt/zuse/preparation/layout-receipt.json'))
 assert receipt.stat().st_uid==0 and stat.S_IMODE(receipt.stat().st_mode)==0o444
 case=os.environ['CASE']
 if case=='runtime-changed':pathlib.Path(mapped(paths[1])).write_bytes(b'changed')
 elif case=='symlink':
  p=pathlib.Path(mapped(paths[1]));copy=p.with_suffix('.saved');p.rename(copy);p.symlink_to(copy)
 elif case=='writable-plan':pathlib.Path(mapped(paths[-1])).chmod(0o666)
 elif case=='receipt-writable':receipt.chmod(0o666)
 elif case=='wrong-plan-owner':os.chown(mapped(paths[-1]),0,0)
 try:block('LAYOUT_VERIFY')
 except AssertionError:
  assert case!='valid'
 else:assert case=='valid'
 print(case)
'''
  result=subprocess.run(['sudo','-n','env','INSTALLER='+str(SCRIPT),'CASE='+case,'python3','-I','-c',code],capture_output=True,text=True,timeout=5)
  self.assertEqual(result.returncode,0,result.stderr)
 def test_actual_layout_receipt_and_all_input_fences(self):
  for case in ('valid','runtime-changed','symlink','writable-plan','receipt-writable','wrong-plan-owner'):
   with self.subTest(case=case):self.exercise(case)
if __name__=='__main__':unittest.main()
