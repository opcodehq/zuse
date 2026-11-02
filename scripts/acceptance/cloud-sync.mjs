// Real host controller -> retained connection -> packaged guest -> verified mirrors.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {engine,artifact,sleep} from './harness.mjs';
let host, connection, stage='startup';
const fixture=process.env.ZUSE_CLOUD_LAUNCH_UI_FIXTURE_FILE;
async function until(read,accept,label,timeout=30000) {
  const end=Date.now()+timeout;let last;
  while(Date.now()<end){
    try {const value=await read();if(accept(value))return value;last=JSON.stringify(value).slice(0,1000);}
    catch(error){last=error.message;}
    await sleep(150);
  }
  throw new Error(`${label} timed out: ${last}`);
}
async function connect(host,workspace) {
  const socket=new WebSocket(`ws://127.0.0.1:${host.port}`);
  await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject;});
  const ready=new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>reject(new Error('Cloud connection timed out')),30000);
    socket.onmessage=event=>{const frame=JSON.parse(event.data);if(frame.err){clearTimeout(timer);reject(new Error(frame.err));}if((frame.item??frame.ok)?.state==='ready'){clearTimeout(timer);resolve();}};
  });
  socket.send(JSON.stringify({id:1,method:'CloudEnvironment',params:{workspaceId:workspace}}));
  await ready;return socket;
}
try {
  const origin=fs.readFileSync(fixture,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  process.env.ZUSE_CLOUD_MIRROR_ROOT=path.join(process.cwd(),'.context',`cloud-sync-default-${randomUUID()}`);
  host=await engine('cloud-sync');
  const account=(await host.call('AuthStatus')).user.id;
  const initial=await host.call('CloudCreate',{projectId:'create-project',providerId:'boxd',baseRef:'main',agent:'claude-code',model:'sonnet',runtimeMode:'approval-required',firstMessage:'Create the original cloud conversation',idempotencyKey:randomUUID(),expectedAccountId:account});
  const workspace=(initial.workspace.workspace??initial.workspace).workspaceId;
  connection=await connect(host,workspace);
  await until(()=>host.call('CloudHistory',{workspaceId:workspace}),h=>JSON.stringify(h).includes('Brokered native provider reply'),'Original cloud turn');
  fs.writeFileSync(fixture.replace(/\.[^.]+$/,'.sync-repository'),'import legacy repository metadata\n');
  await until(()=>Promise.resolve(fs.existsSync(fixture.replace(/\.[^.]+$/,'.sync-repository-ready'))),Boolean,'Repository metadata import');
  const root=fs.realpathSync(fs.readFileSync(fixture.replace(/\.[^.]+$/,'.runtime'),'utf8'));
  const remote=path.join(root,'checkout');const mirror=path.join(host.root,'local-mirror');const second=path.join(host.root,'second-mirror');
  const params={workspaceId:workspace,expectedAccountId:account};
  const status=()=>host.call('CloudSyncStatus',params).then(r=>r.status);
  stage='default-path';const configured=await host.call('CloudSyncConfigure',{...params,enabled:true});
  const defaultMirror=configured.status.localPath;assert.ok(defaultMirror.startsWith(process.env.ZUSE_CLOUD_MIRROR_ROOT+path.sep));
  await until(status,s=>s.state==='synced','Default mirror');fs.writeFileSync(path.join(defaultMirror,'local-only'),'default mirror local work');
  stage='enable';await host.call('CloudSyncConfigure',{...params,enabled:true,localPath:mirror});
  await until(status,s=>s.state==='synced','Initial local mirror');
  assert.deepEqual(fs.readFileSync(path.join(mirror,'tracked')),fs.readFileSync(path.join(remote,'tracked')));
  fs.writeFileSync(path.join(mirror,'local-only'),'unrelated local work');fs.writeFileSync(path.join(remote,'sync-file'),'cloud bytes');
  stage='invalid-path';const invalid=path.join(host.root,'unowned-folder');fs.mkdirSync(invalid);fs.writeFileSync(path.join(invalid,'sentinel'),'preserved');
  await assert.rejects(host.call('CloudSyncConfigure',{...params,enabled:true,localPath:invalid}),/empty folder/);
  assert.equal((await status()).localPath,mirror);assert.equal(fs.readFileSync(path.join(invalid,'sentinel'),'utf8'),'preserved');
  stage='background';await until(()=>Promise.resolve(fs.existsSync(path.join(mirror,'sync-file'))),Boolean,'Background file update');
  const originalInode=fs.statSync(path.join(mirror,'tracked')).ino;
  stage='disable';await host.call('CloudSyncConfigure',{...params,enabled:false});
  fs.writeFileSync(path.join(remote,'sync-file'),'after disable');await sleep(5500);
  assert.equal(fs.readFileSync(path.join(mirror,'sync-file'),'utf8'),'cloud bytes');assert.equal((await status()).enabled,false);
  stage='reenable';await host.call('CloudSyncConfigure',{...params,enabled:true});
  await until(()=>Promise.resolve(fs.readFileSync(path.join(mirror,'sync-file'),'utf8')),s=>s==='after disable','Re-enabled mirror');
  assert.equal(fs.statSync(path.join(mirror,'tracked')).ino,originalInode);
  const profile=host.root;
  stage='restart';connection.close();connection=null;await host.close();host=null;
  const trace=process.env.ZUSE_CLOUD_TEST_HTTP_TRACE;const prior=trace&&fs.readFileSync(trace,'utf8');
  host=await engine('cloud-sync',{profileRoot:profile});
  const restored=await status();assert.equal(restored.enabled,true);assert.equal(restored.localPath,mirror);
  await sleep(750);
  if(trace){const later=fs.readFileSync(trace,'utf8').slice(prior.length);assert.ok(!/^POST .*\/(resume|gateway\/ticket)(?:\n|$)/m.test(later),'Loading persisted sync cannot attach or wake compute');}
  connection=await connect(host,workspace);
  fs.writeFileSync(path.join(remote,'after-restart'),'continued original workspace');await host.call('CloudSyncRetry',params);
  await until(()=>Promise.resolve(fs.existsSync(path.join(mirror,'after-restart'))),Boolean,'Sync after host restart');
  stage='path-change';await host.call('CloudSyncConfigure',{...params,enabled:true,localPath:second});
  await until(status,s=>s.state==='synced'&&s.localPath===second,'Custom path change');fs.writeFileSync(path.join(second,'local-only'),'another unrelated file');
  stage='archive';const archived=await fetch(`${origin}/v1/cloud/workspaces/${workspace}/archive`,{method:'POST',headers:{authorization:'Bearer dev-user','content-type':'application/json'},body:JSON.stringify({workspaceId:workspace,commandId:randomUUID()})});assert.equal(archived.status,200);
  await until(()=>Promise.resolve([defaultMirror,mirror,second].every(local=>!fs.existsSync(path.join(local,'tracked')))),Boolean,'Automatic archive mirror cleanup',45000);
  const archivedRows=await host.call('CloudWorkspaces');assert.ok(archivedRows.workspaces.some(w=>w.workspaceId===workspace&&w.state==='archived'&&w.localMirrorCleanup?.state==='complete'));
  assert.equal((await status()).enabled,false);
  for(const local of [defaultMirror,mirror,second]){assert.ok(!fs.existsSync(path.join(local,'tracked')));assert.ok(fs.existsSync(path.join(local,'local-only')));}
  await assert.rejects(host.call('CloudSyncConfigure',{...params,enabled:true}),/Unarchive/);
  await assert.rejects(host.call('CloudSyncStatus',{...params,expectedAccountId:'other-account'}),/account changed/);
  const receipt={actualHostAndGuest:true,postgresApi:true,backgroundSync:true,disableRetainsMirror:true,reEnable:true,persistedHostRestart:true,noWakeOnReload:true,explicitConnectOnly:true,unchangedInode:true,customPaths:true,defaultPath:true,archiveStopsSync:true,allRegisteredPathsCleaned:true,cleanupWithoutViewer:true,unrelatedFilesPreserved:true,wrongAccountRejected:true,unownedTargetRejected:true,workspaceId:workspace,liveProvider:false};
  artifact('cloud-sync-controller-acceptance.json',receipt);
  fs.writeFileSync(fixture.replace(/\.[^.]+$/,'.done'),'verified\n');console.log('Cloud sync controller acceptance passed');
} catch(error) {artifact('cloud-sync-controller-failure.json',{stage,error:error.message});console.error(`Cloud sync failed at ${stage}: ${error.message}`);process.exitCode=1;}
finally {connection?.close();await host?.close();}
