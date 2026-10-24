// Deterministic execution-host contracts; no installed CLI or login service is invoked.
export function providerSettingsFixture(events) {
  let enabled = false, hold = false, accounts = [], usageError = null, holdUsage = false;
  const usageWaiting = [];
  const waiting = [], watches = new Map();
  const descriptors = [
    ['claude-code', 'Claude Code', true, true],
    ['codex', 'Codex', true, true],
    ['gemini', 'Gemini CLI', false, false],
  ].map(([id,name,installed,on]) => ({id,name,installed,enabled:on,canInstall:true,supportsSteering:false,steeringMode:'turn-boundary',reasoningLevels:[]}));
  const models = Object.fromEntries(descriptors.map(p => [p.id, [
    {id:'balanced',label:p.id==='codex'?'Codex Balanced':'Balanced',description:'Everyday coding',reasoningLevels:[],options:[]},
    {id:'deep',label:p.id==='codex'?'Codex Deep':'Deep',description:'Complex reasoning',reasoningLevels:[],options:[]},
  ]]));
  const updates = descriptors.filter(p=>p.installed).map(p=>({harness:p.id,installedVersion:'1.2.3',latestVersion:'1.2.3',phase:'current',policy:'notify',checkedAt:Date.now(),canApply:false}));
  const snapshot = () => ({enabled,hold,descriptors,models,updates});
  const reply = (send,client,frame) => hold ? waiting.push([send,client,frame]) : send(client,frame);
  return {
    snapshot,
    configure(value) { enabled=true; hold=value.hold??false; if(value.accounts)accounts=value.accounts; if(Object.hasOwn(value,'usageError'))usageError=value.usageError; if(Object.hasOwn(value,'holdUsage'))holdUsage=value.holdUsage; if(!holdUsage)for(const [send,client,frame]of usageWaiting.splice(0))send(client,frame); if(!hold)for(const [send,client,frame] of waiting.splice(0))send(client,frame); },
    handle(client,frame,send) {
      if(frame.cancel && watches.get(client)?.delete(frame.id))return true;
      if(!enabled)return false;
      const {method,params:p={}}=frame;
      if(!['ListHarnesses','SetHarnessEnabled','ListModels','ListAgentAccounts','WatchHarnessUpdates','CheckHarnessUpdates','StartAgentLogin','PollAgentLogin','CompleteAgentLogin','CancelAgentLogin'].includes(method))return false;
      events.push({method,params:p});
      let data;
      if(method==='ListHarnesses')data=descriptors;
      if(method==='ListModels')data=models[p.harness]??[];
      if(method==='ListAgentAccounts'){
        const response=usageError?{id:frame.id,err:usageError,done:true}:{id:frame.id,ok:{accounts:structuredClone(accounts),warnings:[]}};
        if(holdUsage)usageWaiting.push([send,client,response]);else reply(send,client,response);return true;
      }
      if(method==='CheckHarnessUpdates')data=updates;
      if(method==='WatchHarnessUpdates'){
        let ids=watches.get(client);if(!ids)watches.set(client,ids=new Set());ids.add(frame.id);
        send(client,{id:frame.id,item:updates});return true;
      }
      if(method==='SetHarnessEnabled'){
        const provider=descriptors.find(v=>v.id===p.harness);
        if(!provider || (!provider.installed&&p.enabled) || (!p.enabled&&descriptors.filter(v=>v.enabled&&v.installed).length===1)){
          send(client,{id:frame.id,err:'Keep at least one installed agent enabled.',done:true});return true;
        }
        provider.enabled=p.enabled;data=descriptors;
      }
      if(method==='StartAgentLogin')data={loginId:'provider-login',url:'https://identity.example.test/provider',mode:'paste-code'};
      if(method==='PollAgentLogin')data={status:'pending',message:'Waiting for sign-in…'};
      if(method==='CompleteAgentLogin'||method==='CancelAgentLogin')data={ok:true};
      reply(send,client,{id:frame.id,ok:data});return true;
    },
  };
}
