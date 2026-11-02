// Persist actual preferences across reloads; intercept external sign-in navigation.
(()=>{
 if(!localStorage.getItem('zuse.workspace.preferences.v1'))localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));
 const proof=window.__generalParity={methods:[],openedUrls:[],preferences:[],accountStates:[]};
 window.open=url=>{proof.openedUrls.push(String(url));return null;};
 const Native=WebSocket;
 window.WebSocket=class extends Native{
  constructor(...args){super(...args);this.accountIds=new Set();this.addEventListener('message',event=>{try{const frame=JSON.parse(event.data);if(this.accountIds.has(frame.id))proof.accountStates.push({item:!!frame.item,ok:!!frame.ok,error:!!frame.err,user:!!(frame.item??frame.ok)?.user});}catch{}});}
  send(value){try{const frame=JSON.parse(value);if(frame.method)proof.methods.push(frame.method);if(frame.method==='CloudAccountStatus')this.accountIds.add(frame.id);}catch{}super.send(value);}
 };
 const save=Storage.prototype.setItem;
 Storage.prototype.setItem=function(key,value){if(key==='zuse.workspace.preferences.v1'){try{proof.preferences.push(JSON.parse(value));}catch{}}return save.call(this,key,value);};
})();
