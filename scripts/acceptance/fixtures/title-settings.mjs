// Observe real title preferences and control only their transport receipts.
export function titleSettingsFixture(events) {
  let enabled=false, readError=null, rejectWrite=false, loseReceipt=false, holdWrites=false, saved=null;
  const pending=new Map(),waiting=[];
  return {
    snapshot:()=>({enabled,readError,rejectWrite,loseReceipt,holdWrites,saved,pending:waiting.length}),
    configure(value) {
      enabled=true;
      if(Object.hasOwn(value,'readError'))readError=value.readError;
      if(Object.hasOwn(value,'rejectWrite'))rejectWrite=value.rejectWrite;
      if(Object.hasOwn(value,'loseReceipt'))loseReceipt=value.loseReceipt;
      if(Object.hasOwn(value,'holdWrites'))holdWrites=value.holdWrites;
      if(!holdWrites)for(const [client,frame,send]of waiting.splice(0))send(client,frame);
    },
    admit(client,frame,send) {
      if(!enabled||!['GetTitleSettings','SetTitleSettings'].includes(frame.method))return false;
      events.push({method:frame.method,params:frame.params??{}});
      if(frame.method==='GetTitleSettings'&&readError){send(client,{id:frame.id,err:readError,done:true});return true;}
      if(frame.method==='SetTitleSettings'){
        if(rejectWrite){rejectWrite=false;send(client,{id:frame.id,err:'Naming preference could not be saved.',done:true});return true;}
        let ids=pending.get(client);if(!ids)pending.set(client,ids=new Set());ids.add(frame.id);
      }
      return false;
    },
    receive(client,frame,send) {
      if(!pending.get(client)?.delete(frame.id))return false;
      if(frame.ok)saved=structuredClone(frame.ok);
      if(loseReceipt){loseReceipt=false;send(client,{id:frame.id,err:'The naming receipt was interrupted.',done:true});return true;}
      if(holdWrites)waiting.push([client,frame,send]);else send(client,frame);
      return true;
    },
  };
}
