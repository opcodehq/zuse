// Record image control-plane contracts only; never retain bearer tokens or unrelated payloads.
(()=>{
  try{localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));}catch{}
  const Native=WebSocket,pending=new Map();
  const proof=window.__imageProof={methods:[],statuses:[],builds:[],deleted:[],errors:[]};
  window.WebSocket=class extends Native{
    constructor(...args){super(...args);this.addEventListener('message',event=>{try{
      const frame=JSON.parse(event.data),request=pending.get(frame.id);if(!request)return;
      if(frame.err){proof.errors.push({method:request.method,error:String(frame.err)});pending.delete(frame.id);return;}
      if(!Object.hasOwn(frame,'ok'))return;pending.delete(frame.id);
      const data=frame.ok.data;if(!data)return;
      const status={storage:data.storage?{snapshotId:data.storage.snapshotId,state:data.storage.state}:null,state:data.state,generation:data.generation,phase:data.progressPhase,builds:data.builds.map(b=>({id:b.buildId,state:b.state,active:b.active,mode:b.mode}))};
      proof.statuses.push(status);
      if(request.method==='CloudImageBuild')proof.builds.push({request:request.params,result:status});
      if(request.method==='CloudImageDelete')proof.deleted.push({request:request.params,result:status});
    }catch{}});}
    send(value){try{const frame=JSON.parse(value);if(frame.method){proof.methods.push(frame.method);if(['CloudImage','CloudImageBuild','CloudImageDelete'].includes(frame.method))pending.set(frame.id,{method:frame.method,params:frame.params});}if(frame.cancel)pending.delete(frame.id);}catch{}super.send(value);}
  };
})();
