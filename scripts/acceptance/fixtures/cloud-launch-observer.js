// Observe only non-secret cloud lifecycle and RPC outcomes from the real UI.
(() => {
  try {localStorage.setItem("zuse.workspace.preferences.v1",JSON.stringify({appearance:"dark"}));} catch {}
  const Native = WebSocket;
  const proof = window.__cloudLaunch = {methods:[],status:[],forwards:[],creates:[],history:[]};
  const pending = new Map();
  window.WebSocket = class extends Native {
    constructor(...args) {
      super(...args);
      this.addEventListener('message', event => {
        try {
          const frame = JSON.parse(event.data), method = pending.get(frame.id);
          if (frame.err) proof.error = String(frame.err);
          const value = frame.ok ?? frame.item;
          if (!value) return;
          if (method === 'CloudContext') proof.projects = value.projects;
          if (method === 'CloudWorkspaces') proof.list = value.workspaces;
          if (method === 'CloudCreate') proof.creates.push(value.workspace.workspace ?? value.workspace);
          if (method === 'CloudEnvironment') proof.status.push(value);
          if (method === 'CloudHistory') proof.history.push({workspaceId:value.head.workspaceId,sessionId:value.head.sessionId,reply:JSON.stringify(value.head.transcript).includes('Brokered native provider reply')});
          if (method === 'CloudRpc') {
            proof.forwards.push({id:value.id,done:value.done,error:value.err,reply:JSON.stringify(value).includes('Brokered native provider reply')});
          }
        } catch {}
      });
    }
    send(value) {
      try {
        const frame = JSON.parse(value);
        if (frame.method) { proof.methods.push(frame.method); pending.set(frame.id,frame.method); }
        if (frame.cancel) pending.delete(frame.id);
      } catch {}
      super.send(value);
    }
  };
})();
