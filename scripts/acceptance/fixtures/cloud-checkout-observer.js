// Observe checkout RPC/navigation without retaining the provider session URL or credentials.
(() => {
  try{localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));}catch{}
  const Native=WebSocket,pending=new Map();
  const proof=window.__checkoutProof={methods:[],checkouts:[],entitlements:[],opened:[],errors:[]};
  const open=window.open.bind(window);
  window.open=(url,...args)=>{try{const parsed=new URL(url,location.href);if(parsed.hostname==='stripe.example.test'){proof.opened.push(parsed.hostname);return null;}}catch{}return open(url,...args);};
  window.WebSocket=class extends Native{
    constructor(...args){super(...args);this.addEventListener('message',event=>{try{
      const frame=JSON.parse(event.data),method=pending.get(frame.id);if(!method)return;
      if(frame.err){proof.errors.push({method,error:String(frame.err)});return;}
      if(method==='CloudBillingEntitlements'&&frame.ok)proof.entitlements.push(frame.ok.data.entitlements);
      if(method==='CloudBillingCheckout'&&frame.ok)proof.checkouts.push({accountId:frame.ok.accountId,organizationId:frame.ok.organizationId,providerHost:new URL(frame.ok.data.checkoutUrl).hostname});
    }catch{}});}
    send(value){try{const frame=JSON.parse(value);if(frame.method){proof.methods.push(frame.method);pending.set(frame.id,frame.method);}if(frame.cancel)pending.delete(frame.id);}catch{}super.send(value);}
  };
})();
