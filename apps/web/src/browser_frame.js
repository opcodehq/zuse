// The only DOM surface in the GPUI browser pane. Its storage and credentials
// are isolated from the application; all lifetime/layout commands are scoped.
let report;
let generation = 0;
let frame;
let visible = false;
let bounds;
let local = false;
function emit(kind, value = {}) {
  report?.(JSON.stringify({browserFrame: {generation, kind, ...value}}));
}
function dispose() {
  if (frame) { frame.onload = null; frame.onerror = null; frame.remove(); frame = undefined; }
}
function layout() {
  if (!frame) return;
  const valid = bounds?.length === 4 && bounds.every(Number.isFinite) && bounds[2] > 0 && bounds[3] > 0;
  frame.style.display = visible && valid ? 'block' : 'none';
  if (valid) Object.assign(frame.style, {left: `${bounds[0]}px`, top: `${bounds[1]}px`, width: `${bounds[2]}px`, height: `${bounds[3]}px`});
}
export function installBrowserFrame(callback, isLocal) {
  report = callback;
  local = isLocal;
  window.addEventListener('pagehide', dispose);
  emit('capability', {supported: 'credentialless' in HTMLIFrameElement.prototype});
}
export function browserFrameCommand(serialized) {
  const command = JSON.parse(serialized);
  if (!Number.isSafeInteger(command.generation) || command.generation < generation) return;
  if (command.generation > generation) {
    dispose(); generation = command.generation; bounds = undefined; visible = false;
  }
  switch (command.action) {
    case 'close': dispose(); return;
    case 'visible': visible = command.visible === true; layout(); return;
    case 'bounds': bounds = command.bounds; layout(); return;
    case 'reload': if (frame) frame.src = frame.src; return;
    case 'navigate': break;
    default: return;
  }
  let external;
  try {
    const text = String(command.url || '').trim();
    if (!text || text.length > 8192) throw new Error('Enter a web address.');
    if (/^(javascript|data|file|blob|about|vbscript):/i.test(text)) throw new Error('Use an HTTP or HTTPS address.');
    const loopback = /^(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?(?:[/?#]|$)/i.test(text);
    const url = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(text) ? text : `${loopback ? 'http' : 'https'}://${text}`);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('Use an HTTP or HTTPS address without embedded credentials.');
    if (url.origin === window.location.origin) throw new Error('Open a project or website address, rather than the Zuse application.');
    if (command.preview === true && !local) throw new Error('This preview uses the execution computer’s local proxy. Enter its published URL to open it from this browser.');
    external = url.href;
    if (!('credentialless' in HTMLIFrameElement.prototype)) throw new Error('This browser cannot isolate an embedded page. Open the address in a new tab.');
    dispose();
    const current = document.createElement('iframe');
    current.id = 'zuse-browser-frame';
    current.title = 'Web preview';
    current.credentialless = true;
    current.referrerPolicy = 'no-referrer';
    current.setAttribute('sandbox', 'allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads');
    current.setAttribute('allow', 'camera \'none\'; microphone \'none\'; geolocation \'none\'');
    Object.assign(current.style, {position: 'fixed', border: '0', zIndex: '10', background: '#fff', display: 'none'});
    frame = current;
    const scope = generation;
    current.onload = () => { if (frame === current && generation === scope) emit('loaded'); };
    current.onerror = () => { if (frame === current && generation === scope) emit('error', {message: 'The page could not load. Try opening it in a new tab.'}); };
    current.src = url.href;
    document.body.appendChild(current);
    layout();
    emit('navigated', {url: url.href});
  } catch (error) {
    emit('error', {message: error.message, url: external});
  }
}
