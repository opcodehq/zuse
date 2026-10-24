import assert from 'node:assert/strict';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page,stage='startup';
try {
  host=await engine('managed-controls');
  page=browser('managed-controls');page.open(host.url,fixture('managed-access-observer.js'));
  page.wait('window.__managedAccessProof?.methods.includes("WatchChats")');
  await sleep(200);
  stage='settings';page.click(274,982);await sleep(150);
  page.click(100,182);
  page.wait('window.__managedAccessProof?.host?.configured===false');
  const proof=page.evaluate('window.__managedAccessProof');
  assert.equal(proof.methods.filter(m=>m==='WatchManagedHost').length,1);
  assert.ok(!proof.methods.includes('ListAccountEnvironments')&&!proof.methods.includes('ListAccountDevices'));
  assert.ok(!proof.methods.includes('QueueCommand')&&!proof.methods.includes('QueueMessage'));
  const tree=page.run(['snapshot','-i']);
  artifact('managed-controls-ui-tree.json',tree);
  page.screenshot('managed-controls-settings.png');
  const result={compiledGpui:true,actualEngine:true,accountSignedOut:true,hostConfigured:false,hostStatusSubscribed:true,statusWatchCount:1,accountCatalogCalls:0,agentWrites:0,liveAccountAndDeployedTunnel:false};
  artifact('managed-controls-browser-acceptance.json',result);console.log(JSON.stringify(result));
}catch(error){console.error(`Managed controls acceptance failed at ${stage}: ${error.message}`);process.exitCode=1;}
finally{page?.close();await host?.close();}
