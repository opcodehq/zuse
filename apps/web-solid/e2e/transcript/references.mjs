// Inline references in the transcript: chips for every kind, live status icons, navigation, issue activity rows,
// the open-ask bar and the issue list's status icons, in light and dark.
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
const url = (theme, view = "") => `http://127.0.0.1:${server.httpServer.address().port}/e2e/transcript/references.html?theme=${theme}${view ? `&view=${view}` : ""}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const ready = "document.querySelectorAll('.t-entry .md-ref').length > 10";
const tone = (selector, token) => `(() => { const s = document.createElement('span'); s.style.color = '${token}'; document.body.append(s); const want = getComputedStyle(s).color; s.remove(); const el = document.querySelector('${selector}'); return !!el && getComputedStyle(el).color === want; })()`;
let page;
try {
  page = await openBrowser(url("dark"), { width: 1100, height: 1000 });
  await page.waitFor(ready, 30000);
  const refs = (await page.eval("[...document.querySelectorAll('.t-entry[data-role=assistant] .md-ref')].map(a => [a.dataset.ref, a.dataset.refKind, a.dataset.status ?? null, a.textContent])")).sort((a, b) => a[0].localeCompare(b[0]));
  assert.deepEqual(refs, [
    ["ZAP-18", "issue", "needsYou", "ZAP-18"],
    ["zuse#378", "pr", "open", "zuse#378"],
    ["opcodehq/zuse@39d73fe", "commit", null, "39d73fe"],
    ["website-pricing-v2", "branch", null, "website-pricing-v2"],
    ["apps/web/src/pricing.tsx", "file", null, "apps/web/src/pricing.tsx:42"],
    ["reports/ZUSA-16.md", "file", null, "ZUSA-16.md"],
    ["c7e1f0a2-9b3d", "thread", null, "Pricing page copy"],
    ["c7e1--sub--verify", "agent", "done", "Verify pricing copy"],
    ["ZAP-17", "issue", "done", "ZAP-17"],
    ["opcodehq/zuse#375", "pr", "merged", "opcodehq/zuse#375"],
    ["opcodehq/zuse@e4166aaf", "commit", null, "zuse@e4166aa"],
    ["ZAP-21", "issue", "inProgress", "ZAP-21"],
    ["#370", "pr", "closed", "#370"],
    ["Website", "zapper", "active", "Website"],
    ["pricing-mock.png", "attachment", null, "pricing-mock.png"],
    ["opcodehq/zuse#371", "pr", "merged", "zuse#371"],
  ].map(([ref, kind, status, text]) => [ref, kind, status, text]).sort((a, b) => a[0].localeCompare(b[0])));
  assert.equal(await page.eval("document.querySelectorAll('code .md-ref, pre .md-ref, .md-ref .md-ref').length"), 0, "code stays plain");
  assert.equal(await page.eval("document.querySelector('.t-entry[data-role=assistant] li').textContent"), "Unknown things stay plain: ZAP-999, other/zuse#378, src/missing.ts, a1b2c3d and main.");
  assert.deepEqual(await page.eval("[...document.querySelectorAll('.md-ref')].filter(a => !a.getAttribute('aria-label') || !a.querySelector('.md-ref-icon > *')).map(a => a.dataset.ref)"), []);
  assert.equal(await page.eval("document.querySelector('[data-ref=\"apps/web/src/pricing.tsx\"] .md-ref-icon img').getAttribute('src').includes('react-ts.svg')"), true, "file chips use the file-type icon");
  assert.equal(await page.eval("document.querySelector('[data-ref=\"opcodehq/zuse@39d73fe\"]').href"), "https://github.com/opcodehq/zuse/commit/39d73fe");

  const summaries = () => page.eval("[...document.querySelectorAll('.t-activity')].map(g => [g.querySelector('.t-activity-summary').textContent.trim(), g.querySelectorAll('.t-activity-row').length])");
  assert.deepEqual(await summaries(), [["6 issue updates", 0], ["2 issue updates, ZAP-17 closed as merged", 3]]);
  assert.deepEqual(await page.eval("[...document.querySelectorAll('.t-activity-row')].map(r => r.querySelector('.t-activity-actor').textContent + ' ' + r.querySelector('.t-activity-text').textContent)"), ["Website asked you about ZAP-18", "Website moved ZAP-18 to Needs you", "Website closed ZAP-17 as merged"]);
  for (const theme of ["dark", "light"]) {
    await page.goto(url(theme));
    await page.waitFor(ready, 30000);
    for (const [selector, token] of [
      ['.t-entry [data-ref="ZAP-18"] .status-icon', "var(--danger)"],
      ['.t-entry [data-ref="ZAP-17"] .status-icon', "var(--status-done)"],
      ['.t-entry [data-ref="ZAP-21"] .status-icon', "var(--status-progress)"],
      ['.t-entry [data-ref="zuse#378"] .md-ref-icon', "var(--success)"],
      ['.t-entry [data-ref="#370"] .md-ref-icon', "var(--danger)"],
      [".t-entry .md-ref", "var(--syntax-function)"],
      [".t-activity-row .status-icon[data-status=needsYou]", "var(--danger)"],
    ]) assert.equal(await page.eval(tone(selector, token)), true, `${theme} ${selector}`);
    await page.click(".t-activity-divider");
    await page.waitFor("document.querySelectorAll('.t-activity-row').length === 9");
    await sleep(400);
    if (shots) await page.screenshot(`${shots}/after-${theme}.png`);
    await page.goto(url(theme, "issues"));
    await page.waitFor("document.querySelectorAll('.zp-issue-status .status-icon').length === 6", 30000);
    await page.press('.zp-select[aria-label="Issue status filter"]');
    await page.waitFor("document.querySelectorAll('.menu-item .status-icon').length === 8");
    assert.deepEqual(await page.eval("[...document.querySelectorAll('.menu-item')].filter(i => i.querySelector('.status-icon')).map(i => [i.querySelector('.status-icon').dataset.status, i.textContent.trim()])"), [["needsYou", "Needs you"], ["inReview", "In review"], ["inProgress", "In progress"], ["blocked", "Blocked"], ["todo", "Todo"], ["backlog", "Backlog"], ["done", "Done"], ["canceled", "Canceled"]]);
    await sleep(300);
    if (shots) await page.screenshot(`${shots}/after-issues-${theme}.png`);
  }

  await page.goto(url("dark"));
  await page.waitFor(ready, 30000);
  await page.eval("__refs.status(18, 'inReview')");
  assert.equal(await page.eval("document.querySelector('.t-entry [data-ref=\"ZAP-18\"] .status-icon').dataset.status"), "inReview", "status follows the store");
  assert.equal(await page.eval("document.querySelectorAll('.t-waiting').length"), 0, "answered asks leave");
  await page.eval("__refs.status(18, 'needsYou')");
  await page.click('.t-entry [data-ref="ZAP-21"]');
  assert.deepEqual(await page.eval("__refs.destination().page"), { kind: "issue", id: "computers" });
  await page.click('.t-entry [data-ref="zuse#378"]');
  assert.deepEqual(await page.eval("__refs.destination().page"), { kind: "pr", url: "https://github.com/opcodehq/zuse/pull/378" });
  await page.click('.t-entry [data-ref="apps/web/src/pricing.tsx"]');
  assert.deepEqual(await page.eval("__refs.destination().file"), { path: "apps/web/src/pricing.tsx", line: 42 });
  await page.click('.t-entry [data-ref="reports/ZUSA-16.md"]');
  assert.deepEqual(await page.eval("__refs.destination().file"), { path: "reports/ZUSA-16.md", line: null });
  await page.click('.t-entry [data-ref="c7e1--sub--verify"]');
  assert.deepEqual(await page.eval("__refs.destination().agent"), { chatId: "c-coordinator", docId: "c7e1--sub--verify" });
  await page.click('.t-entry [data-ref="Website"]');
  assert.equal(await page.eval("__refs.destination().route"), "zappers");
  assert.equal(await page.eval("document.querySelectorAll('.t-waiting').length"), 1);
  await page.click(".t-waiting-dismiss");
  assert.equal(await page.eval("document.querySelectorAll('.t-waiting').length"), 0);
  assert.equal(await page.eval("localStorage.getItem('zuse.dismissedAsks')"), JSON.stringify(["question:q-402"]));
  await page.click('.t-entry [data-ref="website-pricing-v2"]');
  assert.equal(await page.eval("__refs.destination().chat"), "c7e1f0a2-9b3d");
  assert.deepEqual(page.errors.filter(e => !/image|ReadAttachment|connection/i.test(e)), []);
  console.log("references ok");
} finally {
  await page?.close();
  await server.close();
}
