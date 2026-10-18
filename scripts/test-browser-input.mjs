#!/usr/bin/env node
// Exercise real GPUI editors through the browser's DOM and trusted input paths.
import assert from 'node:assert/strict';
import {isDeepStrictEqual} from 'node:util';
import {readFile, stat} from 'node:fs/promises';
import {createServer} from 'node:http';
import {resolve, sep, extname} from 'node:path';

const option = name => process.argv[process.argv.indexOf(name) + 1];
if (!process.argv.includes('--assets') || !process.argv.includes('--devtools-file')) {
  console.error('Usage: node scripts/test-browser-input.mjs --assets DIR --devtools-file DevToolsActivePort');
  process.exit(2);
}
const assets = resolve(option('--assets'));
const [port, endpoint] = (await readFile(option('--devtools-file'), 'utf8')).trim().split('\n');
const socket = new WebSocket(`ws://127.0.0.1:${port}${endpoint}`);
await new Promise((done, fail) => { socket.onopen = done; socket.onerror = fail; });
let serial = 0;
const pending = new Map();
socket.onmessage = event => {
  const message = JSON.parse(event.data);
  const request = pending.get(message.id);
  if (!request) return;
  pending.delete(message.id);
  clearTimeout(request.timeout);
  message.error ? request.reject(new Error(JSON.stringify(message.error))) : request.resolve(message.result);
};
function call(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const id = ++serial;
    const timeout = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 10000);
    pending.set(id, {resolve, reject, timeout});
    socket.send(JSON.stringify({id, method, params, ...(sessionId ? {sessionId} : {})}));
  });
}
const server = createServer(async (request, response) => {
  response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
  response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
  try {
    const path = resolve(assets, '.' + decodeURIComponent(new URL(request.url, 'http://localhost').pathname));
    if (path !== assets && !path.startsWith(assets + sep)) throw new Error('Invalid asset path');
    const file = (await stat(path)).isDirectory() ? resolve(path, 'index.html') : path;
    const mime = {'.js': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html'};
    response.setHeader('Content-Type', mime[extname(file)] || 'application/octet-stream');
    response.end(await readFile(file));
  } catch {
    response.writeHead(404).end();
  }
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const origin = `http://127.0.0.1:${server.address().port}`;
let context;
const results = [];
try {
  ({browserContextId: context} = await call('Target.createBrowserContext'));
  await call('Browser.grantPermissions', {browserContextId: context, origin, permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite']});
  const {targetId} = await call('Target.createTarget', {url: origin, browserContextId: context});
  const {sessionId} = await call('Target.attachToTarget', {targetId, flatten: true});
  await call('Emulation.setDeviceMetricsOverride', {width: 900, height: 600, deviceScaleFactor: 1, mobile: false}, sessionId);
  await call('Emulation.setFocusEmulationEnabled', {enabled: true}, sessionId);
  await call('Page.bringToFront', {}, sessionId);
  const evaluate = async expression => {
    const response = await call('Runtime.evaluate', {expression, returnByValue: true, awaitPromise: true}, sessionId);
    if (response.exceptionDetails) throw new Error(JSON.stringify(response.exceptionDetails));
    return response.result.value;
  };
  const pause = ms => new Promise(done => setTimeout(done, ms));
  const state = () => evaluate("JSON.parse(document.body.getAttribute('data-input-fixture') || 'null')");
  async function expect(expected) {
    expected = {submitted: 0, firstError: false, secondError: false, readonly: false, ...expected};
    let actual;
    for (let attempt = 0; attempt < 100; attempt++) {
      actual = await state();
      if (isDeepStrictEqual(actual, expected)) return;
      await pause(50);
    }
    assert.deepEqual(actual, expected);
  }
  async function reset() {
    await evaluate(`(() => {
      if (Object.hasOwn(navigator, 'clipboard')) delete navigator.clipboard;
      if (navigator.clipboard) delete navigator.clipboard.readText;
      window.__clipboardReads=0; window.__clipboardResolvers=[];
      const input=document.querySelector('input'); input.blur();
      const canvas=document.querySelector('canvas');
      for(const type of ['pointerdown','pointerup']) canvas.dispatchEvent(new PointerEvent(type,
        {clientX:100,clientY:335,button:0,buttons:type==='pointerdown'?1:0,pointerId:1,bubbles:true,cancelable:true}));
    })()`);
    await expect({first: '', second: ''});
  }
  // The first GPU/font initialization may take longer than a field update.
  for (let attempt = 0; attempt < 400 && !await state(); attempt++) await pause(50);
  assert(await state(), 'The opt-in browser input fixture did not initialize');
  const focus = (field, suffix = '') => evaluate(`(() => {
    const canvas = document.querySelector('canvas');
    const input = document.querySelector('input');
    const y = ${field === 'first' ? 95 : 185};
    for (const type of ['pointerdown', 'pointerup']) canvas.dispatchEvent(new PointerEvent(type,
      {clientX: 150, clientY: y, button: 0, buttons: type === 'pointerdown' ? 1 : 0, pointerId: 1, bubbles: true, cancelable: true}));
    ${suffix}
  })()`);
  const composition = (type, text) => `input.dispatchEvent(new CompositionEvent(${JSON.stringify(type)}, {data: ${JSON.stringify(text)}}));`;
  const paste = text => `{
    const transfer = new DataTransfer(); transfer.setData('text/plain', ${JSON.stringify(text)});
    input.dispatchEvent(new ClipboardEvent('paste', {clipboardData: transfer, cancelable: true, bubbles: true}));
  }`;
  const emit = code => evaluate(`(() => {const input = document.querySelector('input'); ${code}})()`);
  const key = async (key, code, modifiers = 0) => {
    for (const type of ['keyDown', 'keyUp']) await call('Input.dispatchKeyEvent', {type, key, code, modifiers,
      windowsVirtualKeyCode: key.length === 1 ? key.toUpperCase().charCodeAt(0) : undefined}, sessionId);
  };
  async function test(name, run) {
    try { await reset(); await run(); results.push({name, passed: true}); }
    catch (error) { results.push({name, passed: false, error: error.message}); }
    console.error(`${results.at(-1).passed ? 'PASS' : 'FAIL'} ${name}${results.at(-1).error ? ': ' + results.at(-1).error : ''}`);
  }
  await test('paste immediately after changing fields', async () => {
    await focus('first'); await call('Input.insertText', {text: 'Origin'}, sessionId);
    await expect({first: 'Origin', second: ''});
    await focus('second', paste('日本語 🦀 paste'));
    await expect({first: 'Origin', second: '日本語 🦀 paste'});
  });
  await test('composition immediately after changing fields', async () => {
    await focus('first'); await call('Input.insertText', {text: 'Origin'}, sessionId);
    await expect({first: 'Origin', second: ''});
    await focus('second', composition('compositionstart', '') + composition('compositionupdate', 'か') + composition('compositionupdate', '漢字🦀') + composition('compositionend', '漢字🦀'));
    await expect({first: 'Origin', second: '漢字🦀'});
  });
  await test('cancel composition without keeping preedit', async () => {
    await focus('first', composition('compositionstart', '') + composition('compositionupdate', 'cancel') + composition('compositionend', ''));
    await expect({first: '', second: ''});
    await call('Input.insertText', {text: 'after cancel'}, sessionId);
    await expect({first: 'after cancel', second: ''});
  });
  await test('late composition stays out of the next field', async () => {
    await focus('first', composition('compositionstart', '') + composition('compositionupdate', 'か'));
    await expect({first: 'か', second: ''});
    await focus('second'); await pause(150);
    await emit(composition('compositionupdate', 'late') + composition('compositionend', '漢字'));
    await expect({first: 'か', second: ''});
    await focus('first'); await key('End', 'End'); await call('Input.insertText', {text: '+'}, sessionId);
    await expect({first: 'か+', second: ''});
  });
  await test('DOM blur abandons the old composition', async () => {
    await focus('first', composition('compositionstart', '') + composition('compositionupdate', 'か'));
    await expect({first: 'か', second: ''});
    await emit("input.blur(); input.focus();" + composition('compositionend', 'late'));
    await expect({first: 'か', second: ''});
    await call('Input.insertText', {text: '+'}, sessionId);
    await expect({first: 'か+', second: ''});
  });
  await test('trusted browser paste replaces selection once', async () => {
    await focus('first'); await call('Input.insertText', {text: 'replace me'}, sessionId);
    await expect({first: 'replace me', second: ''});
    await evaluate("navigator.clipboard.writeText('trusted 🦀 paste')");
    await key('a', 'KeyA', 2); await key('v', 'KeyV', 2);
    await expect({first: 'trusted 🦀 paste', second: ''});
  });
  await test('IME Enter confirms without submitting the composer', async () => {
    await focus('first', composition('compositionstart', '') + composition('compositionupdate', 'か'));
    await expect({first: 'か', second: ''});
    await emit("input.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', code:'Enter', isComposing:true, bubbles:true, cancelable:true}));"
      + composition('compositionend', '漢字'));
    await expect({first: '漢字', second: ''});
    await key('Enter', 'Enter');
    await expect({first: '漢字', second: '', submitted: 1});
  });
  await test('window blur fences late composition events', async () => {
    await focus('first', composition('compositionstart', '') + composition('compositionupdate', 'か'));
    await expect({first: 'か', second: ''});
    await evaluate(`(() => { const c=document.querySelector('canvas');for(const type of ['pointerdown','pointerup'])
      c.dispatchEvent(new PointerEvent(type,{clientX:100,clientY:275,button:0,buttons:type==='pointerdown'?1:0,pointerId:1,bubbles:true,cancelable:true})); })()`);
    await emit(composition('compositionupdate', 'late') + composition('compositionend', 'late'));
    await focus('first'); await call('Input.insertText', {text: '+'}, sessionId);
    await expect({first: 'か+', second: ''});
  });
  await test('paste ends composition without a late duplicate', async () => {
    await focus('first', composition('compositionstart', '') + composition('compositionupdate', 'draft'));
    await expect({first: 'draft', second: ''});
    await emit(paste('pasted') + composition('compositionend', 'draft'));
    await expect({first: 'pasted', second: ''});
  });
  await test('trusted IME updates and commits Unicode once', async () => {
    await focus('first');
    await call('Input.imeSetComposition', {text: 'か', selectionStart: 1, selectionEnd: 1}, sessionId);
    await expect({first: 'か', second: ''});
    await call('Input.imeSetComposition', {text: '漢字🦀', selectionStart: 4, selectionEnd: 4}, sessionId);
    await expect({first: '漢字🦀', second: ''});
    await call('Input.insertText', {text: '漢字🦀'}, sessionId);
    await expect({first: '漢字🦀', second: ''});
    await call('Input.insertText', {text: '+'}, sessionId);
    await expect({first: '漢字🦀+', second: ''});
  });
  const pasteAction = () => evaluate(`(() => {
    const canvas=document.querySelector('canvas');
    for(const type of ['pointerdown','pointerup']) canvas.dispatchEvent(new PointerEvent(type,
      {clientX:100,clientY:395,button:0,buttons:type==='pointerdown'?1:0,pointerId:1,bubbles:true,cancelable:true}));
  })()`);
  await test('explicit Paste action reads browser clipboard', async () => {
    await focus('first'); await call('Input.insertText', {text:'replace me'}, sessionId);
    await expect({first:'replace me',second:''});
    await key('a','KeyA',2);
    await evaluate("navigator.clipboard.writeText('menu 日本語 🦀')");
    await pasteAction();
    await expect({first:'menu 日本語 🦀',second:''});
    await key('z','KeyZ',2);
    await expect({first:'replace me',second:''});
    await key('y','KeyY',2);
    await expect({first:'menu 日本語 🦀',second:''});
  });
  const until = async expression => {
    for(let i=0;i<100;i++) { if(await evaluate(expression)) return; await pause(25); }
    assert(await evaluate(expression), expression);
  };
  const drain = () => evaluate('new Promise(done=>requestAnimationFrame(()=>requestAnimationFrame(done)))');
  const mockClipboard = (expression = 'new Promise(resolve=>window.__clipboardResolvers.push(resolve))') => evaluate(`
    Object.defineProperty(navigator.clipboard,'readText',{configurable:true,value:()=>{
      window.__clipboardReads++; return ${expression};
    }});`);
  async function pendingPaste() {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''});
    await mockClipboard(); await pasteAction(); await until('window.__clipboardReads===1');
  }
  async function releasePaste(text = 'late', index = 0) {
    await evaluate(`window.__clipboardResolvers[${index}](${JSON.stringify(text)})`);
    await drain();
  }
  const readonlyButton = () => evaluate(`(() => {const canvas=document.querySelector('canvas');
    for(const type of ['pointerdown','pointerup']) canvas.dispatchEvent(new PointerEvent(type,
      {clientX:100,clientY:455,button:0,buttons:type==='pointerdown'?1:0,pointerId:1,bubbles:true,cancelable:true}));
  })()`);
  await test('late Paste cannot follow focus away and back', async () => {
    await pendingPaste(); await focus('second'); await focus('first'); await releasePaste();
    await expect({first:'keep',second:''});
  });
  await test('late Paste cannot follow an edit and undo', async () => {
    await pendingPaste(); await call('Input.insertText',{text:'+'},sessionId);
    await expect({first:'keep+',second:''}); await key('z','KeyZ',2);
    await expect({first:'keep',second:''}); await releasePaste();
    await expect({first:'keep',second:''});
  });
  await test('late Paste cannot follow a caret round trip', async () => {
    await pendingPaste(); await key('Home','Home'); await key('End','End'); await releasePaste();
    await expect({first:'keep',second:''});
  });
  await test('newer Paste wins when reads resolve out of order', async () => {
    await pendingPaste(); await pasteAction(); await until('window.__clipboardReads===2');
    await releasePaste('second',1); await expect({first:'keepsecond',second:''});
    await releasePaste('first',0); await expect({first:'keepsecond',second:''});
  });
  await test('denied clipboard read preserves text and allows keyboard fallback', async () => {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''});
    await mockClipboard('Promise.reject(new DOMException("fixture denied","NotAllowedError"))');
    await pasteAction(); await expect({first:'keep',second:'',firstError:true});
    await evaluate("navigator.clipboard.writeText('fallback')"); await key('v','KeyV',2);
    await expect({first:'keepfallback',second:''});
    assert.equal(await evaluate('window.__clipboardReads'),1);
  });
  await test('synchronous clipboard errors stay inside the editor', async () => {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''});
    await mockClipboard('(()=>{throw new Error("fixture failure")})()');
    await pasteAction(); await expect({first:'keep',second:'',firstError:true});
  });
  await test('missing clipboard API reports the keyboard fallback', async () => {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''});
    await evaluate("Object.defineProperty(navigator,'clipboard',{configurable:true,value:undefined})");
    await pasteAction(); await expect({first:'keep',second:'',firstError:true});
  });
  await test('empty clipboard does not delete the selection', async () => {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''}); await key('a','KeyA',2);
    await mockClipboard('Promise.resolve("")'); await pasteAction();
    await until('window.__clipboardReads===1'); await drain();
    await expect({first:'keep',second:''});
  });
  await test('read-only field does not request clipboard access', async () => {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''}); await mockClipboard('Promise.resolve("changed")');
    await readonlyButton(); await expect({first:'keep',second:'',readonly:true});
    await pasteAction(); await drain();
    await expect({first:'keep',second:'',readonly:true});
    assert.equal(await evaluate('window.__clipboardReads'),0);
  });
  await test('read-only round trip cancels a pending Paste', async () => {
    await pendingPaste(); await readonlyButton(); await readonlyButton(); await releasePaste();
    await expect({first:'keep',second:''});
  });
  await test('window activation round trip cancels a pending Paste', async () => {
    await pendingPaste(); await emit('input.blur();input.focus();'); await releasePaste();
    await expect({first:'keep',second:''});
  });
  const pointer = (x,y,button=0) => evaluate(`(() => {const canvas=document.querySelector('canvas');
    for(const type of ['pointerdown','pointerup']) canvas.dispatchEvent(new PointerEvent(type,
      {clientX:${x},clientY:${y},button:${button},buttons:type==='pointerdown'?${button===2?2:1}:0,pointerId:1,bubbles:true,cancelable:true}));
  })()`);
  await test('right-click menu Paste replaces the actual selection', async () => {
    await focus('first'); await call('Input.insertText',{text:'replace me'},sessionId);
    await expect({first:'replace me',second:''}); await key('a','KeyA',2);
    await mockClipboard('Promise.resolve("context paste")');
    await pointer(75,95,2); await drain(); await pointer(150,170);
    await expect({first:'context paste',second:''});
    await call('Input.insertText',{text:'+'},sessionId);
    await expect({first:'context paste+',second:''});
  });
  await test('context menu keyboard selection does not submit', async () => {
    await focus('first'); await call('Input.insertText',{text:'replace me'},sessionId);
    await expect({first:'replace me',second:''}); await key('a','KeyA',2);
    await mockClipboard('Promise.resolve("keyboard menu")');
    await pointer(75,95,2); await drain();
    await key('ArrowDown','ArrowDown'); await key('ArrowDown','ArrowDown'); await key('Enter','Enter');
    await expect({first:'keyboard menu',second:''});
  });
  await test('paste shortcut works while the context menu owns focus', async () => {
    await focus('first'); await call('Input.insertText',{text:'replace me'},sessionId);
    await expect({first:'replace me',second:''}); await key('a','KeyA',2);
    await evaluate("navigator.clipboard.writeText('menu shortcut')");
    await pointer(75,95,2); await drain(); await key('v','KeyV',2);
    await expect({first:'menu shortcut',second:''});
  });
  await test('context menu Copy and Cut preserve undo', async () => {
    await focus('first'); await call('Input.insertText',{text:'replace me'},sessionId);
    await expect({first:'replace me',second:''}); await key('a','KeyA',2);
    await evaluate("navigator.clipboard.writeText('sentinel')");
    await pointer(75,95,2); await drain(); await key('ArrowDown','ArrowDown'); await key('Enter','Enter');
    await until('(async()=>await navigator.clipboard.readText()==="replace me")()');
    await expect({first:'replace me',second:''});
    await pointer(75,95,2); await drain(); await pointer(150,114);
    await expect({first:'',second:''});
    await until('(async()=>await navigator.clipboard.readText()==="replace me")()');
    await key('z','KeyZ',2); await expect({first:'replace me',second:''});
  });
  await test('context menu Enter activates the hovered item', async () => {
    await focus('first'); await call('Input.insertText',{text:'replace me'},sessionId);
    await expect({first:'replace me',second:''}); await key('a','KeyA',2);
    await mockClipboard('Promise.resolve("hover paste")');
    await pointer(75,95,2); await drain();
    await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:150,y:170},sessionId);
    await drain(); await key('Enter','Enter');
    await expect({first:'hover paste',second:''});
  });
  await test('read-only context menu rejects Paste without clipboard access', async () => {
    await focus('first'); await call('Input.insertText',{text:'keep'},sessionId);
    await expect({first:'keep',second:''}); await readonlyButton();
    await expect({first:'keep',second:'',readonly:true});
    await mockClipboard('Promise.resolve("changed")');
    await pointer(60,95,2); await drain(); await pointer(150,170); await key('Escape','Escape');
    await expect({first:'keep',second:'',readonly:true});
    assert.equal(await evaluate('window.__clipboardReads'),0);
  });
  await test('WASM Meta selection and undo commands reach the editor', async () => {
    await focus('first'); await call('Input.insertText',{text:'replace me'},sessionId);
    await expect({first:'replace me',second:''}); await key('a','KeyA',4);
    await mockClipboard('Promise.resolve("Meta paste")'); await pasteAction();
    await expect({first:'Meta paste',second:''}); await key('z','KeyZ',4);
    await expect({first:'replace me',second:''}); await key('z','KeyZ',12);
    await expect({first:'Meta paste',second:''});
  });
  console.log(JSON.stringify(results, null, 2));
  assert(results.every(result => result.passed), 'Browser input conformance failed');
} finally {
  if (context) await call('Target.disposeBrowserContext', {browserContextId: context}).catch(() => {});
  for (const request of pending.values()) clearTimeout(request.timeout);
  socket.close();
  await new Promise(done => server.close(done));
}
