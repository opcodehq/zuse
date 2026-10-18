// Observe real transport metadata without changing requests or collecting tokens.
(()=>{
  const Native=WebSocket;
  const proof=window.__managedAccessProof={methods:[],host:null,hostUpdates:0};
  const watches=new Set();
  window.WebSocket=class extends Native {
    constructor(...args){
      super(...args);
      this.addEventListener('message',event=>{
        try {
          const frame=JSON.parse(event.data);
          if(watches.has(frame.id)&&(frame.ok||frame.item)){
            proof.host=frame.item??frame.ok;proof.hostUpdates++;
          }
        }catch{}
      });
    }
    send(value){
      try {
        const frame=JSON.parse(value);
        if(frame.method)proof.methods.push(frame.method);
        if(frame.method==='WatchManagedHost')watches.add(frame.id);
        if(frame.cancel)watches.delete(frame.id);
      }catch{}
      super.send(value);
    }
  };
})();
