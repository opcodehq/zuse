// Observe public RPC outcomes from the compiled canvas, without changing app state.
(() => {
 localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));
 const Native=WebSocket,pending=new Map();
 const proof=window.__cloudModels={methods:[],context:[],creates:[],status:[],history:[],replies:[],errors:[]};
 window.WebSocket=class extends Native{
  constructor(...args){super(...args);this.addEventListener('message',event=>{try{
   const frame=JSON.parse(event.data),method=pending.get(frame.id),value=frame.ok??frame.item;
   if(frame.err)proof.errors.push({method,error:String(frame.err)});
   if(!value)return;
   if(method==='CloudContext')proof.context.push(value);
   if(method==='CloudCreate')proof.creates.push(value.workspace.workspace??value.workspace);
   if(method==='CloudEnvironment')proof.status.push(value);
   if(method==='CloudHistory')proof.history.push(value);
   if(/Brokered native provider reply|Cloud issue context verified/.test(JSON.stringify(value)))proof.replies.push(method);
  }catch{}});}
  send(text){try{const frame=JSON.parse(text);if(frame.method){pending.set(frame.id,frame.method);proof.methods.push(frame.method);}}catch{}super.send(text);}
 };
})();
