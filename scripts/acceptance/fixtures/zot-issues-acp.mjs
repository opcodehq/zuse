// A deterministic agent exercises scoped coordinator/worker tools without a model API.
import fs from 'node:fs';
import readline from 'node:readline';
const [proofFile] = process.argv.slice(2);
let env = {};
const proof = () => fs.existsSync(proofFile) ? JSON.parse(fs.readFileSync(proofFile, 'utf8')) : {};
const record = patch => fs.writeFileSync(proofFile, JSON.stringify({...proof(), ...patch}));
async function control(method, params) {
  const ws = new WebSocket(`ws://127.0.0.1:${env.ZUSE_IPC_PORT}`);
  await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject;});
  return new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>{ws.close();reject(new Error('Scoped fixture RPC timed out'));},15000);
    ws.onmessage=e=>{const frame=JSON.parse(e.data);if(frame.id!==1)return;clearTimeout(timer);ws.close();frame.err?reject(new Error(String(frame.err))):resolve(frame.ok);};
    ws.send(JSON.stringify({id:1,method:'ControlCall',params:{token:env.ZUSE_CONTROL_TOKEN,method,params}}));
  });
}
const command = (id,action) => control('ZotCommand',{commandId:id,zotId:env.ZUSE_ZOT_ID??env.ZUSE_ZOT_WORKER_ID,expectedRevision:null,action});
for await (const line of readline.createInterface({input:process.stdin})) {
  const frame=JSON.parse(line);let result={};
  try {
    if(frame.method==='initialize')result={protocolVersion:1,agentCapabilities:{loadSession:true}};
    else if(frame.method==='session/new'||frame.method==='session/load'){
      env=Object.fromEntries((frame.params.mcpServers?.find(s=>s.name==='zuse')?.env??[]).map(e=>[e.name,e.value]));
      result={sessionId:'zot-issues-fixture',models:{currentModelId:'fixture/model',availableModels:[{modelId:'fixture/model',name:'Fixture'}]}};
    } else if(frame.method==='session/prompt'){
      const text=(frame.params.prompt??[]).map(p=>p.text??'').join('\n');
      const parsed=text.match(/Current state:\n([\s\S]*?)\nPurpose:/);
      const current=parsed?JSON.parse(parsed[1]):null;
      if(env.ZUSE_ZOT_ID){
        if(!proof().created){
          const first=await command('fixture-create-issue',{type:'createIssue',issueId:'work',details:{title:'Deliver a tracked change',description:'Implement and verify the requested change.',status:'todo',priority:'normal',assigned:true,dueDate:null,labels:[],relations:[]}});
          await command('fixture-unrelated-issue',{type:'createIssue',issueId:'unrelated',details:{title:'Unrelated backlog',description:'Not assigned to this worker.',dueDate:null}});
          record({created:true});
          const repository=first.config.repositories[0];
          await command('fixture-delegate',{type:'delegate',issueId:'work',worker:{chatId:'zot-worker-fixture-delegate',deviceId:repository.deviceId,spaceId:repository.spaceId,objective:'Implement the issue and record verification and its pull request.',followups:0},model:first.config.coordinatorModel,base:'main'});
          record({delegated:true});
        } else if(current?.issues.find(i=>i.id==='work')?.pullRequests.length){
          const issue=current.issues.find(i=>i.id==='work');
          const accepted=text.includes('USER_ACCEPTANCE_CONFIRMED');
          await command(accepted?'fixture-done':'fixture-review',{type:'editIssue',issueId:'work',details:{...issue.details,status:accepted?'done':'inReview'}});
          await command('fixture-report',{type:'writeFile',path:'reports/work.md',content:'Verification passed; linked PR recorded. Completion requires user acceptance.',expectedVersion:0});
          record(accepted?{accepted:true}:{reviewed:true});
        }
        const events=await control('ZotEvents',{zotId:env.ZUSE_ZOT_ID});
        await control('AckZotEvents',{zotId:env.ZUSE_ZOT_ID,eventIds:events.map(e=>e.id)});
      }else if(env.ZUSE_ZOT_WORKER_ID){
        await command('fixture-worker-report',{type:'commentIssue',issueId:'work',text:'Verification passed in the isolated checkout.'});
        await command('fixture-worker-pr',{type:'linkIssuePullRequest',issueId:'work',url:'https://github.com/example/project/pull/123',title:'Deliver tracked change'});
        let unrelatedDenied=false,completionDenied=false;
        try{await command('fixture-forbidden-unrelated',{type:'commentIssue',issueId:'unrelated',text:'Should be rejected'});}catch{unrelatedDenied=true;}
        try{await command('fixture-forbidden-done',{type:'editIssue',issueId:'work',details:{title:'Forbidden',description:'',status:'done',dueDate:null}});}catch{completionDenied=true;}
        record({workerReported:true,unrelatedDenied,completionDenied,workerReceivedIssueContext:text.includes('Implement and verify the requested change.')&&text.includes('\"issueId\":\"work\"')});
      } else throw new Error('Missing engine-stamped Zot scope');
      process.stdout.write(JSON.stringify({jsonrpc:'2.0',method:'session/update',params:{sessionId:'zot-issues-fixture',update:{sessionUpdate:'agent_message_chunk',content:{type:'text',text:'Tracked issue update completed.'}}}})+'\n');
      result={stopReason:'end_turn'};
    }
    if(frame.id!==undefined)process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:frame.id,result})+'\n');
  } catch(error) {
    record({fixtureError:error.message});
    if(frame.id!==undefined)process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:frame.id,error:{code:-32000,message:error.message}})+'\n');
  }
}
