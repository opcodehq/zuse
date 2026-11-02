// Real browser action against a disposable engine; no provider run or cloud fixture.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {engine,browser,fixture,artifact,sleep} from './harness.mjs';
let host,page;
try {
  host=await engine('thread-conversations');
  const info=await host.call('EngineInfo');
  const project=host.root+'/project';fs.mkdirSync(project);
  await host.call('Mutate',{op:'createSpace',spaceId:'project',deviceId:info.deviceId,path:project,name:'Project'});
  await host.call('Mutate',{op:'createChat',chatId:'thread',spaceId:'project',cwd:project});
  await host.call('Mutate',{op:'renameChat',chatId:'thread',title:'First conversation'});
  await host.call('CreateThreadSession',{chatId:'second-conversation',threadId:'thread'});
  page=browser('thread-conversations');page.open(host.url,fixture('browser-observer.js'));
  page.wait('window.__browserProof.methods.includes("WatchChats")');await sleep(200);
  page.click(120,112);page.wait('window.__browserProof.methods.includes("WatchDocMessages")');
  page.click(497,62);page.wait('window.__browserProof.methods.includes("CreateThreadSession")');
  const chats=await host.call('WatchChats');
  assert.equal(chats.filter(chat=>chat.threadId==='thread').length,2);
  assert.equal(chats.filter(chat=>!chat.parentChatId&&!chat.threadId).length,1);
  const calls=page.evaluate('window.__browserProof.methods');
  assert.equal(calls.filter(method=>method==='CreateThreadSession').length,1);
  assert.equal(calls.filter(method=>method==='QueueMessage'||method==='QueueCommand').length,0);
  artifact('thread-conversations-runtime-acceptance.json',{compiledGpui:true,realEngine:true,uiCreatesConversation:true,threeConversationsOneThread:true,agentWrites:0});
  console.log('Thread conversation UI accepted.');
} catch(error) {
  console.error(`Thread conversation UI failed: ${error.message}`);process.exitCode=1;
} finally {page?.close();await host?.close();}
