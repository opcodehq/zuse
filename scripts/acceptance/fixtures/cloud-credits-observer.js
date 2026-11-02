// Observe the real credit UI without retaining checkout session URLs or account credentials.
(() => {
  try{localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));}catch{}
  const Native=WebSocket,pending=new Map();const proof=window.__creditsProof={methods:[],balances:[],checkouts:[],opened:[],errors:[]};const open=window.open.bind(window);
  window.open=(url,...args)=>{try{const parsed=new URL(url,location.href);if(parsed.hostname==='stripe.example.test'){proof.opened.push(parsed.hostname);return null;}}catch{}return open(url,...args);};
  window.WebSocket=class extends Native{
    constructor(...args){super(...args);this.addEventListener('message',event=>{try{const frame=JSON.parse(event.data),request=pending.get(frame.id);if(!request)return;if(frame.err){proof.errors.push({method:request.method,error:String(frame.err)});return;}if(request.method==='CloudPrepaidBalance'&&frame.ok)proof.balances.push(frame.ok.data);if(request.method==='CloudPrepaidCheckout'&&frame.ok)proof.checkouts.push({amountCents:request.params.amountCents,accountId:frame.ok.accountId,providerHost:new URL(frame.ok.data.checkoutUrl).hostname});}catch{}});}
    send(value){try{const frame=JSON.parse(value);if(frame.method){proof.methods.push(frame.method);pending.set(frame.id,{method:frame.method,params:frame.params});}if(frame.cancel)pending.delete(frame.id);}catch{}super.send(value);}
  };
})();
