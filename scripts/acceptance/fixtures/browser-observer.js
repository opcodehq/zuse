(()=>{const Native=WebSocket;const proof=window.__browserProof={methods:[]};window.WebSocket=class extends Native{send(value){try{const frame=JSON.parse(value);if(frame.method)proof.methods.push(frame.method);}catch{}super.send(value);}};})();

window.__browserProof.previewRevision=0;
window.addEventListener('message',event=>{
  const frame=document.querySelector('#zuse-browser-frame');
  if(frame&&event.source===frame.contentWindow&&event.origin===new URL(frame.src).origin&&event.data?.previewProof){
    window.__browserProof.preview=event.data.previewProof;
    window.__browserProof.previewRevision++;
  }
});
window.__browserProof.inspect=()=>{
  const frame=document.querySelector('#zuse-browser-frame');
  frame.contentWindow.postMessage('preview-inspect',new URL(frame.src).origin);
  return window.__browserProof.previewRevision;
};
