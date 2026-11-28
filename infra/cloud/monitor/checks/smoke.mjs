// Run beside the guest with Playwright installed. Only loopback viewers are opened.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { randomBytes } from 'node:crypto';

const require = createRequire(resolve(process.env.PLAYWRIGHT_NODE_MODULES || 'node_modules', '../monitor.cjs'));
const { chromium } = require('playwright');
const [width, height] = (process.env.POOF_DISPLAY_SIZE || '1920x1080').split('x').map(Number);
const tool = (...args) => execFileSync('zuse-monitor', args, { encoding: 'utf8' }).trim();
const options = { executablePath: process.env.CHROMIUM || '/usr/bin/chromium' };
const guest = await chromium.launch({ ...options, headless: false, args: ['--kiosk'] });
let viewer;
try {
  const app = await guest.newPage({ viewport: null });
  await app.goto('data:text/html,' + encodeURIComponent(
    '<style>body{font:36px sans-serif;margin:60px}input{font:inherit;height:160px}</style>' +
    '<label>Monitor acceptance<br><input placeholder="Type here"></label>'));
  await app.bringToFront();
  // Locate the input in desktop coordinates including the browser chrome.
  tool('key', 'F11');
  await app.waitForFunction(() => innerHeight === screen.height);
  const point = await app.locator('input').evaluate(el => {
    const r = el.getBoundingClientRect();
    return [Math.round(r.x + 30),
            Math.round(r.y + r.height / 2)];
  });
  const value = 'Zuse-' + randomBytes(4).toString('hex');
  await app.locator('input').evaluate((input, expected) => {
    input.addEventListener('input', () => {
      input.style.backgroundColor = input.value === expected ? 'lime' : 'white';
    });
  }, value);
  viewer = await chromium.launch(options);
  const context = await viewer.newContext({ httpCredentials: { username: 'viewer', password: tool('password') },
    viewport: { width, height } });
  const open = async () => {
    const page = await context.newPage();
    // The viewer consumes the fragment before requesting authenticated video.
    const port = Number(process.env.MONITOR_VIEWER_PORT || 6901);
    assert(Number.isInteger(port) && port > 0 && port <= 65535);
    await page.goto(`http://127.0.0.1:${port}/#` + encodeURIComponent(tool('password')));
    await page.waitForFunction(width => typeof VideoDecoder === 'function' &&
      [...document.querySelectorAll('video,canvas')].some(el => (el.videoWidth || el.width) === width), width);
    return page;
  };
  let page = await open();
  tool('click', ...point.map(String));
  await page.keyboard.type('viewer-must-be-read-only');
  tool('type', value);
  await app.waitForFunction(v => document.querySelector('input').value === v, value);
  const capture = async page => {
    // Look for the black text strokes in the input field on the decoded desktop.
    await page.waitForFunction(([x, y, width]) => {
      const src = [...document.querySelectorAll('video,canvas')].find(el => (el.videoWidth || el.width) === width && el.getBoundingClientRect().width);
      if (!src) return false;
      const dst = document.createElement('canvas');
      dst.width = 140; dst.height = 24;
      const ctx = dst.getContext('2d');
      ctx.drawImage(src, x - 22, y - 12, 140, 24, 0, 0, 140, 24);
      const pixels = ctx.getImageData(0, 0, 140, 24).data;
      let dark = 0, green = 0;
      for (let i = 0; i < pixels.length; i += 4) {
        if (pixels[i+1] < 60 && pixels[i+3] === 255) dark++;
        if (pixels[i] < 60 && pixels[i+1] > 180) green++;
      }
      return green > 300 && dark > 50;
    }, [...point, width]);
  };
  await capture(page);
  const shot = JSON.parse(tool('screenshot', resolve(process.argv[2] || 'agent.png')));
  assert.equal(shot.width, width);
  assert.equal(shot.height, height);
  await page.close();
  page = await open();
  await capture(page);
  await page.screenshot({ path: resolve(process.argv[3] || 'viewer.png') });
  console.log(JSON.stringify({ value, shot, viewerReopened: true }));
} finally {
  await Promise.all([guest.close(), viewer?.close()]);
}
