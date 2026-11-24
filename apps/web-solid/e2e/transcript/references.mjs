import assert from "node:assert/strict";
import { createServer } from "vite";
import { openBrowser } from "../harness.mjs";
const root = new URL("../..", import.meta.url).pathname;
const server = await createServer({ root, configFile: `${root}/vite.config.ts`, server: { host: "127.0.0.1", port: 0, hmr: { overlay: false } } });
await server.listen();
let page;
try {
  page = await openBrowser(`http://127.0.0.1:${server.httpServer.address().port}/e2e/transcript/references.html`, { width: 1000, height: 800 });
  await page.waitFor("!!window.referenceFixture && document.querySelectorAll('.md-reference').length > 5");
  assert.equal(await page.eval("document.querySelectorAll('a a, code .md-reference, pre .md-reference, #generic .md-reference').length"), 0);
  assert.equal(await page.eval("document.querySelector('a[href=\"https://example.com\"]').textContent"), "ZAP-12");
  assert.equal(await page.eval("document.querySelectorAll('[data-reference=\"ZAP-999\"], [data-reference=\"Subagent 1\"]').length"), 0);
  assert.equal(await page.eval("document.querySelector('[data-reference=\"ZAP-12\"]').dataset.status"), "inProgress");
  assert.equal(await page.eval("document.querySelector('[data-reference=\"ZAP-11\"]').dataset.status"), "done");
  assert.equal(await page.eval("(() => { const probe=document.createElement('span');probe.style.color='var(--syntax-function)';document.body.append(probe);const matches=getComputedStyle(document.querySelector('.md-reference')).color===getComputedStyle(probe).color;probe.remove();return matches; })()"), true, "reference links stay blue under transcript styles");
  for (const theme of ["dark", "light"]) {
    await page.eval(`document.documentElement.dataset.theme=${JSON.stringify(theme)}`);
    for (const [status, token] of [["inProgress", "--warning"], ["done", "--syntax-keyword"], ["needsYou", "--danger"], ["inReview", "--success"], ["blocked", "--danger"], ["todo", "--text-muted"], ["backlog", "--text-muted"], ["canceled", "--text-muted"]]) {
      await page.eval(`referenceFixture.status(${JSON.stringify(status)})`);
      assert.equal(await page.eval(`(() => {const icon=document.querySelector('[data-reference="ZAP-12"] .issue-state-icon');const probe=document.createElement('span');probe.style.color='var(${token})';document.body.append(probe);const matches=getComputedStyle(icon).color===getComputedStyle(probe).color;probe.remove();return matches && !!icon.querySelector('svg') && icon.dataset.status===${JSON.stringify(status)};})()`), true, `${theme} ${status} uses shared semantic icon and token`);
    }
  }
  await page.eval("document.documentElement.dataset.theme='dark'; referenceFixture.status('inProgress')");
  assert.equal(await page.eval("document.querySelector('.md-reference[data-kind=pr]').rel"), "noopener noreferrer");
  assert.equal(await page.eval("[...document.querySelectorAll('.md-reference')].every(a => a.getAttribute('aria-label') && a.href && a.querySelector('[aria-hidden=true]'))"), true);
  await page.click('[data-reference="ZAP-12"]');
  assert.deepEqual(await page.eval("referenceFixture.destination().page"), { kind: "issue", id: "progress" });
  await page.eval("document.querySelector('[data-reference=\"ZAP-12\"]').focus()");
  await page.key("Enter");
  assert.deepEqual(await page.eval("referenceFixture.destination().page"), { kind: "issue", id: "progress" });
  await page.click('[data-reference="parent--sub--verify"]');
  assert.deepEqual(await page.eval("referenceFixture.destination().agent"), { chatId: "parent", docId: "parent--sub--verify" });
  await page.eval("referenceFixture.status('needsYou')");
  await page.waitFor("[...document.querySelectorAll('[data-reference=\"ZAP-12\"]')].every(a=>a.dataset.status==='needsYou')");
  await page.eval("referenceFixture.status('inProgress')");
  await page.eval("referenceFixture.title('<img src=x onerror=alert(1)>')");
  assert.equal(await page.eval("document.querySelector('[data-reference=\"ZAP-12\"]').title"), "<img src=x onerror=alert(1)>");
  assert.equal(await page.eval("document.querySelectorAll('img, [onerror]').length"), 0);
  await page.eval("referenceFixture.title('Improve conversation references')");
  await page.eval("document.querySelector('main').style.maxWidth='260px'");
  assert.equal(await page.eval("document.querySelector('main').scrollWidth <= document.querySelector('main').clientWidth"), true);
  assert.equal(await page.eval("getComputedStyle(document.querySelector('.md-reference')).whiteSpace"), "normal");
  assert.equal(await page.eval("(() => { const style=getComputedStyle(document.querySelector('.md-reference-icon')); return Math.abs(parseFloat(style.verticalAlign) / parseFloat(style.fontSize) + 0.12) < 0.01; })()"), true);
  await page.eval("referenceFixture.text('parent--sub--verify'); document.querySelector('main').style.maxWidth='85px'");
  assert.equal(await page.eval("document.querySelector('.md-reference').getClientRects().length > 1"), true);
  await page.eval("referenceFixture.text('ZAP-12 '.repeat(500))");
  assert.equal(await page.eval("document.querySelectorAll('.md-reference').length"), 100);
  await page.eval("referenceFixture.text('ZAP-12 '.repeat(12000))");
  assert.equal(await page.eval("document.querySelectorAll('.md-reference').length"), 0);
  await page.eval("referenceFixture.text('ZAP-12 [ZAP-12](javascript:alert(1))\\n\\n<script>alert(1)</script>')");
  assert.equal(await page.eval("document.querySelectorAll('script:not([type=module]), a[href^=javascript]').length"), 0);
  assert.equal(await page.eval("document.querySelectorAll('.md-reference').length"), 1);
  await page.eval("referenceFixture.text('ZAP-12 and ZAP-11 are tracked in linked-thread with owner/repo#34. The supporting conversation is parent--sub--verify. Unknown ZAP-999 remains ordinary text.'); document.querySelector('main').style.maxWidth='540px'");
  if (process.env.ZUSE_REFERENCE_SCREENSHOT) {
    await page.screenshot(process.env.ZUSE_REFERENCE_SCREENSHOT);
    await page.eval("document.documentElement.dataset.theme='light'");
    await page.screenshot(process.env.ZUSE_REFERENCE_SCREENSHOT.replace(/\.png$/, "-light.png"));
  }
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/e2e/transcript/reference-navigation.html`);
  await page.waitFor("!!window.referenceNavigation");
  await page.eval("referenceNavigation.start()");
  await page.waitFor("!!document.querySelector('.t-entry .md-reference[data-reference=\"ZAP-12\"]')");
  assert.deepEqual(await page.eval("referenceNavigation.state()"), { route: { kind: "chat" }, chat: "linked", rightPanel: "files" });
  assert.equal(await page.eval("document.querySelectorAll('.zp-inspector').length"), 0);
  await page.press('.t-entry .md-reference[data-reference="ZAP-12"]');
  await page.waitFor("!!document.querySelector('.zp-inspector .zp-issue-page')");
  assert.deepEqual(await page.eval("referenceNavigation.state()"), { route: { kind: "zappers", zapperId: "captain" }, chat: "linked", rightPanel: null });
  assert.equal(await page.eval("(() => {const issue=document.querySelector('.zp-inspector .zp-issue-page'); const rect=issue.getBoundingClientRect();return rect.width>0 && rect.height>0 && getComputedStyle(issue).visibility!=='hidden' && issue.textContent.includes('Match the desktop model picker');})()"), true);
  await page.eval("referenceNavigation.coverInspector()");
  await page.waitFor("!document.querySelector('.zp-inspector')");
  await page.press('.t-entry .md-reference[data-reference="ZAP-12"]');
  await page.waitFor("!!document.querySelector('.zp-inspector .zp-issue-page')");
  assert.equal(await page.eval("referenceNavigation.state().rightPanel"), null);
  await page.waitFor("!document.querySelector('.app-right')");
  if (process.env.ZUSE_REFERENCE_SCREENSHOT) await page.screenshot(process.env.ZUSE_REFERENCE_SCREENSHOT.replace(/\.png$/, "-navigation.png"));
  assert.deepEqual(page.errors, []);
  console.log("PASS inline reference rendering, state updates, exclusions, navigation, accessibility, wrapping and bounds");
} finally {
  if (page) await page.close();
  await server.close();
}
