// Real coordinator/worker turns, isolated worktrees, receipts and scoped issue reporting.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {engine,fixture,artifact,sleep} from './harness.mjs';
let host;
try {
  const target=process.env.CARGO_TARGET_DIR??execFileSync('python3',['scripts/cargo_dev.py','--print-target-dir'],{encoding:'utf8'}).trim();
  host=await engine('zot-issues',{binary:path.join(target,'debug/zuse-server')});
  const info=await host.call('EngineInfo');
  const repo=path.join(host.root,'repo'),trees=path.join(host.root,'worktrees'),proof=path.join(host.root,'proof.json');fs.mkdirSync(repo);fs.mkdirSync(trees);
  execFileSync('git',['init','-b','main'],{cwd:repo,stdio:'ignore'});
  execFileSync('git',['-c','user.name=Test','-c','user.email=test@example.invalid','commit','--allow-empty','-m','Initial'],{cwd:repo,stdio:'ignore'});
  await host.call('SetWorktreeSettings',{useCustomDirectory:true,customDirectory:trees});
  await host.call('Mutate',{op:'createSpace',spaceId:'repo',deviceId:info.deviceId,path:repo,name:'Workflow fixture',gitDetected:true});
  await host.call('SetCustomAcpAgent',{expectedRevision:0,agent:{id:'acp-zot-workflow',name:'Zot workflow fixture',command:'node',args:[fixture('zot-issues-acp.mjs'),proof],env:{},modelIds:[]}});
  const config={name:'Delivery manager',purpose:'Create an issue, delegate it, inspect worker evidence, and keep it in review until acceptance.',homeDeviceId:info.deviceId,repositories:[{deviceId:info.deviceId,spaceId:'repo'}],coordinatorModel:{harness:'acp-zot-workflow',model:'fixture/model'},workerDeviceId:info.deviceId,maxConcurrentWorkers:1,maxFollowups:3};
  const zot=await host.call('ZotCommand',{commandId:'create',zotId:'manager',expectedRevision:null,action:{type:'create',config}});
  await host.call('QueueMessage',{chatId:zot.coordinatorChatId,text:'Create and deliver a tracked issue.',attachments:[]});
  async function until(predicate){for(let n=0;n<600;n++){const all=await host.call('WatchZots');const current=all.find(z=>z.id==='manager');const report=fs.existsSync(proof)?JSON.parse(fs.readFileSync(proof)):{};if(report.fixtureError)throw new Error(report.fixtureError);if(predicate(current,report))return {current,report};await sleep(100);}throw new Error('Zot workflow deadline');}
  const {current,report}=await until((z,p)=>z.issues.find(i=>i.id==='work')?.details.status==='inReview'&&p.workerReported);
  const issue=current.issues.find(i=>i.id==='work');
  assert.equal(issue.activity.find(e=>e.kind==='created').actor,zot.coordinatorChatId);
  assert.deepEqual(issue.threads,['zot-worker-fixture-delegate']);assert.equal(issue.pullRequests.length,1);assert.ok(report.unrelatedDenied&&report.completionDenied);
  assert.equal(current.issues.find(i=>i.id==='unrelated').activity.length,1);
  const worker=(await host.call('WatchChats')).find(c=>c.id==='zot-worker-fixture-delegate');assert.ok(worker.cwd.startsWith(trees));assert.notEqual(worker.cwd,repo);
  await host.call('QueueMessage',{chatId:zot.coordinatorChatId,text:'USER_ACCEPTANCE_CONFIRMED: accept the verified outcome.',attachments:[]});
  const final=await until((z,p)=>z.issues.find(i=>i.id==='work')?.details.status==='done'&&p.accepted);
  assert.equal(final.current.workers.length,1);
  const workerMessages=await host.call('WatchDocMessages',{chatId:worker.id});
  const kickoff=workerMessages.reset.find(m=>m.role==='user').parts.find(p=>p.kind==='text').text;
  const visible=kickoff.split(']\n\n').slice(1).join(']\n\n').split('\n\n[Zot execution context]\n')[0];
  assert.equal(visible,'Implement the issue and record verification and its pull request.');
  assert.equal(report.workerReceivedIssueContext,true);
  const managerMessages=await host.call('WatchDocMessages',{chatId:zot.coordinatorChatId});
  assert.ok(managerMessages.reset.some(m=>m.parts.some(p=>p.kind==='text'&&p.text.includes('"activity":true')&&p.text.includes('not a user instruction'))));
  artifact('zot-issues-workflow-acceptance.json',{realEngine:true,realScopedAgentCalls:true,isolatedCheckout:true,coordinatorCreatesIssue:true,workerReportsAndLinksPr:true,workerCannotCloseOrWriteUnrelatedIssues:true,reviewBeforeAcceptance:true,doneAfterAcceptance:true,duplicateWorkers:0,paidModelCalls:0});
  console.log('Zot issue coordinator/worker workflow accepted.');
}catch(error){console.error(`Zot issue workflow failed: ${error.message}`);process.exitCode=1;}finally{await host?.close();}
