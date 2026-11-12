// Transcript fixture checks in headless Chromium: rendering of every part kind, and smoothness under a
// synthetic session of thousands of messages with a fast token stream. Usage: node e2e/transcript/perf.mjs [messages]
import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdtempSync, readFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { openBrowser } from "../harness.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const app = join(here, "../..");
const out = mkdtempSync(join(tmpdir(), "zuse-transcript-fixture-"));
const shots = process.env.ZUSE_SHOTS ?? out;
execFileSync(join(app, "node_modules/.bin/vite"), ["build", "--config", join(here, "vite.config.mjs"), "--logLevel", "error"], {
  cwd: app,
  env: { ...process.env, ZUSE_FIXTURE_OUT: out },
  stdio: "inherit",
});

const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".ttf": "font/ttf", ".woff2": "font/woff2" };
const server = createServer((req, res) => {
  const path = join(out, decodeURIComponent(new URL(req.url, "http://x").pathname));
  if (!existsSync(path) || path.endsWith("/")) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" }).end(readFileSync(path));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}/fixture.html`;
const messages = Number(process.argv[2] ?? 5000);
const results = {};
const page = await openBrowser(`${base}?theme=dark`, { width: 1280, height: 860 });
const failures = [];
const check = (cond, what) => { if (!cond) failures.push(what); };
try {
  await page.waitFor(`!!window.__fixture`);
  const t0 = Date.now();
  await page.eval(`window.__fixture.seed(${messages})`);
  await page.waitFor(`document.querySelectorAll(".t-row").length > 0`);
  results.seedMs = Date.now() - t0;
  await new Promise((r) => setTimeout(r, 800));
  results.renderedRows = await page.eval(`document.querySelectorAll(".t-row").length`);
  results.atBottom = await page.eval(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight })()`);
  check(results.renderedRows < 60, "list is virtualized");
  check(results.atBottom < 4, "opens at the latest message");
  await page.screenshot(join(shots, "transcript-bottom.png"));

  results.stream = await page.eval(`window.__fixture.stream({ tokens: 6000, batch: 4, intervalMs: 2 })`);
  results.afterStreamDistance = await page.eval(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight })()`);
  check(results.afterStreamDistance < 4, "stays pinned to the bottom while streaming");
  await page.screenshot(join(shots, "transcript-streamed.png"));

  results.scroll = await page.eval(`window.__fixture.scrollTest(900)`);
  results.jumpShown = await page.eval(`!!document.querySelector(".t-jump")`);
  check(results.jumpShown, "jump to latest appears after scrolling up");
  await page.screenshot(join(shots, "transcript-top.png"));

  // Streaming while reading history must not move the reader.
  await page.eval(`(() => { const s = document.querySelector(".t-scroller"); s.dispatchEvent(new WheelEvent("wheel")); s.scrollTop = Math.max(0, s.scrollHeight - s.clientHeight - 4000) })()`);
  await new Promise((r) => setTimeout(r, 300));
  const anchorBefore = await page.eval(`(() => { const r = document.querySelector(".t-row"); return r.dataset.entry + ":" + Math.round(r.getBoundingClientRect().top) })()`);
  results.readingStream = await page.eval(`window.__fixture.stream({ tokens: 1500, batch: 6, intervalMs: 2 })`);
  const [anchorId, anchorTop] = anchorBefore.split(":");
  results.readerDrift = await page.eval(`(() => { const r = document.querySelector('[data-entry="${anchorId}"]'); return r ? Math.round(r.getBoundingClientRect().top) - ${anchorTop} : "unmounted" })()`);
  check(results.readerDrift === 0, "reading position holds while a reply streams below");

  await page.click(".t-jump");
  await page.waitFor(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight < 4 })()`, 5000);
  results.jumpWorks = true;

  // Rail outline.
  await page.press(".t-rail-trigger");
  await page.waitFor(`document.querySelectorAll(".t-rail-row").length >= 2`);
  await page.screenshot(join(shots, "transcript-rail.png"));
  results.railDomRows = await page.eval(`document.querySelectorAll(".t-rail-row").length`);
  check(results.railDomRows < 60, "rail list is windowed");
  results.railOpensOnActive = await page.eval(`!!document.querySelector('.t-rail-row[aria-selected="true"]')`);
  check(results.railOpensOnActive, "rail list opens on the prompt being read");
  await page.eval(`(() => { const l = document.querySelector(".t-rail-list"); l.scrollTop = 0; l.dispatchEvent(new Event("scroll")); })()`);
  await page.waitFor(`!!document.querySelector("#rail-turn-3")`);
  await page.eval(`document.querySelector("#rail-turn-3").click()`);
  results.railTarget = await page.eval(`window.__fixture.state.transcript.messages.findIndex((m, i) => m.role === "user" && window.__fixture.state.transcript.messages.slice(0, i + 1).filter((x) => x.role === "user").length === 4)`);
  await new Promise((r) => setTimeout(r, 1200));
  results.railLanded = await page.eval(`(() => { const rows = [...document.querySelectorAll(".t-row")]; const s = document.querySelector(".t-scroller").getBoundingClientRect(); const top = rows.find((r) => r.getBoundingClientRect().bottom > s.top + 20); return top?.dataset.index })()`);
  check(Number(results.railLanded) === results.railTarget, "rail lands on the picked prompt");

  // Every part kind, in both themes.
  await page.eval(`window.__fixture.kinds()`);
  await page.waitFor(`!!document.querySelector(".t-jump")`);
  await page.click(".t-jump");
  await page.waitFor(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight < 4 })()`, 5000);
  await new Promise((r) => setTimeout(r, 300));
  results.dock = await page.eval(`document.querySelector(".t-dock")?.textContent?.slice(0, 80) ?? null`);
  check(results.dock, "the open question docks above the composer");
  await page.screenshot(join(shots, "transcript-kinds-dark.png"));
  await page.eval(`document.documentElement.dataset.theme = "light"`);
  await new Promise((r) => setTimeout(r, 200));
  await page.screenshot(join(shots, "transcript-kinds-light.png"));
  await page.eval(`document.documentElement.dataset.theme = "dark"`);
  results.openedGroups = await page.eval(`(() => { const heads = [...document.querySelectorAll(".tool-group-head[aria-expanded='false']")].slice(-3); heads.forEach((h) => h.click()); return heads.length })()`);
  await new Promise((r) => setTimeout(r, 200));
  results.openedChips = await page.eval(`(() => { const heads = [...document.querySelectorAll("button.chip-head[aria-expanded='false']")].slice(-4); heads.forEach((h) => h.click()); return heads.length })()`);
  await new Promise((r) => setTimeout(r, 400));
  await page.screenshot(join(shots, "transcript-kinds-open.png"));

  console.log(JSON.stringify(results, null, 2));
  if (page.errors.length) failures.push(`page errors:\n${page.errors.join("\n")}`);
  if (failures.length) throw new Error(`failed: ${failures.join("; ")}`);
  console.log("screenshots in", shots);
} finally {
  await page.close();
  server.close();
  if (!process.env.ZUSE_SHOTS) rmSync(out, { recursive: true, force: true });
}
