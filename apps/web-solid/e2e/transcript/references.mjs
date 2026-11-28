// Inline references in the transcript: chips, live status, navigation and the open-ask bar, light and dark.
// Usage: node e2e/transcript/references.mjs (ZUSE_SHOTS=dir keeps screenshots).
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { createServer } from "vite";
import { openBrowser } from "../harness.mjs";

const root = new URL("../..", import.meta.url).pathname;
const shots = process.env.ZUSE_SHOTS;
if (shots) mkdirSync(shots, { recursive: true });
const server = await createServer({ root, configFile: `${root}/vite.config.ts`, server: { host: "127.0.0.1", port: 0, hmr: { overlay: false } } });
await server.listen();
const url = (theme) => `http://127.0.0.1:${server.httpServer.address().port}/e2e/transcript/references.html?theme=${theme}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let page;
try {
  page = await openBrowser(url("dark"), { width: 1100, height: 760 });
  await page.waitFor("document.querySelectorAll('.md-ref').length > 5", 30000);
  const refs = await page.eval("[...document.querySelectorAll('.t-entry .md-ref')].map(a => [a.dataset.ref, a.dataset.refKind, a.dataset.status ?? null, a.textContent])");
  assert.deepEqual(refs, [
    ["ZAP-18", "issue", "needsYou", "ZAP-18"],
    ["zuse#378", "pr", "open", "zuse#378"],
    ["c7e1f0a2-9b3d", "thread", null, "Pricing page copy"],
    ["c7e1--sub--verify", "agent", "done", "Verify pricing copy"],
    ["ZAP-17", "issue", "done", "ZAP-17"],
    ["opcodehq/zuse#375", "pr", "merged", "opcodehq/zuse#375"],
    ["ZAP-21", "issue", "inProgress", "ZAP-21"],
    ["#370", "pr", "closed", "#370"],
    ["opcodehq/zuse#371", "pr", "merged", "zuse#371"],
  ]);
  assert.equal(await page.eval("document.querySelectorAll('code .md-ref, pre .md-ref, a .md-ref, .md-ref .md-ref').length"), 0, "code and links stay plain");
  assert.equal(await page.eval("document.querySelector('.t-entry[data-role=assistant] .md').textContent.includes('ZAP-999 or other/zuse#378')"), true);
  assert.deepEqual(await page.eval("[...document.querySelectorAll('.md-ref')].filter(a => !a.getAttribute('aria-label') || !a.querySelector('.md-ref-icon svg')).map(a => a.dataset.ref)"), []);
  assert.equal(await page.eval("document.querySelector('.t-waiting .t-waiting-text').textContent"), "Waiting on you for ZAP-18: Fix Vercel billing (site 402), then merge zuse#378.");
  for (const theme of ["dark", "light"]) {
    await page.goto(url(theme));
    await page.waitFor("document.querySelectorAll('.md-ref').length > 5", 30000);
    const tones = await page.eval(`(() => { const probe = (v) => { const s = document.createElement('span'); s.style.color = v; document.body.append(s); const c = getComputedStyle(s).color; s.remove(); return c; };
      const icon = (ref) => getComputedStyle(document.querySelector('.t-entry [data-ref="' + ref + '"] .md-ref-icon')).color;
      return { needs: icon('ZAP-18') === probe('var(--danger)'), open: icon('zuse#378') === probe('var(--success)'), merged: icon('opcodehq/zuse#375') === probe('#a78bfa'), closed: icon('#370') === probe('var(--danger)'), done: icon('ZAP-17') === probe('var(--success)'), link: getComputedStyle(document.querySelector('.md-ref')).color === probe('var(--syntax-function)') }; })()`);
    assert.deepEqual(tones, { needs: true, open: true, merged: true, closed: true, done: true, link: true }, `${theme} tones`);
    await sleep(300);
    if (shots) await page.screenshot(`${shots}/after-${theme}.png`);
  }
  await page.eval("__refs.status(18, 'inReview')");
  assert.equal(await page.eval("document.querySelector('.t-entry [data-ref=\"ZAP-18\"]').dataset.status"), "inReview", "status follows the store");
  assert.equal(await page.eval("document.querySelectorAll('.t-waiting').length"), 0, "answered asks leave");
  await page.eval("__refs.status(18, 'needsYou')");
  await page.click('.t-entry [data-ref="ZAP-21"]');
  assert.deepEqual(await page.eval("__refs.destination().page"), { kind: "issue", id: "computers" });
  await page.click('.t-entry [data-ref="zuse#378"]');
  assert.deepEqual(await page.eval("__refs.destination().page"), { kind: "pr", url: "https://github.com/opcodehq/zuse/pull/378" });
  await page.click('.t-entry [data-ref="c7e1--sub--verify"]');
  assert.deepEqual(await page.eval("__refs.destination().agent"), { chatId: "c-coordinator", docId: "c7e1--sub--verify" });
  assert.equal(await page.eval("document.querySelectorAll('.t-waiting').length"), 1);
  await page.click(".t-waiting-dismiss");
  assert.equal(await page.eval("document.querySelectorAll('.t-waiting').length"), 0);
  assert.equal(await page.eval("localStorage.getItem('zuse.dismissedAsks')"), JSON.stringify(["question:q-402"]));
  await page.click('.t-entry [data-ref="c7e1f0a2-9b3d"]');
  assert.equal(await page.eval("__refs.destination().chat"), "c7e1f0a2-9b3d");
  assert.deepEqual(page.errors, []);
  console.log("references ok");
} finally {
  await page?.close();
  await server.close();
}
