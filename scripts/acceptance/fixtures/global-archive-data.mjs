// Real saved-chat metadata; transcript previews use the project archive fixture.
import {seedProjectArchives} from './project-archive-data.mjs';
export async function seedGlobalArchives(host){
 const data=await seedProjectArchives(host);
 for(let n=1;n<=39;n++){
  const chatId='library-'+String(n).padStart(2,'0');
  await host.call('Mutate',{op:'createChat',chatId,spaceId:'archive-project',cwd:data.project,...(n===1?{parentChatId:'live-draft'}:{})});
  await host.call('Mutate',{op:'renameChat',chatId,title:'Saved session '+String(n).padStart(2,'0')});
  await host.call('Mutate',{op:'setChatArchived',chatId,archived:true});
 }
 const devices=await host.call('WatchDevices');
 return {...data,computer:devices.find(d=>d.id===data.deviceId)?.name,total:42};
}
