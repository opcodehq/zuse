// Shared states and toasts in headless Chromium: ErrorScreen focus, retry, copy and details; EmptyState and Skeleton;
// toast pause on hover and focus, close, alert role. Usage: ZUSE_SHOTS=/tmp/dir node e2e/extras-fixture/run.mjs
import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { openBrowser } from "../harness.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const app = join(here, "../..");
const out = mkdtempSync(join(tmpdir(), "zuse-extras-fixture-"));
const shots = process.env.ZUSE_SHOTS;
execFileSync(join(app, "node_modules/.bin/vite"), ["build", "--config", join(here, "vite.config.mjs"), "--logLevel", "error"], { cwd: app, env: { ...process.env, ZUSE_FIXTURE_OUT: out }, stdio: "inherit" });
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".ttf": "font/ttf", ".woff2": "font/woff2" };
const server = createServer((req, res) => {
  const path = join(out, decodeURIComponent(new URL(req.url, "http://x").pathname));
  if (!existsSync(path) || path.endsWith("/")) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" }).end(readFileSync(path));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}/fixture.html`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  try { await fn(); results.push(true); console.log("ok"); } catch (error) { results.push(false); console.log(`FAIL\n    ${error.message}`); }
};
const expect = (cond, what) => { if (!cond) throw new Error(what); };
const page = await openBrowser(`${base}?theme=dark`, { width: 1280, height: 800 });
await page.call("Emulation.setFocusEmulationEnabled", { enabled: true });
const origin = new URL(base).origin;
await page.call("Browser.grantPermissions", { origin, permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"] });
const mouse = async (x, y) => page.call("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
try {
  await page.waitFor(`!!window.__fx`);
  await step("ErrorScreen takes focus on its heading and is announced", async () => {
    await page.waitFor(`document.activeElement?.id === "x-error-title"`, 3000);
    expect(await page.eval(`document.querySelector(".x-error-screen").getAttribute("role") === "alert"`), "role");
    await sleep(300);
    shots && (await page.screenshot(join(shots, "fx-01-dark.png")));
  });
  await step("Copy details puts the message and stack on the clipboard", async () => {
    await page.eval(`[...document.querySelectorAll(".x-error-actions button")].find((b) => b.textContent.includes("Copy details")).click(); true`);
    await page.waitFor(`[...document.querySelectorAll(".x-error-actions button")].some((b) => b.textContent.includes("Copied"))`, 3000);
    const copied = await page.eval(`navigator.clipboard.readText()`);
    expect(copied.startsWith("TypeError: Cannot read properties") && copied.includes("at "), `clipboard: ${copied.slice(0, 80)}`);
  });
  await step("Details expand with the stack", async () => {
    await page.eval(`document.querySelector(".x-error-details summary").click(); true`);
    expect(await page.eval(`document.querySelector(".x-error-details").open`), "not open");
    shots && (await page.screenshot(join(shots, "fx-02-details.png")));
  });
  await step("Try again calls the boundary's reset", async () => {
    await page.eval(`[...document.querySelectorAll(".x-error-actions button")].find((b) => b.textContent.includes("Try again")).click(); true`);
    await page.waitFor(`window.__fx.resets() === 1 && !document.querySelector(".x-error-screen")`);
  });
  await step("A toast pauses while hovered and dismisses after the pointer leaves", async () => {
    await page.eval(`window.__fx.toast("Saved the draft"); true`);
    await page.waitFor(`!!document.querySelector(".toast")`);
    const box = await page.eval(`(() => { const r = document.querySelector(".toast").getBoundingClientRect(); return { x: r.x + 20, y: r.y + r.height / 2 } })()`);
    await mouse(box.x, box.y);
    await sleep(5000);
    expect(await page.eval(`!!document.querySelector(".toast")`), "dismissed while hovered");
    await mouse(10, 10);
    await page.waitFor(`!document.querySelector(".toast")`, 6000);
  });
  await step("A focused toast stays; its close button removes it; danger toasts are alerts", async () => {
    await page.eval(`window.__fx.toast("Couldn't archive the thread.", "danger"); true`);
    await page.waitFor(`!!document.querySelector('.toast[role="alert"]')`);
    await page.eval(`document.querySelector(".toast-close").focus(); true`);
    await sleep(9000);
    expect(await page.eval(`!!document.querySelector(".toast")`), "dismissed while focused");
    shots && (await page.screenshot(join(shots, "fx-03-toast.png")));
    await page.eval(`document.querySelector(".toast-close").click(); true`);
    await page.waitFor(`!document.querySelector(".toast")`, 2000);
  });
  await step("Light theme", async () => {
    await page.goto(`${base}?theme=light`);
    await page.waitFor(`!!window.__fx`);
    await page.eval(`window.__fx.toast("Thread archived"); true`);
    await sleep(400);
    shots && (await page.screenshot(join(shots, "fx-04-light.png")));
  });
  if (page.errors.length) throw new Error(`page errors:\n${page.errors.join("\n")}`);
} finally {
  await page.close();
  server.close();
  rmSync(out, { recursive: true, force: true });
  console.log(`${results.filter(Boolean).length}/${results.length} passed`);
  if (results.includes(false)) process.exitCode = 1;
}
