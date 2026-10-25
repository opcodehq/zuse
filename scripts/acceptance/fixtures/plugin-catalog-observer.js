// Observe interaction contracts without retaining account or authentication data.
(() => {
  localStorage.setItem('zuse.workspace.preferences.v1', JSON.stringify({appearance: 'dark'}));
  const Native = WebSocket;
  const proof = window.__uiParity = {methods: [], mutations: []};
  window.WebSocket = class extends Native {
    send(value) {
      try {
        const frame = JSON.parse(value);
        if (frame.method) proof.methods.push(frame.method);
        if (frame.method === 'Mutate') proof.mutations.push({op: frame.params.op, chatId: frame.params.chatId, archived: frame.params.archived});
      } catch {}
      super.send(value);
    }
  };
})();

(() => {
 const write = navigator.clipboard.writeText.bind(navigator.clipboard);
 navigator.clipboard.writeText = value => {window.__pluginCopied=value;return write(value);};
})();
