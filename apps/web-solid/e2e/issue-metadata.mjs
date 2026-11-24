import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { createServer } from "vite";
import { openBrowser } from "./harness.mjs";

const root = new URL("..", import.meta.url).pathname;
const server = await createServer({ root, configFile: `${root}/vite.config.ts`, server: { host: "127.0.0.1", port: 0, hmr: { overlay: false } } });
await server.listen();
const output = process.env.ZUSE_ISSUE_SCREENSHOTS;
if (output) mkdirSync(output, { recursive: true });
let page;
try {
  page = await openBrowser(`http://127.0.0.1:${server.httpServer.address().port}/e2e/issue-metadata/fixture.html`, { width: 1000, height: 900 });
  await page.waitFor("document.querySelectorAll('.zp-issue-row').length === 6");
  await page.press('[aria-label="Issue status filter"]');
  await page.waitFor("document.querySelectorAll('[role=menuitem]').length === 10");
  await page.eval("[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent==='All issues').setAttribute('data-all-issues','')");
  await page.press("[data-all-issues]");
  await page.waitFor("document.querySelectorAll('.zp-issue-row').length === 8");
  for (const theme of ["dark", "light"]) {
    await page.eval(`document.documentElement.dataset.theme = '${theme}'`);
    assert.equal(await page.eval("new Set([...document.querySelectorAll('.zp-issue-row .issue-state-icon')].map(e=>e.innerHTML + getComputedStyle(e).color)).size"), 8);
    assert.equal(await page.eval("new Set([...document.querySelectorAll('.zp-issue-row .issue-priority-icon')].map(e=>e.innerHTML + getComputedStyle(e).color)).size"), 4);
    assert.equal(await page.eval("[...document.querySelectorAll('.zp-issue-row [data-priority=high] .issue-priority-bars i')].every(e=>getComputedStyle(e).opacity === '1')"), true);
    assert.equal(await page.eval(`(() => {
      const luminance = color => { const rgb = color.match(/[\\d.]+/g).slice(0,3).map(Number).map(v => { v /= 255; return v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; }); return rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722; };
      const bg = luminance(getComputedStyle(document.body).backgroundColor);
      return [...document.querySelectorAll('.zp-issue-row .issue-state-icon, .zp-issue-row .issue-priority-icon')].every(e => { const fg = luminance(getComputedStyle(e).color); return (Math.max(fg,bg)+.05)/(Math.min(fg,bg)+.05) >= 3; });
    })()`), true, `${theme} issue icons meet 3:1 contrast against the row surface`);
    assert.equal(await page.eval("document.querySelectorAll('.issue-working[data-working]').length"), 4);
    if (output) await page.screenshot(`${output}/issues-${theme}.png`);
    await page.press('[aria-label="Status preview"]');
    await page.waitFor("document.querySelectorAll('[role=menuitem]').length === 8");
    assert.equal(await page.eval("document.querySelectorAll('[role=menuitem] .issue-state-icon').length"), 8);
    assert.equal(await page.eval("document.querySelectorAll('[aria-label=\"Status preview\"] .issue-state-icon').length"), 1);
    if (output) await page.screenshot(`${output}/status-menu-${theme}.png`);
    await page.key("Escape", "Escape", 27);
    await page.press('[aria-label="Priority preview"]');
    await page.waitFor("document.querySelectorAll('[role=menuitem]').length === 4");
    assert.equal(await page.eval("document.querySelectorAll('[role=menuitem] .issue-priority-icon').length"), 4);
    if (output) await page.screenshot(`${output}/priority-menu-${theme}.png`);
    await page.key("Escape", "Escape", 27);
  }
  const bounds = await page.eval("JSON.stringify([...document.querySelectorAll('.issue-working')].map(e=>[e.getBoundingClientRect().width,e.getBoundingClientRect().height]))");
  for (const status of ["idle", "completed", "awaitingInput", "errored"]) {
    await page.eval(`issueFixture.session('${status}')`);
    await page.waitFor("document.querySelectorAll('.issue-working[data-working]').length === 0");
    assert.equal(await page.eval("[...document.querySelectorAll('.issue-working i')].every(e=>getComputedStyle(e).animationName === 'none')"), true);
    assert.equal(await page.eval("JSON.stringify([...document.querySelectorAll('.issue-working')].map(e=>[e.getBoundingClientRect().width,e.getBoundingClientRect().height]))"), bounds);
  }
  await page.eval("issueFixture.session('working'); issueFixture.connected(false)");
  await page.waitFor("document.querySelectorAll('.issue-working[data-working]').length === 0");
  await page.eval("issueFixture.connected(true)");
  await page.waitFor("document.querySelectorAll('.issue-working[data-working]').length === 4");
  await page.eval("issueFixture.stale(true)");
  await page.waitFor("document.querySelectorAll('.issue-working[data-working]').length === 0");
  await page.eval("issueFixture.stale(false)");
  await page.waitFor("document.querySelectorAll('.issue-working[data-working]').length === 4");
  assert.equal(await page.eval("document.querySelector('.issue-working[data-working]').getAttribute('aria-label')"), "Assigned agent is working");
  await page.call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  assert.equal(await page.eval("getComputedStyle(document.querySelector('.issue-working i')).animationName"), "none");
  await page.press('[aria-label="Priority preview"]');
  await page.waitFor("document.querySelectorAll('[role=menuitem]').length === 4");
  await page.eval("document.querySelectorAll('[role=menuitem]')[1].focus()");
  await page.key("Enter");
  await page.waitFor("!document.querySelector('[role=menu]')");
  assert.equal(await page.eval("document.querySelector('[aria-label=\"Priority preview\"]').textContent.trim()"), "High");
  await page.call("Emulation.setEmulatedMedia", { features: [] });
  await page.eval("document.documentElement.dataset.reduceMotion = 'on'");
  assert.equal(await page.eval("getComputedStyle(document.querySelector('.issue-working i')).animationName"), "none");
  assert.deepEqual(page.errors, []);
  console.log("Issue metadata: both themes, all states/priorities, keyboard menus, activity transitions, connection gating, stable slots and reduced motion passed");
} finally {
  await page?.close();
  await server.close();
}
