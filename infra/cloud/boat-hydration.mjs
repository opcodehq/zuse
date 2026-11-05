// Official GET sandbox hydration is distinct from command-ready. No mutations,
// guessed delay, restart, TTL update, fallback allocation or service keepalive.
export async function waitBoatHydration(adapter,id,name,{deadlineMs=300000,sleep=ms=>new Promise(r=>setTimeout(r,ms)),clock=Date.now,onObservation=async()=>{}}={}){
 const until=clock()+deadlineMs;
 for(let attempt=1;attempt<=301;attempt++){
  const v=await adapter.identity(id);
  if(v.id!==id||v.name!==name)throw new Error('OriginalBuilderIdentityMismatch');
  const fact={id,name,state:v.state,hydrated:v.hydrated===true,attempt};
  if(typeof v.hydratedAt==='string'||typeof v.hydratedAt==='number')fact.hydratedAt=v.hydratedAt;
  await onObservation(fact);
  if(v.hydrated===true)return fact;
  if(v.hydrated!==false)throw new Error('ProviderHydrationUnavailable');
  if(clock()>=until)break;
  await sleep(Math.min(1000,Math.max(0,until-clock())));
 }
 throw new Error('OriginalHydrationPending');
}
