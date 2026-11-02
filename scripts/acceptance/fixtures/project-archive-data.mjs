import fs from 'node:fs';
export const archiveDates={
 'arc-release':'2026-10-20T12:00:00Z','arc-release-tab':'2026-10-20T12:00:00Z',
 'arc-design':'2024-10-04T12:00:00Z','arc-other':'2026-10-21T12:00:00Z',
};
export const archiveMetadata={
 'arc-release':{lastMessagePreview:'Keep the migration incremental and preserve the existing account history.',branch:'release/v2'},
 'arc-design':{lastMessagePreview:'Explore a cleaner settings layout and navigation.',branch:'design/settings'},
};
export const archiveMessages=id=>[
 {id:id+'-question',role:'user',parts:[{kind:'text',id:'question',text:id==='arc-release-tab'?'What changed in the second conversation?':'Review the saved release decision.'}],createdAt:1792497600000,deviceId:'fixture-device'},
 {id:id+'-answer',role:'assistant',status:'complete',parts:[{kind:'text',id:'answer',text:id==='arc-release-tab'?'The second conversation preserves the rollback checklist.':'## Saved release decision\n\nKeep the migration incremental and preserve the existing account history.\n\n- Verify the archive before restoring.\n- Keep unrelated conversations unchanged.'}],createdAt:1792497601000,deviceId:'fixture-device'},
];
export async function seedProjectArchives(host){
 const info=await host.call('EngineInfo');
 for(const [spaceId,name]of[['archive-project','Archive project'],['other-project','Other project']]){
  const folder=host.root+'/'+spaceId;fs.mkdirSync(folder);
  await host.call('Mutate',{op:'createSpace',spaceId,deviceId:info.deviceId,path:folder,name});
 }
 for(const [chatId,spaceId,title]of[['live-draft','archive-project','Live conversation'],['arc-release','archive-project','Release decision'],['arc-design','archive-project','Design notes'],['arc-other','other-project','Other project archive']]){
  await host.call('Mutate',{op:'createChat',chatId,spaceId,cwd:host.root+'/'+spaceId});
  await host.call('Mutate',{op:'renameChat',chatId,title});
 }
 await host.call('CreateThreadSession',{chatId:'arc-release-tab',threadId:'arc-release'});
 await host.call('Mutate',{op:'renameChat',chatId:'arc-release-tab',title:'Rollback checklist'});
 for(const chatId of ['arc-release','arc-design','arc-other'])await host.call('Mutate',{op:'setChatArchived',chatId,archived:true});
 return {deviceId:info.deviceId,project:host.root+'/archive-project'};
}
