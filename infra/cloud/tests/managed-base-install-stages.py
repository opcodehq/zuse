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
class NodeRuntime(unittest.TestCase):
 """Every managed base ships one pinned, checksum-verified Node.js LTS."""
 source=SCRIPT.read_text()
 def test_official_tarball_is_pinned_and_verified_before_unpacking(self):
  import re
  version=re.search(r'^node_version=(\S+)$',self.source,re.M).group(1)
  digest=re.search(r'^node_sha256=(\S+)$',self.source,re.M).group(1)
  self.assertRegex(version,r'^\d+\.\d+\.\d+$')
  self.assertRegex(digest,r'^[0-9a-f]{64}$')
  self.assertIn('https://nodejs.org/dist/v$node_version/node-v$node_version-linux-x64.tar.xz',self.source)
  check=self.source.index('sha256sum -c');self.assertLess(check,self.source.index('tar -xJf'))
  copy=self.source.index('cp -a "$node_tree/bin/." /usr/local/bin/')
  self.assertLess(self.source.index('tar -xJf'),copy)
  # Provider images may link node elsewhere; entries are replaced, never written through.
  self.assertLess(self.source.index('rm -rf -- /usr/local/bin/node /usr/local/bin/npm /usr/local/bin/npx /usr/local/bin/corepack'),copy)
  self.assertIn('[[ -f /usr/local/bin/node && ! -L /usr/local/bin/node','\n'.join(self.source.splitlines()))
  self.assertIn('npm prefix -g)" == /usr/local ]]',self.source)
  self.assertNotRegex(self.source,r'apt-get install[^\n]*\bnodejs\b')
  for tool in ('npm','npx','corepack'):self.assertIn(tool,self.source.split('for tool in ',1)[1].split(';',1)[0])
 def test_task_user_global_installs_need_no_sudo_and_are_on_path(self):
  self.assertIn("printf 'prefix=/home/zuse/.local\\n' > /home/zuse/.npmrc",self.source)
  self.assertLess(self.source.index('/home/zuse/.npmrc'),self.source.index('chown -R zuse:zuse /home/zuse'))
  wrapper=self.source.split("cat > /usr/local/lib/zuse/launch-rust-runtime <<'EOF'\n",1)[1].split('\nEOF',1)[0]
  self.assertIn('export PATH="/home/zuse/.local/bin:',wrapper)
  self.assertIn('/etc/profile.d/zuse-user-bin.sh',self.source)
  # The task user itself must run node with the setup PATH, not just root.
  check=self.source.index('node_user_check="$(sudo -n -u zuse')
  self.assertLess(self.source.index('/home/zuse/.npmrc'),check)
  self.assertIn('== "v$node_version"$\'\\n\'/home/zuse/.local ]]',self.source)
if __name__=='__main__':unittest.main()
