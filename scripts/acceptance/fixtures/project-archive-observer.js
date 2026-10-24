(()=>{
 localStorage.setItem('zuse.workspace.preferences.v1',JSON.stringify({appearance:'dark',sidebarView:'projects'}));
 const Native=WebSocket,proof=window.__uiParity={methods:[],mutations:[]};
 window.WebSocket=class extends Native{send(raw){try{const f=JSON.parse(raw);if(f.method)proof.methods.push(f.method);if(f.method==='Mutate')proof.mutations.push(f.params);}catch{}super.send(raw);}};
})();
