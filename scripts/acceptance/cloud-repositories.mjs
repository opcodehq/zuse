// Compiled GPUI -> host authority -> original scoped repository HTTP contracts -> PostgreSQL.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
const file=process.env.ZUSE_CLOUD_PROJECTS_UI_FIXTURE_FILE;
try{
  const origin=fs.readFileSync(file,'utf8').trim();assert.match(origin,/^http:\/\/127\.0\.0\.1:\d+$/);
  process.env.ZUSE_CLOUD_API_URL=origin;
  host=await engine('cloud-repositories');
  await assert.rejects(host.call('CloudProjectRemove',{projectId:'foreign',expectedAccountId:'another-account',expectedOrganizationId:null}),/account changed/);
  await assert.rejects(host.call('CloudProjects',{expectedAccountId:'dev-user',expectedOrganizationId:'another-organization'}),/account changed/);
  page=browser('cloud-repositories');
  page.open(host.url,fixture('cloud-repositories-observer.js'));
  page.wait('window.__repositoryProof?.methods.includes("WatchChats")');
  fs.writeFileSync('.context/cloud-repositories-browser.ready','ready\n');
  while(!fs.existsSync('.context/cloud-repositories-browser.done'))await sleep(250);
  const proof=page.evaluate('window.__repositoryProof');
  assert.ok(proof.github.some(g=>g.repositories.includes('example/cloud-app')),'Picker must load the authorized catalog');
  assert.ok(proof.connected.some(c=>c.result.repositoryIdentity==='github.com/example/cloud-app'));
  assert.ok(proof.removed.length>=1,'Remove must reach the real API');
  assert.ok(!proof.methods.includes('CloudEnvironment'),'Repository settings cannot wake compute');
  assert.ok(proof.connected.every(c=>c.request.expectedAccountId==='dev-user'&&c.request.expectedOrganizationId===null));
  artifact('cloud-repositories-browser-acceptance.json',{compiledGpui:true,sharedNativeBrowserComponent:true,actualHostApiPostgres:true,authorizedGithubCatalogAdapter:true,connectAndRemove:true,staleAccountAndOrganizationRejected:true,noComputeWake:true,liveGithub:false});
  artifact('cloud-repositories-browser-proof.json',proof);
  console.log('Repository browser acceptance passed');
}catch(error){console.error(`Repository acceptance failed: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
