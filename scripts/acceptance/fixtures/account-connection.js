// Account/endpoint response boundary only. Workspace and chunk uploads use the
// disposable real engine; the compiled application owns routing and lifecycle.
(()=>{
  if(new URLSearchParams(location.search).has('fixtureNoAccountStorage')){
    const set=Storage.prototype.setItem;
    Storage.prototype.setItem=function(key,value){if(key==='zuse.account-target.v1')throw new DOMException('Storage unavailable','QuotaExceededError');return set.call(this,key,value);};
  }
  const Native=WebSocket;
  const proof=window.__accountConnectionProof={sockets:[],calls:[],grants:0,renewals:0,pings:0,remembered:0,rejectRenewals:false};
  const tokens=new Map();
  const remote='7a5ef180-af4a-44a3-b86b-e1ce925eb827',subject='account-fixture-user';
  window.WebSocket=class extends Native {
    constructor(url,protocol){
      const account=new URL(url).pathname==='/api/account-rpc';
      super(account?`${location.protocol==='https:'?'wss':'ws'}://${location.host}/api/rpc`:url);
      this.account=account;this.authenticated=false;this.watch=null;
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
        if(!grant){this.close();return;}
        const renewed=this.authenticated;this.authenticated=true;if(renewed)proof.renewals++;
        this.reply({authenticated:true,protocol:f.protocol,deviceId:remote,expiresAt:grant,renewed});return;
      }
      if(f.method)proof.calls.push({method:f.method,account:this.account,id:f.id});
      if(this.account){
        if(!this.authenticated)throw new Error('RPC preceded account authorization');
        if(f.method==='EngineInfo'||f.method==='LocalDevice'){
          this.reply({id:f.id,ok:{deviceId:remote,hostname:'Account fixture',engineId:remote,version:'0.25.0'}});return;
        }
        if(f.method==='CloudAccountStatus'){this.reply({id:f.id,item:{user:null}});return;}
        if(f.method==='QueueCommand'||f.method==='QueueMessage')throw new Error('Unexpected agent write');
        return super.send(text);
      }
      if(f.method==='CloudAccountStatus'){
        this.watch=f.id;this.reply({id:f.id,item:{user:{id:subject,email:'fixture@example.invalid'}}});return;
      }
      if(f.method==='ListAccountEnvironments'){
        this.reply({id:f.id,ok:{environments:[{environmentId:remote,label:'Account fixture computer',serviceState:'online',wireProtocolVersion:1,capabilities:{version:1,features:['zuse-rpc-json-v1']}}]}});return;
      }
      if(f.method==='ListAccountDevices'){this.reply({id:f.id,ok:{clients:[]}});return;}
      if(f.method==='ListAccountRoutes'){this.reply({id:f.id,ok:{computers:[]}});return;}
      if(f.method==='BootstrapAccountConnection'){
        if(f.params.expectedSubject!==subject||f.params.environmentId!==remote)throw new Error('Unfenced account bootstrap');
        if(proof.rejectRenewals){this.reply({id:f.id,err:'account_connection.rejected'});return;}
        proof.grants++;const token=`account-fixture-grant-${proof.grants}`,expiresAt=Date.now()+3000;tokens.set(token,expiresAt);
        this.reply({id:f.id,ok:{environmentId:remote,origin:'https://account-fixture.invalid',connectToken:token,expiresAt}});return;
      }
      if(f.method==='RememberAccountRoute'){
        if(f.params.origin!=='https://account-fixture.invalid'||f.params.expectedSubject!==subject)throw new Error('Wrong remembered computer');
        proof.remembered++;this.reply({id:f.id,ok:{computers:[]}});return;
      }
      super.send(text);
    }
  };
})();
