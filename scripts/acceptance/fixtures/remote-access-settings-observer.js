// Observe the public RPC method names and native clipboard output without altering replies.
(()=>{
 if(!localStorage.getItem('zuse.workspace.preferences.v1'))localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark'}));
 const proof=window.__remoteAccess={methods:[],copied:null};
 const copy=navigator.clipboard.writeText.bind(navigator.clipboard);
 navigator.clipboard.writeText=async value=>{proof.copied=String(value);return copy(value);};
 const Native=WebSocket;
 window.WebSocket=class extends Native{send(value){try{const f=JSON.parse(value);if(f.method)proof.methods.push(f.method);}catch{}super.send(value);}};
})();
