// Observe only non-secret cloud lifecycle and RPC outcomes from the real UI.
(() => {
  try {localStorage.setItem("zuse.workspace.preferences.v1",JSON.stringify({appearance:"dark"}));} catch {}
  const Native = WebSocket;
  const proof = window.__cloudLaunch = {methods:[],status:[],forwards:[],creates:[],history:[],uploads:[],imageReads:[],innerMethods:[],staged:[],pauses:[],metadata:[],sync:[],tools:[],sockets:0};
  const pending = new Map();
  const fetchOriginal = window.fetch.bind(window);
  window.fetch = async (...args) => {
    const response = await fetchOriginal(...args);
    if(String(args[0]?.url ?? args[0]).includes('/api/attachments')) {
      try {const value=await response.clone().json();if(value.path)proof.staged.push(value.path);}catch{}
    }
    return response;
  };

  window.WebSocket = class extends Native {
    constructor(...args) {
      super(...args);proof.sockets+=1;
      this.addEventListener('message', event => {
        try {
          const frame = JSON.parse(event.data), request = pending.get(frame.id), method = request?.method;
          if (frame.err) proof.error = String(frame.err);
          const value = frame.ok ?? frame.item;
          if (!value) return;
          if (method === 'UploadCommit' && value.path) proof.staged.push(value.path);
          if (method?.startsWith('CloudSync')) proof.sync.push({method,...value});
          if (method === 'CloudContext') proof.projects = value.projects;
          if (method === 'CloudWorkspaces') proof.list = value.workspaces;
          if (method === 'CloudCreate') proof.creates.push(value.workspace.workspace ?? value.workspace);
          if (method === 'CloudEnvironment') proof.status.push(value);
          if (method === 'CloudPause') proof.pauses.push(value.workspace);
          if (method === 'CloudWorkspace') proof.metadata.push(value.workspace);
          if (method === 'CloudHistory') proof.history.push({workspaceId:value.head.workspaceId,sessionId:value.head.sessionId,reply:JSON.stringify(value.head.transcript).includes('Brokered native provider reply') || JSON.stringify(value.head?.transcript ?? value).includes('Cloud attachment verified')});
          if (method === 'CloudRpc' || method === 'CloudLocalTerminal') {
            if(['ListTerminals','OpenTerminal','WriteTerminal','CloseTerminal','ReadWorkspaceFile','WriteWorkspaceFile','ListWorkspaceDirectory','GetCheckoutDiff','GetGitWorkspace'].includes(request.inner) && Object.hasOwn(value,'ok')) proof.tools.push({environment:method==='CloudLocalTerminal'?'local':'cloud',method:request.inner,params:request.params,result:value.ok});
            if(request.inner==='SubscribeTerminal' && value.item?.type==='data') proof.tools.push({environment:method==='CloudLocalTerminal'?'local':'cloud',method:'TerminalData',data:value.item.data});
            proof.forwards.push({id:value.id,done:value.done,error:value.err,reply:JSON.stringify(value).includes('Brokered native provider reply') || JSON.stringify(value.head?.transcript ?? value).includes('Cloud attachment verified'),attachmentReply:JSON.stringify(value).includes('Cloud attachment verified')});
            if(request.inner==='ReadAttachmentChunk' && value.ok?.data)proof.imageReads.push({done:value.ok.done,bytes:value.ok.data.length});
            if(request.inner==='UploadCommit' && value.ok?.path)proof.uploads.push(value.ok.path);
          }
        } catch {}
      });
    }
    send(value) {
      try {
        const frame = JSON.parse(value);
        if (frame.method) { if(frame.method==='CloudRpc'||frame.method==='CloudLocalTerminal')proof.innerMethods.push(frame.params?.method);proof.methods.push(frame.method); pending.set(frame.id,{method:frame.method,inner:frame.params?.method,params:frame.params?.params}); }
        if (frame.cancel) pending.delete(frame.id);
      } catch {}
      super.send(value);
    }
  };
})();
