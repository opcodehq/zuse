// The issue conversation in the browser: reply drafts survive live updates and in-flight
// submits, and run state expires on a fake clock. Real component, in-memory store.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { openBrowser } from "./harness.mjs";
const app = fileURLToPath(new URL("..", import.meta.url));
const server = await createServer({ root: app, configFile: app + "/vite.config.ts", server: { host: "127.0.0.1", port: 0, open: false, hmr: { overlay: false } } });
await server.listen();
const url = "http://127.0.0.1:" + server.httpServer.address().port + "/e2e/zapper-conversation/fixture.html";
const q = JSON.stringify;
const fill = (page, selector, value) => page.eval("(()=>{const e=document.querySelector(" + q(selector) + ");e.value=" + q(value) + ";e.dispatchEvent(new Event('input',{bubbles:true}));})()");
const value = (page, selector) => page.eval("document.querySelector(" + q(selector) + ")?.value ?? null");
const reply = "textarea[aria-label=Reply]";
const submitReply = (page) => page.eval("document.querySelector('form.zp-composer[data-reply]').requestSubmit()");
const openReply = async (page) => {
  await page.click(".zp-issue-card button[aria-label=Reply]");
  await page.waitFor("!!document.querySelector(" + q(reply) + ")");
};
const stages = {
  // An open reply keeps its draft while unrelated activity arrives.
  async "live update"(page) {
    await openReply(page);
    await fill(page, reply, "half typed");
    await page.eval("conv.arrive('priority-1')");
    await page.eval("conv.arrive('priority-2')");
    await page.waitFor("document.querySelectorAll('.zp-event').length === 3");
    assert.equal(await value(page, reply), "half typed", "a live update keeps the open reply and its draft");
  },
  // Text typed while the reply is saving is kept; only the sent text is cleared.
  async "pending submit"(page) {
    await openReply(page);
    await fill(page, reply, "Ship B");
    await submitReply(page);
    await page.waitFor("conv.pending()");
    await fill(page, reply, "Ship B, and then the scope");
    await page.eval("conv.confirm()");
    await page.waitFor("!conv.pending() && !document.querySelector('form.zp-composer button[type=submit]')?.textContent.includes('Sending')");
    assert.equal(await value(page, reply), "Ship B, and then the scope", "a newer draft survives the confirmation");
    assert.deepEqual((await page.eval("conv.sent()")).map(a => a.text), ["Ship B"]);
    // Sending what is in the box clears it and closes the reply once confirmed.
    await submitReply(page);
    await page.waitFor("conv.pending()");
    await page.eval("conv.confirm()");
    await page.waitFor("!document.querySelector(" + q(reply) + ")");
  },
  // Working expires to "can't confirm" as time passes, with nothing else changing.
  async "stale run state"(page) {
    await page.eval("conv.session('working')");
    await page.waitFor("document.querySelector('.zp-run-state')?.textContent.includes('is working')");
    await page.eval("clock.advance(30000)");
    assert.match(await page.eval("document.querySelector('.zp-run-state').textContent"), /is working/, "still fresh inside the window");
    await page.eval("clock.advance(30000)");
    await page.waitFor("document.querySelector('.zp-run-state')?.textContent.includes(\"Can't confirm\")", 5000);
  },
};
const failed = [];
for (const [name, stage] of Object.entries(stages)) {
  const page = await openBrowser(url, { width: 1100, height: 900 });
  try {
    await page.waitFor("!!window.conv && !!document.querySelector('.zp-issue-card button[aria-label=Reply]')");
    await stage(page);
    assert.deepEqual(page.errors, [], name + ": no page errors");
    console.log("ok   " + name);
  } catch (error) {
    console.log("FAIL " + name + ": " + error.message.split("\n")[0]);
    failed.push(name);
  } finally {
    await page.close();
  }
}
await server.close();
if (failed.length) { console.error("Zapper conversation e2e failed: " + failed.join(", ")); process.exit(1); }
console.log("Zapper conversation e2e passed.");
