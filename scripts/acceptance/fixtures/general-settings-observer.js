// Persist actual preferences across reloads; intercept external sign-in navigation.
(()=>{
 if(!localStorage.getItem('zuse.workspace.preferences.v1'))localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));
 const proof=window.__generalParity={methods:[],openedUrls:[],preferences:[]};
 window.open=url=>{proof.openedUrls.push(String(url));return null;};
 const Native=WebSocket;
 window.WebSocket=class extends Native{send(value){try{const frame=JSON.parse(value);if(frame.method)proof.methods.push(frame.method);}catch{}super.send(value);}};
 const save=Storage.prototype.setItem;
 Storage.prototype.setItem=function(key,value){if(key==='zuse.workspace.preferences.v1'){try{proof.preferences.push(JSON.parse(value));}catch{}}return save.call(this,key,value);};
})();
