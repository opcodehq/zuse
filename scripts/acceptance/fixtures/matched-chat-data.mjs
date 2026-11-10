import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {randomUUID,createHash} from 'node:crypto';
import {engine} from '../harness.mjs';
import {t3Server} from './t3-server.mjs';

export async function matchedChatData(root, count=1500) {
  const project=path.join(root,'project');fs.mkdirSync(project,{recursive:true});
  fs.writeFileSync(path.join(project,'README.md'),'Matched 3G benchmark workspace.\n');
  execFileSync('git',['init','-q',project]);
  execFileSync('git',['-C',project,'add','README.md']);
  execFileSync('git',['-C',project,'-c','user.name=Benchmark','-c','user.email=fixture@example.test','commit','-qm','fixture']);
  const provider=path.resolve('scripts/acceptance/fixtures/matched-chat-provider.py');fs.chmodSync(provider,0o755);
  const messages=Array.from({length:count},(_,i)=>({id:'history-'+String(i).padStart(5,'0'),role:i%2?'assistant':'user',
    text:(`History ${String(i).padStart(5,'0')}: `+'Historical context 🦀 '.repeat(40)).trimEnd()+(i%2?'\n\n':''),
    createdAt:new Date(Date.parse('2026-11-08T12:00:00.000Z')+i*1000).toISOString()}));
  fs.writeFileSync(path.join(root,'canonical-history.json'),JSON.stringify(messages));
  const zuseRoot=path.join(root,'zuse');fs.mkdirSync(path.join(zuseRoot,'profile'),{recursive:true});
  fs.writeFileSync(path.join(zuseRoot,'profile/harness-prefs.json'),JSON.stringify({enabled:['claude-code']}));
  const claude=path.join(zuseRoot,'claude');const source=path.join(claude,'projects/fixture/history.jsonl');fs.mkdirSync(path.dirname(source),{recursive:true});
  const session='8a2a521a-3c16-44a9-b719-8040658f28af';
  const records=messages.map(row=>({type:row.role,uuid:row.id,timestamp:row.createdAt,sessionId:session,cwd:project,
    message:{role:row.role,content:row.role==='assistant'?[{type:'text',text:row.text}]:row.text}}));
  records.push({type:'summary',summary:'3G matched chat',sessionId:session,cwd:project});
  fs.writeFileSync(source,records.map(JSON.stringify).join('\n'));
  const zuse=await engine('chat-comparison',{profileRoot:zuseRoot,env:{PATH:'/usr/bin:/bin',ZUSE_NO_LOGIN_SHELL:'1',CLAUDE_CONFIG_DIR:claude,CODEX_HOME:path.join(zuseRoot,'codex'),CLAUDE_CODE_EXECUTABLE:provider,ZUSE_MATCHED_PROVIDER_LOG:path.join(zuseRoot,'provider-seen.jsonl')}});
  let t3;
  try {
    const imports=await zuse.call('ListExternalChats');
    const external=imports.find(row=>row.provider==='claude-code');assert(external,'Canonical history is discoverable');
    const imported=await zuse.call('ImportExternalChat',{id:external.id,requestId:randomUUID()});
    const chatId=imported.chat.id;
    await zuse.call('Mutate',{op:'setChatConfig',chatId,expectedConfig:{deviceId:imported.chat.deviceId,config:imported.chat.config},config:{...imported.chat.config,harness:'claude-code',model:'claude-sonnet-4-6'}});
    const zuseHistory=await zuse.call('WatchDocMessages',{chatId});
    assert.equal(zuseHistory.reset.length,count,'Both products must retain the same canonical history count');
    for(let i=0;i<count;i++){assert.equal(zuseHistory.reset[i].role,messages[i].role);assert.equal(zuseHistory.reset[i].parts.find(p=>p.kind==='text')?.text,messages[i].text);}
    const seedRoot=path.join(root,'t3-seed');fs.mkdirSync(seedRoot,{recursive:true});
    t3=await t3Server(seedRoot,{provider,workspace:project});
    const projectId='benchmark-project',threadId='benchmark-thread';
    await t3.call('projects.mutate',{type:'project.create',commandId:randomUUID(),projectId,title:'3G benchmark',workspaceRoot:project});
    await t3.call('orchestration.dispatchCommand',{type:'thread.create',commandId:randomUUID(),threadId,projectId,title:'3G matched chat',createdBy:'user',creationSource:'web',modelSelection:{instanceId:'claudeAgent',model:'claude-sonnet-4-6'},runtimeMode:'full-access',interactionMode:'default',branch:null,worktreePath:null});
    await t3.close();t3=null;
    execFileSync('python3',['-c',`import sqlite3,json,sys
db,messages_path,thread_id=sys.argv[1:]
c=sqlite3.connect(db)
rows=json.load(open(messages_path))
for i,row in enumerate(rows):
 m={'id':row['id'],'threadId':thread_id,'runId':None,'nodeId':None,'role':row['role'],'text':row['text'],'attachments':[],'streaming':False,'createdAt':row['createdAt'],'updatedAt':row['createdAt'],'createdBy':'user' if row['role']=='user' else 'agent','creationSource':'provider'}
 c.execute('insert into orchestration_v2_projection_messages values (?,?,?,?,?,?,?,?,?)',(m['id'],thread_id,None,None,m['role'],0,m['createdAt'],m['updatedAt'],json.dumps(m)))
 item={'id':'item-'+row['id'],'threadId':thread_id,'runId':None,'nodeId':None,'providerThreadId':None,'providerTurnId':None,'nativeItemRef':None,'parentItemId':None,'ordinal':i+1,'status':'completed','title':None,'startedAt':row['createdAt'],'completedAt':row['createdAt'],'updatedAt':row['createdAt'],'type':'user_message' if row['role']=='user' else 'assistant_message','messageId':row['id'],'text':row['text'],'attachments':[]}
 if row['role']=='user': item.update(inputIntent='turn_start',createdBy='user',creationSource='web')
 else: item['streaming']=False
 c.execute('insert into orchestration_v2_projection_turn_items values (?,?,?,?,?,?,?,?,?,?,?,?)',(item['id'],thread_id,None,None,None,None,None,i+1,item['type'],'completed',row['createdAt'],json.dumps(item)))
c.commit()
assert c.execute('select count(*) from orchestration_v2_projection_messages where thread_id=?',(thread_id,)).fetchone()[0]==len(rows)
`,path.join(seedRoot,'t3/userdata/statev2.sqlite'),path.join(root,'canonical-history.json'),threadId]);
    const liveRoot=path.join(root,'t3-runtime');fs.mkdirSync(liveRoot,{recursive:true});
    t3=await t3Server(liveRoot,{provider,workspace:project,template:path.join(seedRoot,'t3')});
    const snapshot=await t3.call('orchestration.getThreadProjection',{threadId});
    fs.writeFileSync(path.join(root,'t3-initial-projection.json'),JSON.stringify(snapshot));
    const t3Messages=snapshot.messages ?? snapshot.projection?.messages;
    assert(t3Messages?.length>0,'T3 initial projection contains history');
    const canonical=new Map(messages.map(message=>[message.id,message]));
    for(const message of t3Messages){assert.equal(message.role,canonical.get(message.id)?.role);assert.equal(message.text,canonical.get(message.id)?.text);}
    return {zuse,t3,chatId,threadId,project,provider,messages,fixture:{messages:count,legacyFullHistoryBytes:Buffer.byteLength(JSON.stringify(zuseHistory)),historyTextBytes:messages.reduce((sum,m)=>sum+Buffer.byteLength(m.text),0),canonicalSha256:createHash('sha256').update(JSON.stringify(messages)).digest('hex'),roles:'alternating user / assistant',t3Seed:'native stopped projection database; full retained count verified in SQLite; initial RPC page text and roles verified',zuseSeed:'Claude transcript import; original canonical text and roles verified through RPC'}};
  }catch(error){await t3?.close();await zuse.close();throw error;}
}
