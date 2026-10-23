// Observe actual host RPC metadata; no credentials or encrypted envelopes are collected.
(() => {
  const Native = WebSocket;
  const proof = window.__cloudArchiveProof = {methods: [], list: null, history: null, unarchive: null, deletions: [], cleanups: []};
  const pending = new Map();
  window.WebSocket = class extends Native {
    constructor(...args) {
      super(...args);
      this.addEventListener('message', event => {
        try {
          const frame = JSON.parse(event.data);
          const method = pending.get(frame.id);
          if (frame.err) proof.error = String(frame.err);
          if (!frame.ok) return;
          if (method === 'CloudWorkspaces') proof.list = frame.ok.workspaces;
          if (method === 'CloudHistory') {
            const head = frame.ok.head;
            proof.history = {workspaceId: head.workspaceId, sessionId: head.sessionId,
              archived: head.chat.archived, paused: head.queuePaused,
              entries: head.transcript.reset?.length ?? 0,
              fixtureReply: JSON.stringify(head.transcript).includes('Cloud runtime fixture reply')};
          }
          if (method === 'CloudUnarchive') proof.unarchive = frame.ok.workspace;
          if (method === 'CloudDelete') proof.deletions.push(frame.ok.workspace);
          if (method === 'CloudCleanupMirror') proof.cleanups.push(frame.ok.workspace);
        } catch {}
      });
    }
    send(value) {
      try {
        const frame = JSON.parse(value);
        if (frame.method) { proof.methods.push(frame.method); pending.set(frame.id, frame.method); }
        if (frame.cancel) pending.delete(frame.id);
      } catch {}
      super.send(value);
    }
  };
})();
