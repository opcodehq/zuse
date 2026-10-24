import fs from 'node:fs';
import path from 'node:path';
export function importChatData(root){
 const project=path.join(root,'Sample project'),claude=path.join(root,'claude'),codex=path.join(root,'codex');
 fs.mkdirSync(project,{recursive:true});fs.writeFileSync(path.join(project,'README.md'),'Original file bytes');
 const source=path.join(claude,'projects/project/original-claude-session.jsonl');fs.mkdirSync(path.dirname(source),{recursive:true});
 const timestamp=new Date().toISOString();
 const rows=[
  {type:'user',uuid:'user-1',timestamp,sessionId:'original-claude-session',cwd:project,message:{role:'user',content:'Keep the original history'}},
  {type:'assistant',uuid:'assistant-1',timestamp,message:{role:'assistant',content:[{type:'text',text:'Before the tool'},{type:'tool_use',id:'read-1',name:'Read',input:{file_path:'README.md'}}]}},
  {type:'user',uuid:'tool-result',timestamp,message:{role:'user',content:[{type:'tool_result',tool_use_id:'read-1',content:'Original file bytes'}]}},
  {type:'assistant',uuid:'assistant-2',timestamp,message:{role:'assistant',content:[{type:'text',text:'After the tool'}]}},
  {type:'summary',summary:'Imported design chat',sessionId:'original-claude-session',cwd:project},
 ];
 const bytes=rows.map(row=>JSON.stringify(row)).join('\n');fs.writeFileSync(source,bytes);
 const archived=path.join(codex,'archived_sessions/rollout-native.jsonl');fs.mkdirSync(path.dirname(archived),{recursive:true});
 fs.writeFileSync(archived,[{type:'session_meta',payload:{id:'original-codex-thread',cwd:project}},{type:'response_item',payload:{type:'message',role:'user',content:[{type:'input_text',text:'Original Codex question'}]}},{type:'response_item',payload:{type:'message',role:'assistant',content:[{type:'output_text',text:'Original Codex answer'}]}}].map(JSON.stringify).join('\n'));
 fs.writeFileSync(path.join(codex,'session_index.jsonl'),JSON.stringify({id:'original-codex-thread',thread_name:'Archived provider conversation'}));
 const executable=path.resolve('scripts/acceptance/fixtures/import-provider.py');fs.chmodSync(executable,0o755);
 const seen=path.join(root,'provider-seen.jsonl');
 return {project,source,bytes,seen,env:{CLAUDE_CONFIG_DIR:claude,CODEX_HOME:codex,CLAUDE_CODE_EXECUTABLE:executable,ZUSE_IMPORT_PROVIDER_SEEN:seen,ZUSE_NO_LOGIN_SHELL:'1'}};
}
