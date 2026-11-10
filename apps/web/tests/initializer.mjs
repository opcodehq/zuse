import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const source = fs.readFileSync(new URL('../initializer.js', import.meta.url), 'utf8')
  .replace('export default function initializer', 'function initializer');

test('slow compressed downloads report bytes and a failed load can be retried', () => {
  const label = {textContent:''}, buttons = [];
  let reloads = 0, now = 200;
  const context = vm.createContext({performance:{now:()=>now}, location:{reload:()=>reloads++},
    document:{querySelector:selector=>selector==='#boot span'?label:{append:button=>buttons.push(button)},
      createElement:()=>({addEventListener(event, callback) {this[event]=callback;}})}});
  vm.runInContext(source + ';globalThis.hooks=initializer();', context);
  const hooks = context.hooks;
  hooks.onStart(); assert.match(label.textContent, /Downloading/);
  // A compressed response reports a smaller Content-Length than decoded data.
  hooks.onProgress({current:2097152,total:100000});
  assert.match(label.textContent, /2.0 MB/); assert(!label.textContent.includes('%'));
  hooks.onFailure(); assert.match(label.textContent, /Check your connection/);
  assert.equal(buttons[0].textContent, 'Retry'); buttons[0].click(); assert.equal(reloads, 1);
  hooks.onSuccess(); assert.equal(label.textContent, 'Opening workspace…');
});

test('initialization callbacks tolerate a boot screen already removed by GPUI', () => {
  const context = vm.createContext({performance:{now:()=>200},document:{querySelector:()=>null}});
  vm.runInContext(source + ';globalThis.hooks=initializer();', context);
  for(const hook of ['onStart','onSuccess','onFailure']) context.hooks[hook]();
  context.hooks.onProgress({current:1,total:0});
});

test('a rejected stream read exposes one Retry control and retires its listener', () => {
  const listeners = new Map(), buttons = [], label = {textContent:''};
  const context = vm.createContext({performance:{now:()=>200},
    addEventListener:(event,handler)=>listeners.set(event,handler),
    removeEventListener:event=>listeners.delete(event),
    document:{querySelector:selector=>selector==='#boot span'?label:{append:button=>buttons.push(button)},
      createElement:()=>({addEventListener(){}})}});
  vm.runInContext(source + ';globalThis.hooks=initializer();', context);
  context.hooks.onStart();
  assert(listeners.has('unhandledrejection'));
  listeners.get('unhandledrejection')();
  assert.equal(listeners.size, 0);
  context.hooks.onFailure();
  context.hooks.onProgress({current:2097152});
  assert.equal(buttons.length, 1);
  assert.match(label.textContent, /Check your connection/);
});
