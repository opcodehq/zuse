import unittest,subprocess,json,urllib.parse,shutil
from pathlib import Path
PROBE=Path(__file__).resolve().parents[1]/'staging-v2-gateway.mjs'
PEER=r'''
import fs from 'node:fs';import{syncBuiltinESMExports}from'node:module';
const write=fs.writeFileSync;fs.writeFileSync=(p,...args)=>{if(p==='/etc/zuse/cloud/staging/synthetic-ready')return;return write(p,...args)};syncBuiltinESMExports();
const mode=process.env.ZUSE_SYNTHETIC_MODE,sid='synthetic-session';
globalThis.WebSocket=class{constructor(){this.listeners={};this.identity=false;setTimeout(()=>this.emit({_zuseGateway:'availability',state:'available',reset:false}),5)}addEventListener(k,f){this.listeners[k]=f}emit(v){this.listeners.message?.({data:typeof v==='string'?v:JSON.stringify(v)})}send(raw){let r=JSON.parse(raw);if(r.method==='EngineReady')queueMicrotask(()=>this.emit({id:r.id,ok:{}}));else if(r.method==='WatchChats'){queueMicrotask(()=>this.emit({id:r.id,item:[]}));setTimeout(()=>{this.identity=true;this.emit({id:r.id,item:[{id:sid}]})},15)}else if(r.method==='WatchSessions')queueMicrotask(()=>this.emit({id:r.id,item:[]}));else if(r.method==='WatchDocMessages'){if(!this.identity)throw Error('watched_before_identity');queueMicrotask(()=>{if(mode==='error')this.emit({id:r.id,err:{message:'synthetic'}});else if(mode==='reset')this.emit({_zuseGateway:'availability',state:'available',reset:true});else if(mode==='malformed')this.emit('{');else this.emit({id:r.id,item:{reset:[{id:'user',role:'user',status:'complete',parts:[]},{id:'assistant',role:'assistant',status:'complete',parts:[{id:'text',kind:'text',text:'EXPECTED_COMPLETE'},{id:'tool',kind:'tool',call:{},resolved:true,isError:false}]}]}})})}else throw Error('unexpected_effectful_rpc')}close(){}};
'''
class Gateway(unittest.TestCase):
 def run_probe(self,mode):
  value={'grant':{'workspaceId':'synthetic','protocol':'zuse-workspace-rust-v1','rpcProtocol':'zuse-rpc-v1','credential':'synthetic','wsUrl':'wss://api-rust-stag.zuse.sh/v1/cloud/workspaces/synthetic/gateway'},'sessionId':'synthetic-session','prefix':'.zuse-acceptance/'+'a'*32,'action':'observe','expectedMarker':'EXPECTED_COMPLETE','readyFile':'/etc/zuse/cloud/staging/synthetic-ready'}
  result=subprocess.run([shutil.which('node'),'--import','data:text/javascript,'+urllib.parse.quote(PEER,safe=''),str(PROBE)],input=json.dumps(value),text=True,capture_output=True,env={'PATH':'/usr/bin:/bin','ZUSE_SYNTHETIC_MODE':mode},timeout=5)
  self.assertEqual(result.stderr,'');return result,json.loads(result.stdout)
 def test_waits_for_exact_identity_then_observes_complete_tools(self):
  result,value=self.run_probe('success');self.assertEqual(result.returncode,0);self.assertTrue(value['ok']);self.assertTrue(value['sessionIdentityObserved']);self.assertEqual(value['docFrames'],1);self.assertEqual(value['resolvedToolCount'],1)
 def test_stream_error_is_terminal_instead_of_silent_timeout(self):
  result,value=self.run_probe('error');self.assertEqual(result.returncode,1);self.assertEqual(value['code'],'stream_rpc_rejected');self.assertEqual(value['rejectedStream'],'WatchDocMessages');self.assertEqual(value['docFrames'],0)
 def test_gateway_reset_requires_reconnect_instead_of_retaining_dead_stream(self):
  result,value=self.run_probe('reset');self.assertEqual(result.returncode,1);self.assertEqual(value['code'],'gateway_reconnect_required');self.assertTrue(value['gatewayReset'])
 def test_malformed_stream_frame_fails_after_initial_ready(self):
  result,value=self.run_probe('malformed');self.assertEqual(result.returncode,1);self.assertEqual(value['code'],'invalid_gateway_frame')
if __name__=='__main__':unittest.main()
