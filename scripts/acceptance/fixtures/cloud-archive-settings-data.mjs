export const draft='Keep this draft while reviewing Cloud archives.';
export const workspace=(id,title,branch,state='archived',extra={})=>({workspaceId:id,chatId:'arc-release',initialSessionId:'arc-release',branch,state,desiredState:'archived',statusCode:state,revision:1,title,rpcProtocol:'zuse-rpc-v1',...extra});
export const initialRows=[workspace('ws-release','Release workspace','release/v3'),workspace('ws-pending','Search indexing','feature/search','archiving')];
export const deleteRow=workspace('ws-delete','Old deployment','release/old','deleting',{desiredState:'deleted',statusCode:'cloud_object_purge_failed'});
export const cleanupRow=workspace('ws-cleanup','Finished workspace','cleanup/local','deleted',{desiredState:'deleted',statusCode:'deleted',localMirrorCleanup:{state:'error',removed:2,preserved:1,error:'The local copy is unavailable. Check the folder and retry.'}});
export async function seed(host){
 await host.call('SignOut');const info=await host.call('EngineInfo');
 for(const [chatId,title]of[['cloud-live','Live conversation'],['arc-release','Local release notes']]){
  await host.call('Mutate',{op:'createChat',chatId,deviceId:info.deviceId,cwd:host.root});await host.call('Mutate',{op:'renameChat',chatId,title});
 }
 await host.call('CreateThreadSession',{chatId:'arc-release-tab',threadId:'arc-release'});
 await host.call('Mutate',{op:'renameChat',chatId:'arc-release-tab',title:'Rollback checklist'});
 await host.call('Mutate',{op:'setChatArchived',chatId:'arc-release',archived:true});
 const chats=(await host.call('WatchChats')).filter(chat=>chat.id==='arc-release'||chat.threadId==='arc-release');
 return {rows:initialRows,chats};
}
