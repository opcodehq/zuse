// Runs in WebKit's isolated client world; page content cannot call the result handler.
if (globalThis.zuseCancelSelection) globalThis.zuseCancelSelection();
return await new Promise((resolve) => {
  const highlight = document.createElement('div');
  const label = document.createElement('div');
  const hint = document.createElement('div');
  Object.assign(highlight.style, {
    position: 'fixed', pointerEvents: 'none', zIndex: '2147483647',
    border: '1px solid #079c92', background: 'rgba(7,156,146,.1)',
    boxSizing: 'border-box', borderRadius: '2px', display: 'none'
  });
  Object.assign(label.style, {
    position: 'fixed', pointerEvents: 'none', zIndex: '2147483647',
    padding: '3px 6px', borderRadius: '4px', background: '#079c92', color: '#fff',
    font: '600 11px/1.3 -apple-system, BlinkMacSystemFont, sans-serif',
    maxWidth: 'min(280px, 90vw)', overflow: 'hidden', textOverflow: 'ellipsis',
    whiteSpace: 'nowrap', display: 'none', boxSizing: 'border-box'
  });
  Object.assign(hint.style, {
    position: 'fixed', pointerEvents: 'none', zIndex: '2147483647',
    left: '50%', bottom: '12px', transform: 'translateX(-50%)',
    maxWidth: 'calc(100vw - 24px)', padding: '7px 12px', borderRadius: '999px',
    background: '#252529', color: '#fff', boxShadow: '0 2px 10px rgba(0,0,0,.2)',
    font: '500 12px/1.3 -apple-system, BlinkMacSystemFont, sans-serif',
    textAlign: 'center', whiteSpace: 'nowrap', overflow: 'hidden',
    textOverflow: 'ellipsis', boxSizing: 'border-box'
  });
  hint.textContent = 'Click an element to add it to chat · Esc to exit';
  document.documentElement.append(highlight, label, hint);
  let target = null;
  function update() {
    if (!target || !target.isConnected) {
      highlight.style.display = label.style.display = 'none';
      return;
    }
    const rect = target.getBoundingClientRect();
    Object.assign(highlight.style, {display: 'block', left: rect.left + 'px', top: rect.top + 'px', width: rect.width + 'px', height: rect.height + 'px'});
    // Labels identify the element without exposing field values or long generated classes.
    const classes = Array.from(target.classList).filter(c => c.length <= 24).slice(0, 2);
    label.textContent = target.localName + classes.map(c => '.' + c).join('');
    label.style.display = 'block';
    label.style.left = Math.max(4, Math.min(rect.left, innerWidth - label.offsetWidth - 4)) + 'px';
    label.style.top = Math.max(4, rect.top - label.offsetHeight - 3) + 'px';
  }
  function move(event) {
    target = event.target instanceof Element ? event.target : null;
    update();
  }
  function finish(value) {
    document.removeEventListener('pointermove', move, true);
    document.removeEventListener('click', click, true);
    for (const name of ['pointerdown', 'pointerup', 'mousedown', 'mouseup']) document.removeEventListener(name, block, true);
    document.removeEventListener('keydown', key, true);
    window.removeEventListener('scroll', update, true);
    window.removeEventListener('resize', update);
    highlight.remove(); label.remove(); hint.remove();
    delete globalThis.zuseCancelSelection;
    resolve(value);
  }
  function block(event) { event.preventDefault(); event.stopImmediatePropagation(); }
  function click(event) {
    event.preventDefault();
    event.stopImmediatePropagation();
    const el = event.target instanceof Element ? event.target : target;
    if (!el) return finish(null);
    const sensitive = el.closest('input,textarea,select,[contenteditable]');
    const text = sensitive ? '' : (el.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 300);
    const id = el.id ? '#' + CSS.escape(el.id) : '';
    const classes = Array.from(el.classList).slice(0, 3).map(c => '.' + CSS.escape(c)).join('');
    finish(JSON.stringify({tag: el.localName, selector: (el.localName + id + classes).slice(0, 300), text}));
  }
  function key(event) { if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); finish(null); } }
  globalThis.zuseCancelSelection = () => finish(null);
  document.addEventListener('pointermove', move, true);
  document.addEventListener('click', click, true);
  for (const name of ['pointerdown', 'pointerup', 'mousedown', 'mouseup']) document.addEventListener(name, block, true);
  document.addEventListener('keydown', key, true);
  window.addEventListener('scroll', update, true);
  window.addEventListener('resize', update);
});
