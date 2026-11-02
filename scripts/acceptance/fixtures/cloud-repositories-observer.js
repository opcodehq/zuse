// Observe real repository requests/results; credentials and unrelated RPC payloads stay private.
(()=>{
  try{localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));}catch{}
  const Native=WebSocket,pending=new Map();
  const proof=window.__repositoryProof={methods:[],lists:[],github:[],connected:[],removed:[],errors:[]};
  window.WebSocket=class extends Native{
    constructor(...args){super(...args);this.addEventListener('message',event=>{try{
      const frame=JSON.parse(event.data),request=pending.get(frame.id);if(!request)return;
      if(frame.err){proof.errors.push({method:request.method,error:String(frame.err)});pending.delete(frame.id);return;}
      if(!Object.hasOwn(frame,'ok'))return;pending.delete(frame.id);
      const data=frame.ok.data;
      if(request.method==='CloudProjects')proof.lists.push(data);
      if(request.method==='CloudGithubCatalog')proof.github.push({configured:data.configured,repositories:data.repositories.map(r=>r.nameWithOwner)});
      if(request.method==='CloudProjectConnect')proof.connected.push({request:request.params,result:data});
      if(request.method==='CloudProjectRemove')proof.removed.push({request:request.params,result:data});
    }catch{}});}
    send(value){try{const frame=JSON.parse(value);if(frame.method){proof.methods.push(frame.method);if(['CloudProjects','CloudGithubCatalog','CloudProjectConnect','CloudProjectRemove'].includes(frame.method))pending.set(frame.id,{method:frame.method,params:frame.params});}if(frame.cancel)pending.delete(frame.id);}catch{}super.send(value);}
  };
})();
