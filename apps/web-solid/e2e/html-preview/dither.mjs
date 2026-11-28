import assert from 'node:assert/strict';
import { createServer } from 'vite';
import { openBrowser } from '../harness.mjs';
import { readFileSync, mkdirSync } from 'node:fs';
const server = await createServer({ server: { host: '127.0.0.1', port: 0 } });
await server.listen();
const page = await openBrowser(`http://127.0.0.1:${server.httpServer.address().port}/e2e/html-preview/fixture.html`, { width: 1040, height: 850, args: ['--disable-site-isolation-trials'] });
const contexts = new Map();
page.on('Runtime.executionContextCreated', ({ context }) => { if (context.auxData?.isDefault) contexts.set(context.auxData.frameId, context.id); });
async function inside(expression) {
  const tree = await page.call('Page.getFrameTree');
  const contextId = contexts.get(tree.result.frameTree.childFrames?.[0]?.frame.id);
  if (!contextId) return;
  const r = await page.call('Runtime.evaluate', { expression, contextId, returnByValue: true });
  if (r.result.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails));
  return r.result.result.value;
}
try {
  await page.waitFor('!!window.previewFixture');
  // Exclude the standalone fallback: this must exercise the app-provided raster.
  const html = readFileSync(new URL('../../../../sandbox-guide/dither-charts.html', import.meta.url), 'utf8').replace(/<script>\/\* Generated standalone fallback[\s\S]*?<\/script>/, '');
  await page.eval(`window.previewFixture.theme('dark');window.previewFixture.replace(${JSON.stringify(html)})`);
  for (let i = 0; i < 40 && !await inside("document.querySelector('#total')?.textContent"); i++) await new Promise(r => setTimeout(r, 100));
  assert.equal(await inside("typeof window.zuseCharts.paintAreas"), 'function');
  assert.equal(await inside("document.querySelector('#peak').textContent"), '842');
  const pixels = await inside("(() => {const c=document.querySelector('canvas');return Array.from(c.getContext('2d').getImageData(0,0,c.width,c.height).data).filter((_,i)=>i%4===3).some(a=>a>0);})()");
  assert.equal(pixels, true);
  mkdirSync('/tmp/zuse-html-preview', { recursive: true });
  await page.screenshot('/tmp/zuse-html-preview/settings-dither-area.png');
  await inside("document.querySelector('#bars').click();document.querySelector('#day').value=10;document.querySelector('#day').dispatchEvent(new Event('input'))");
  assert.equal(await inside("document.querySelector('#readout').textContent"), 'Day 11 / 28');
  assert.equal(await inside("document.querySelector('#sessions').textContent"), '390');
  await page.screenshot('/tmp/zuse-html-preview/settings-dither-columns.png');
  await inside("document.querySelector('#area').click()");
  await page.eval("window.previewFixture.theme('light')");
  await new Promise(r => setTimeout(r, 200));
  await page.screenshot('/tmp/zuse-html-preview/settings-dither-light.png');
  await page.call('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await new Promise(r => setTimeout(r, 200));
  assert.equal(await page.eval('document.documentElement.scrollWidth <= innerWidth'), true);
  await page.screenshot('/tmp/zuse-html-preview/settings-dither-mobile.png');
  assert.deepEqual(page.errors, []);
  console.log('Settings-style inline raster verified: shared API, area/columns, inspection, light/dark themes and responsive layout.');
} finally { await page.close(); await server.close(); }
