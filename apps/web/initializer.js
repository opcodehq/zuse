// Trunk owns the fetch/instantiate sequence. Report bytes rather than a
// percentage: Content-Length can describe compressed bytes while fetch emits
// decompressed bytes, and some proxies omit it altogether.
export default function initializer() {
  const label = () => document.querySelector('#boot span');
  let lastUpdate = 0;
  let failed = false;
  const failure = () => {
    globalThis.removeEventListener?.('unhandledrejection', failure);
    const boot = document.querySelector('#boot');
    const status = label();
    if (failed || !boot || !status) return;
    failed = true;
    status.textContent = 'Could not load Zuse. Check your connection and retry.';
    const retry = document.createElement('button');
    retry.textContent = 'Retry';
    retry.addEventListener('click', () => location.reload());
    boot.append(retry);
  };
  return {
    onStart() {
      // Trunk 0.21's streaming shim leaves reader.read() rejections unhandled
      // if a connection drops mid-download. Keep its pending init recoverable.
      globalThis.addEventListener?.('unhandledrejection', failure);
      const status = label();
      if (status) status.textContent = 'Downloading Zuse…';
    },
    onProgress({current}) {
      if (failed) return;
      if (performance.now() - lastUpdate < 150) return;
      lastUpdate = performance.now();
      const status = label();
      if (status) status.textContent = `Downloading Zuse… ${(current / 1048576).toFixed(1)} MB received`;
    },
    onSuccess() {
      globalThis.removeEventListener?.('unhandledrejection', failure);
      const status = label();
      if (status) status.textContent = 'Opening workspace…';
    },
    onFailure: failure,
  };
}
