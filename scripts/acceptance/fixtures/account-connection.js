// Account/endpoint response boundary only. Workspace and chunk uploads use the
// disposable real engine; the compiled application owns routing and lifecycle.
(()=>{
  if(new URLSearchParams(location.search).has('fixtureNoAccountStorage')){
    const set=Storage.prototype.setItem;
    Storage.prototype.setItem=function(key,value){if(key==='zuse.account-target.v1')throw new DOMException('Storage unavailable','QuotaExceededError');return set.call(this,key,value);};
  }
  const Native=WebSocket;
  const proof=window.__accountConnectionProof={sockets:[],calls:[],grants:0,renewals:0,pings:0,remembered:0,minted:[],held:0,holdGrants:new URLSearchParams(location.search).has("fixtureHoldGrant"),rejectRenewals:false};
  const tokens=new Map();
  const remote='7a5ef180-af4a-44a3-b86b-e1ce925eb827',second='39b98f91-2e8a-4da2-b94c-467ef55114ac',subject='account-fixture-user',secondSubject='second-fixture-user';
  window.WebSocket=class extends Native {
    constructor(url,protocol){
      const account=new URL(url).pathname==='/api/account-rpc';
      super(account?`${location.protocol==='https:'?'wss':'ws'}://${location.host}/api/rpc`:url);
      this.account=account;this.environment=account?(new URL(url).hostname==='second-account-fixture.invalid'?second:remote):null;this.authenticated=false;this.watch=null;
      proof.sockets.push(this);this.requestedProtocol=protocol;
      this.addEventListener('message',event=>{
        try {const f=JSON.parse(event.data);if(account&&f.id&&f.ok&&f.ok.path)proof.uploadPath=f.ok.path;}catch{}
      });
    }
    reply(value){queueMicrotask(()=>this.dispatchEvent(new MessageEvent('message',{data:JSON.stringify(value)})));}
    send(text){
      let f;try{f=JSON.parse(text);}catch{return super.send(text);}
      if(f.protocol){
        if(!this.account)throw new Error('Account control reached the controller transport');
        if(f.ping){proof.pings++;this.reply({protocol:f.protocol,pong:f.ping});return;}
        const grant=tokens.get(f.connectToken);
        if(!grant||grant.environment!==this.environment){this.close();return;}
        const renewed=this.authenticated;this.authenticated=true;if(renewed)proof.renewals++;
        this.reply({authenticated:true,protocol:f.protocol,deviceId:this.environment,expiresAt:grant.expiresAt,renewed});return;
      }
      if(f.method)proof.calls.push({method:f.method,account:this.account,environment:this.environment,id:f.id});
      if(this.account){
        if(!this.authenticated)throw new Error('RPC preceded account authorization');
        if(f.method==='EngineInfo'||f.method==='LocalDevice'){
          this.reply({id:f.id,ok:{deviceId:this.environment,hostname:'Account fixture',engineId:this.environment,version:'0.25.0'}});return;
        }
        if(f.method==='QueueCommand'||f.method==='QueueMessage')throw new Error('Unexpected agent write');
      }
      if(f.method==='CloudAccountStatus'){
        this.watch=f.id;this.reply({id:f.id,item:{user:this.environment===second?null:{id:this.account?secondSubject:subject,email:'fixture@example.invalid'}}});return;
      }
      if(f.method==='ListAccountEnvironments'){
        this.reply({id:f.id,ok:{environments:[{environmentId:this.account?second:remote,label:'Account fixture computer',serviceState:'online',wireProtocolVersion:1,capabilities:{version:1,features:['zuse-rpc-json-v1']}}]}});return;
      }
      if(f.method==='ListAccountDevices'){this.reply({id:f.id,ok:{clients:[]}});return;}
      if(f.method==='ListAccountRoutes'){this.reply({id:f.id,ok:{computers:[]}});return;}
      if(f.method==='BootstrapAccountConnection'){
        const environment=this.account?second:remote,expected=this.account?secondSubject:subject;
        if(this.environment===second||f.params.expectedSubject!==expected||f.params.environmentId!==environment)throw new Error('Unfenced account bootstrap');
        if(proof.holdGrants){proof.held++;return;}
        if(proof.rejectRenewals){this.reply({id:f.id,err:'account_connection.rejected'});return;}
        proof.grants++;const token=`account-fixture-grant-${proof.grants}`,expiresAt=Date.now()+3000;tokens.set(token,{expiresAt,environment});proof.minted.push({source:this.environment,environment,subject:expected});
        this.reply({id:f.id,ok:{environmentId:environment,origin:this.account?'https://second-account-fixture.invalid':'https://account-fixture.invalid',connectToken:token,expiresAt}});return;
      }
      if(f.method==='RememberAccountRoute'){
        if(f.params.origin!==(this.account?'https://second-account-fixture.invalid':'https://account-fixture.invalid')||f.params.expectedSubject!==(this.account?secondSubject:subject))throw new Error('Wrong remembered computer');
        proof.remembered++;this.reply({id:f.id,ok:{computers:[]}});return;
      }
      super.send(text);
    }
  };
})();
