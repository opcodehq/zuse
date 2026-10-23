// Observe only non-secret cloud lifecycle and RPC outcomes from the real UI.
(() => {
  try {localStorage.setItem("zuse.workspace.preferences.v1",JSON.stringify({appearance:"dark"}));} catch {}
  const Native = WebSocket;
  const proof = window.__cloudLaunch = {methods:[],status:[],forwards:[],creates:[],history:[],uploads:[],imageReads:[],innerMethods:[],sockets:0};
  const pending = new Map();
  window.WebSocket = class extends Native {
    constructor(...args) {
      super(...args);proof.sockets+=1;
      this.addEventListener('message', event => {
        try {
          const frame = JSON.parse(event.data), request = pending.get(frame.id), method = request?.method;
          if (frame.err) proof.error = String(frame.err);
          const value = frame.ok ?? frame.item;
          if (!value) return;
          if (method === 'CloudContext') proof.projects = value.projects;
          if (method === 'CloudWorkspaces') proof.list = value.workspaces;
          if (method === 'CloudCreate') proof.creates.push(value.workspace.workspace ?? value.workspace);
          if (method === 'CloudEnvironment') proof.status.push(value);
          if (method === 'CloudHistory') proof.history.push({workspaceId:value.head.workspaceId,sessionId:value.head.sessionId,reply:JSON.stringify(value.head.transcript).includes('Brokered native provider reply')});
          if (method === 'CloudRpc') {
            proof.forwards.push({id:value.id,done:value.done,error:value.err,reply:JSON.stringify(value).includes('Brokered native provider reply'),attachmentReply:JSON.stringify(value).includes('Cloud attachment verified')});
            if(request.inner==='ReadAttachmentChunk' && value.ok?.data)proof.imageReads.push({done:value.ok.done,bytes:value.ok.data.length});
            if(request.inner==='UploadCommit' && value.ok?.path)proof.uploads.push(value.ok.path);
          }
        } catch {}
      });
    }
    send(value) {
      try {
        const frame = JSON.parse(value);
        if (frame.method) { if(frame.method==='CloudRpc')proof.innerMethods.push(frame.params?.method);proof.methods.push(frame.method); pending.set(frame.id,{method:frame.method,inner:frame.params?.method}); }
        if (frame.cancel) pending.delete(frame.id);
      } catch {}
      super.send(value);
    }
  };
})();
