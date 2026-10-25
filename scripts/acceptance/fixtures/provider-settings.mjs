// Deterministic execution-host contracts; no installed CLI or login service is invoked.
export function providerSettingsFixture(events) {
  let enabled = false, hold = false, accounts = [], usageError = null, holdUsage = false;
  const usageWaiting = [];
  const waiting = [], watches = new Map();
  let modelErrors = {};
  let loginMode='paste-code',loginSequence=0,pollsBeforeDone=null,loginError=null,codeError=null,holdStart=false;
  const logins=new Map(),startWaiting=[];
  let holdAccountActions=false,accountActionError=null;
  const accountWaiting=[];
  const accountFor=harness=>({id:'signed-'+harness,harness,email:'sam@example.test',planLabel:'Pro',active:true,usageWindows:[],switchable:true});
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
  const snapshot = () => ({enabled,hold,descriptors,models,updates,modelErrors,accounts,loginMode,logins:[...logins].map(([id,login])=>({id,...login}))});
  const reply = (send,client,frame) => hold ? waiting.push([send,client,frame]) : send(client,frame);
  return {
    snapshot,
    configure(value) { if(Object.hasOwn(value,'accountActionError'))accountActionError=value.accountActionError;if(Object.hasOwn(value,'holdAccountActions'))holdAccountActions=value.holdAccountActions;if(!holdAccountActions)for(const action of accountWaiting.splice(0))action(); if(Object.hasOwn(value,'loginMode'))loginMode=value.loginMode;if(Object.hasOwn(value,'pollsBeforeDone'))pollsBeforeDone=value.pollsBeforeDone;if(Object.hasOwn(value,'loginError'))loginError=value.loginError;if(Object.hasOwn(value,'codeError'))codeError=value.codeError;if(Object.hasOwn(value,'holdStart'))holdStart=value.holdStart;if(!holdStart)for(const[send,client,frame]of startWaiting.splice(0))send(client,frame); if(value.models)Object.assign(models,value.models); if(value.modelErrors)modelErrors=value.modelErrors; enabled=true; hold=value.hold??false; if(value.accounts)accounts=value.accounts; if(Object.hasOwn(value,'usageError'))usageError=value.usageError; if(Object.hasOwn(value,'holdUsage'))holdUsage=value.holdUsage; if(!holdUsage)for(const [send,client,frame]of usageWaiting.splice(0))send(client,frame); if(!hold)for(const [send,client,frame] of waiting.splice(0))send(client,frame); },
    handle(client,frame,send) {
      if(frame.cancel && watches.get(client)?.delete(frame.id))return true;
      if(!enabled)return false;
      const {method,params:p={}}=frame;
      if(!['ListHarnesses','SetHarnessEnabled','ListModels','ListAgentAccounts','ActivateAgentAccount','ForgetAgentAccount','WatchHarnessUpdates','CheckHarnessUpdates','StartAgentLogin','PollAgentLogin','CompleteAgentLogin','CancelAgentLogin'].includes(method))return false;
      events.push({method,params:p});
      let data;
      if(method==='ActivateAgentAccount'||method==='ForgetAgentAccount'){
        const action=()=>{
          const account=accounts.find(a=>a.id===p.accountId&&a.harness===p.harness);
          const error=accountActionError??(!account?.switchable?'This account cannot be changed.':null);
          if(error){reply(send,client,{id:frame.id,err:error,done:true});return;}
          if(method==='ForgetAgentAccount')accounts=accounts.filter(a=>a.id!==account.id);
          else for(const row of accounts)if(row.harness===account.harness&&row.provider===account.provider)row.active=row.id===account.id;
          reply(send,client,{id:frame.id,ok:{accounts:structuredClone(accounts),warnings:[]}});
        };
        if(holdAccountActions)accountWaiting.push(action);else action();return true;
      }
      if(method==='ListHarnesses')data=descriptors;
      if(method==='ListModels'){
        if(modelErrors[p.harness]){reply(send,client,{id:frame.id,err:modelErrors[p.harness],done:true});return true;}
        data=models[p.harness]??[];
      }
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
      if(method==='StartAgentLogin'){
        const loginId='provider-login-'+(++loginSequence);logins.set(loginId,{harness:p.harness,polls:0,cancelled:false,done:false});
        const response=loginError?{id:frame.id,err:loginError,done:true}:{id:frame.id,ok:{loginId,url:'https://identity.example.test/provider',mode:loginMode}};
        if(holdStart)startWaiting.push([send,client,response]);else reply(send,client,response);return true;
      }
      if(method==='PollAgentLogin'){
        const login=logins.get(p.loginId);if(login){login.polls++;if(pollsBeforeDone!==null&&login.polls>=pollsBeforeDone&&!login.cancelled){login.done=true;accounts=accounts.filter(a=>a.harness!==login.harness);accounts.push(accountFor(login.harness));}}
        data={status:login?.cancelled?'error':login?.done?'done':'pending'};
      }
      if(method==='CompleteAgentLogin'){
        if(codeError){reply(send,client,{id:frame.id,err:codeError,done:true});return true;}
        const login=logins.get(p.loginId);if(login&&!login.cancelled){login.done=true;accounts=accounts.filter(a=>a.harness!==login.harness);accounts.push(accountFor(login.harness));}data={ok:true};
      }
      if(method==='CancelAgentLogin'){const login=logins.get(p.loginId);if(login)login.cancelled=true;data={ok:true};}
      reply(send,client,{id:frame.id,ok:data});return true;
    },
  };
}
