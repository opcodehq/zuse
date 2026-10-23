// Observe real portal RPC and navigation without retaining provider session URLs.
(() => {
  try {localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));} catch {}
  const Native=WebSocket,pending=new Map();
  const proof=window.__portalProof={methods:[],portals:[],opened:[],errors:[]};
  const open=window.open.bind(window);
  window.open=(url,...args)=>{
    try { const parsed=new URL(url,location.href); if(parsed.hostname==='stripe.example.test'||parsed.hostname==='polar.example.test') {proof.opened.push(parsed.hostname);return null;} } catch {}
    return open(url,...args);
  };
  window.WebSocket=class extends Native {
    constructor(...args){super(...args);this.addEventListener('message',event=>{try {
      const frame=JSON.parse(event.data),request=pending.get(frame.id);if(!request)return;
      if(frame.err){proof.errors.push({method:request,error:String(frame.err)});return;}
      if(request==='CloudBillingPortal'&&frame.ok)proof.portals.push({accountId:frame.ok.accountId,organizationId:frame.ok.organizationId,providerHost:new URL(frame.ok.data.portalUrl).hostname});
    }catch{}});}
    send(value){try{const frame=JSON.parse(value);if(frame.method){proof.methods.push(frame.method);pending.set(frame.id,frame.method);}if(frame.cancel)pending.delete(frame.id);}catch{}super.send(value);}
  };
})();
