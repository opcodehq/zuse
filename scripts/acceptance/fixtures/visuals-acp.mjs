import fs from 'node:fs';import readline from 'node:readline';
const [port,chat,source]=process.argv.slice(2);
function publish(){return new Promise((resolve,reject)=>{const ws=new WebSocket(`ws://127.0.0.1:${port}`);const timer=setTimeout(()=>{ws.close();reject(new Error('Publish deadline'));},15000);ws.onopen=()=>ws.send(JSON.stringify({id:1,method:'PublishVisual',params:{chatId:chat,title:'Interactive activity',script:fs.readFileSync(source,'utf8')}}));ws.onerror=()=>reject(new Error('Publish connection'));ws.onmessage=e=>{const f=JSON.parse(e.data);if(f.id!==1)return;clearTimeout(timer);ws.onerror=()=>{};ws.close();if(f.err)reject(new Error(String(f.err)));else resolve(f.ok);};});}
for await(const line of readline.createInterface({input:process.stdin})){
 const f=JSON.parse(line);let result={};
 if(f.method==='initialize')result={protocolVersion:1,agentCapabilities:{loadSession:true}};
 else if(f.method==='session/new'||f.method==='session/load')result={sessionId:'visual-fixture',models:{currentModelId:'fixture/model',availableModels:[{modelId:'fixture/model',name:'Fixture'}]}};
 else if(f.method==='session/prompt'){await publish();result={stopReason:'end_turn'};}
 if(f.id!==undefined)process.stdout.write(JSON.stringify({id:f.id,result})+'\n');
}
