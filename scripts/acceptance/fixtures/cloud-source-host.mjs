// Real local repository and account-bound engine for Cloud source acceptance.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {spawn,execFileSync} from 'node:child_process';
import {fixture,rpc,sleep} from '../harness.mjs';

export async function sourceHost(prefix,api){
 const root=fs.mkdtempSync(path.resolve('.context/'+prefix+'-runtime-'));
 const bin=path.join(root,'bin');fs.mkdirSync(bin);
 fs.copyFileSync(fixture('creation-sources-gh.py'),path.join(bin,'gh'));fs.chmodSync(path.join(bin,'gh'),0o755);
 const profile=path.join(root,'profile');fs.mkdirSync(profile);fs.writeFileSync(path.join(profile,'harness-prefs.json'),JSON.stringify({enabled:[]}));
 const ready=path.join(root,'host.ready'),log=fs.openSync(path.join(root,'host.private.log'),'w',0o600);
 const child=spawn('target/debug/examples/cloud-ui-host',[profile,ready],{stdio:['ignore',log,log],env:{...process.env,ZUSE_CLOUD_API_URL:api,PATH:bin+path.delimiter+process.env.PATH,ZUSE_NO_LOGIN_SHELL:'1',ZUSE_CREATION_SOURCE_FIXTURE:root}});fs.closeSync(log);
 let socket;
 const close=async()=>{socket?.close();child.kill('SIGTERM');for(let n=0;n<100&&child.exitCode===null&&child.signalCode===null;n++)await sleep(50);assert.ok(child.exitCode!==null||child.signalCode!==null,'Owned Cloud source host stopped');};
 try{
  for(let n=0;n<200&&!fs.existsSync(ready);n++){assert.equal(child.exitCode,null);assert.equal(child.signalCode,null);await sleep(100);}
  assert.ok(fs.existsSync(ready),'Account-bound host opened its listener');
  const port=Number(fs.readFileSync(ready,'utf8'));
  socket=await new Promise((resolve,reject)=>{const ws=new WebSocket('ws://127.0.0.1:'+port);ws.onopen=()=>resolve(ws);ws.onerror=()=>reject(Error('Fixture engine unavailable'));});
  const call=rpc(socket);const context=await call('CloudContext');assert.ok(context.projects.some(project=>project.repositoryIdentity==='github.com/example/project'&&project.prepared===true),'Prepared Cloud catalogue is available');fs.writeFileSync(path.join(root,'cloud-context.json'),JSON.stringify(context));const repo=path.join(root,'project');fs.mkdirSync(repo);
  const git=(...args)=>execFileSync('git',['-C',repo,...args],{encoding:'utf8'}).trim();
  git('init','-q','-b','main');git('config','user.name','Cloud source acceptance');git('config','user.email','source@example.test');git('remote','add','origin','https://github.com/example/project.git');
  fs.writeFileSync(path.join(repo,'README.md'),'Primary checkout\n');git('add','README.md');git('commit','-qm','Initial');
  git('switch','-qc','feature/available');fs.writeFileSync(path.join(repo,'source.txt'),'Selected source\n');git('add','source.txt');git('commit','-qm','Selected source');git('branch','pr-fixture');git('update-ref','refs/remotes/origin/feature/available','HEAD');git('switch','-q','main');git('branch','feature/unpublished');
  fs.writeFileSync(path.join(repo,'README.md'),'Unsaved primary changes\n');const originalStatus=git('status','--porcelain=v1'),originalCheckouts=git('worktree','list','--porcelain');
  const info=await call('EngineInfo');await call('Mutate',{op:'createSpace',spaceId:'source-project',deviceId:info.deviceId,path:repo,name:'Source project',gitDetected:true});
  let spaces;
  for(let n=0;n<100;n++){spaces=await call('WatchSpaces');if(spaces.some(space=>space.id==='source-project'&&space.remoteRepository==='github.com/example/project'))break;await sleep(100);}
  assert.ok(spaces.some(space=>space.remoteRepository==='github.com/example/project'),'Real Git metadata links the local and Cloud repository');
  return {root,repo,port,call,git,originalStatus,originalCheckouts,close};
 }catch(error){await close();throw error;}
}
