// Synthetic recorded history for rendering and transport acceptance; no provider is contacted.
const now=new Date(),midnight=new Date(now);midnight.setUTCHours(0,0,0,0);
const day=86400000;
const fields=['inputTokens','outputTokens','cacheReadTokens','cacheCreationTokens','reasoningTokens'];
const counts=(factor=1)=>({inputTokens:1250*factor,outputTokens:420*factor,cacheReadTokens:8000*factor,cacheCreationTokens:320*factor,reasoningTokens:90*factor,costUsd:.125*factor,costStatus:'known',recordCount:factor,possibleDuplicateCount:0});
const total=rows=>{
 const out={...counts(0)};
 for(const row of rows){for(const k of [...fields,'costUsd','recordCount'])out[k]+=row[k];}
 return out;
};
export const accounts={warnings:[],accounts:['claude-code','codex','grok','gemini','kiro'].map((harness,i)=>({id:'usage-'+harness,harness,email:'fixture@example.test',planLabel:'Pro',active:true,usageWindows:[{label:'Weekly',usedFraction:(20+i*12)/100,resetsAt:new Date(midnight.getTime()+day+12*3600000).toISOString()}],usageFetchedAt:now.getTime(),switchable:false}))};
export const sessions=Array.from({length:24},(_,i)=>{
 const started=new Date(midnight.getTime()-(i%7)*day+10*3600000),ended=new Date(started.getTime()+3600000);
 return {key:'fixture-session-'+i,label:`Recorded task ${String(i+1).padStart(2,'0')}`,startedAt:started.toISOString(),endedAt:ended.toISOString(),sourceIds:[i%2?'codex':'claude'],project:i%3===0?'other-project':'archive-project',model:'model-'+i%7,...counts(24-i)};
});
export function select(p={}){
 return sessions.filter(r=>(!p.projectId||r.project===p.projectId)&&(!p.providerId||r.sourceIds.includes(p.providerId))&&(!p.query||JSON.stringify(r).toLowerCase().includes(p.query.toLowerCase()))&&(!p.since||r.endedAt>=p.since)&&(!p.until||r.endedAt<p.until));
}
export function page(p={}){
 const rows=select(p).sort((a,b)=>p.sort==='last-active'?b.endedAt.localeCompare(a.endedAt):p.sort==='cost'?b.costUsd-a.costUsd:fields.reduce((v,k)=>v+b[k]-a[k],0));
 const offset=p.offset??0,limit=p.limit??10,end=Math.min(rows.length,offset+limit);
 return {rows:rows.slice(offset,end),total:rows.length,nextOffset:end<rows.length?end:null};
}
export function overview(p={}){
 const rows=select({...p,query:'',providerId:null}),summary=total(rows);
 const group=(key,label,subset,extra={})=>({key,label,sourceIds:[...new Set(subset.flatMap(r=>r.sourceIds))],...extra,...total(subset)});
 const groups=Array.from({length:7},(_,i)=>{const start=new Date(midnight.getTime()-(6-i)*day),end=new Date(start.getTime()+day);return group(start.toISOString().slice(0,10),start.toLocaleDateString('en-US',{month:'short',day:'numeric',timeZone:'UTC'}),rows.filter(r=>r.endedAt>=start.toISOString()&&r.endedAt<end.toISOString()),{startedAt:start.toISOString(),endedAt:end.toISOString()});});
 const bySource=['claude','codex'].map(id=>group(id,id==='claude'?'Claude Code':'Codex',rows.filter(r=>r.sourceIds.includes(id))));
 const byModel=Array.from({length:7},(_,i)=>group('model-'+i,'Model '+(i+1),rows.filter(r=>r.model==='model-'+i)));
 const byProject=['archive-project','other-project'].map((id,i)=>group(id,i?'Other project':'Archive project',rows.filter(r=>r.project===id)));
 return {bucket:'daily',generatedAt:now.toISOString(),summary,sessionCount:rows.length,previousSummary:{...summary,costUsd:summary.costUsd*.76},previousSessionCount:18,groups,bySource,byModel,byProject,previousBySource:[],previousByModel:[],previousByProject:[],sources:bySource.map(r=>({id:r.key,label:r.label,detected:true,recordCount:r.recordCount,paths:['/fixture/recorded-history'],warning:null}))};
}
