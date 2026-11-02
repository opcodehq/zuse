// Observe account billing results from the real UI, without account credentials.
(() => {
  try {localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));} catch {}
  const Native=WebSocket,pending=new Map();
  const proof=window.__billingProof={methods:[],summaries:[],pages:[],caps:[],errors:[]};
  window.WebSocket=class extends Native {
    constructor(...args){super(...args);this.addEventListener('message',event=>{try {
      const frame=JSON.parse(event.data),request=pending.get(frame.id);if(!request)return;
      if(frame.err){proof.errors.push({method:request.method,error:String(frame.err)});return;}
      if(!Object.hasOwn(frame,'ok'))return;
      if(request.method==='CloudBillingSummary')proof.summaries.push(frame.ok.data);
      if(request.method==='CloudBillingUsage')proof.pages.push(frame.ok.data);
      if(request.method==='CloudBillingSetCap')proof.caps.push({request:request.params,result:frame.ok.data});
    }catch{}});}
    send(value){try{const frame=JSON.parse(value);if(frame.method){proof.methods.push(frame.method);pending.set(frame.id,{method:frame.method,params:frame.params});}if(frame.cancel)pending.delete(frame.id);}catch{}super.send(value);}
  };
})();
