// Run the actual Solid 2 development runtime and compiler, including Kobalte.
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { openBrowser } from "./harness.mjs";

const app = fileURLToPath(new URL("..", import.meta.url));
const server = await createServer({ root: app, configFile: `${app}/vite.config.ts`, server: { host: "127.0.0.1", port: 0, open: false } });
await server.listen();
const page = await openBrowser(`http://127.0.0.1:${server.httpServer.address().port}/e2e/solid2/fixture.html`);
try {
  await page.waitFor("!!window.solid2 && document.querySelectorAll('.virtual-row').length > 0");
  await page.eval("window.solid2.transcriptChecks()");
  assert.deepEqual(await page.eval("window.solid2.counters().effectRuns"), 1);
  await page.eval("window.solid2.update()");
  assert.deepEqual(await page.eval("[window.solid2.counters().effectRuns, window.solid2.counters().cleanups]"), [2, 1]);
  await page.eval("window.solid2.unmount()");
  assert.equal(await page.eval("window.solid2.counters().cleanups"), 2);
  const mounts = await page.eval("window.solid2.counters().rowMounts");
  assert.ok(mounts > 0 && mounts < 30, `expected virtualized rows, saw ${mounts}`);
  await page.eval("window.solid2.measure()");
  await new Promise(resolve => setTimeout(resolve, 150));
  assert.equal(await page.eval("window.solid2.counters().rowMounts"), mounts, "measurement must preserve row identity");
  await page.press("#dialog-trigger");
  await page.waitFor("!!document.querySelector('[role=dialog]') && document.activeElement?.matches('.dialog-close')");
  await page.press(".dialog-close");
  await page.waitFor("!document.querySelector('[role=dialog]')");
  await page.press("#menu-trigger");
  await page.waitFor("!!document.querySelector('[role=menuitem]')");
  await page.press("[role=menuitem]");
  await page.waitFor("window.solid2.choice() === 'picked'");
  assert.deepEqual(page.errors, []);
  console.log("Solid 2 migration checks passed (frames, batching, cleanup, virtual rows, dialogs, menus)");
} finally {
  await page.close();
  await server.close();
}
