// Streaming cost of one long markdown block (list, table, paragraph) and of a large settled code fence, in
// headless Chromium. Usage: node e2e/transcript/stream-bench.mjs [cpuThrottle]; BENCH_RUNS=list:1000,table:600 or
// BENCH_CODE=300,1500 picks the cases.
import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { openBrowser } from "../harness.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const app = join(here, "../..");
const out = mkdtempSync(join(tmpdir(), "zuse-stream-bench-"));
execFileSync(join(app, "node_modules/.bin/vite"), ["build", "--config", join(here, "vite.config.mjs"), "--logLevel", "error"], { cwd: app, env: { ...process.env, ZUSE_FIXTURE_OUT: out }, stdio: "inherit" });
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".wasm": "application/wasm" };
const server = createServer((req, res) => {
  const path = join(out, decodeURIComponent(new URL(req.url, "http://x").pathname));
  if (!existsSync(path) || path.endsWith("/")) return res.writeHead(404).end();
  res.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" }).end(readFileSync(path));
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const throttle = Number(process.argv[2] ?? 1);
const page = await openBrowser(`http://127.0.0.1:${server.address().port}/fixture.html?theme=dark`, { width: 1280, height: 860 });

const STREAM = `async (kind, n) => {
  const f = window.__fixture;
  const entry = "bench-" + Math.random(), part = "bp-" + Math.random();
  f.upsert({ id: entry, role: "assistant", createdAt: Date.now(), deviceId: "fixture-device", status: "streaming", parts: [{ kind: "text", id: part, text: "" }] });
  f.setState("sessions", "fixture-chat", { chatId: "fixture-chat", status: "working", startedAt: new Date().toISOString() });
  let len = 0;
  const send = (text) => { len += text.length; f.apply({ append: [{ entry, part, text, len }], count: f.count() }); };
  const raf = () => new Promise((r) => requestAnimationFrame(r));
  const loaf = [];
  const obs = new PerformanceObserver((l) => l.getEntries().forEach((e) => loaf.push(e.duration)));
  obs.observe({ type: "long-animation-frame" });
  if (kind === "table") send("| Name | Value | Notes |\\n| --- | --- | --- |\\n");
  let replaced = 0, samples = 0, watched = null, nextSample = performance.now() + 100;
  const started = performance.now();
  for (let i = 0; i < n; i++) {
    if (kind === "list") send("- item " + i + " with **some** words and \`code\` here\\n");
    else if (kind === "table") send("| row " + i + " | " + i * 3 + " | a note with *emphasis* |\\n");
    else send("Sentence number " + i + " keeps the paragraph growing with plain words. ");
    await raf(); await raf();
    const now = performance.now();
    if (now >= nextSample && kind !== "para") {
      if (watched) { samples++; if (!watched.isConnected) replaced++; }
      const rows = document.querySelectorAll(kind === "list" ? ".md li" : ".md tbody tr");
      watched = rows[rows.length - 2] ?? null;
      nextSample = now + 100;
    }
  }
  const elapsed = performance.now() - started;
  obs.disconnect();
  f.setState("sessions", "fixture-chat", { chatId: "fixture-chat", status: "idle" });
  const tail = loaf.slice(-Math.ceil(loaf.length / 4));
  const mean = (a) => (a.length ? Math.round(a.reduce((x, y) => x + y, 0) / a.length) : 0);
  return { kind, n, kb: Math.round(len / 1024), msPerUpdate: Math.round(elapsed / n), longFrames: loaf.length, meanLongFrame: mean(loaf), meanLastQuarter: mean(tail), maxLongFrame: Math.round(Math.max(0, ...loaf)), replaced: samples ? replaced + "/" + samples : "n/a" };
}`;

const CODE = `async (n) => {
  const f = window.__fixture;
  const lines = Array.from({ length: n }, (_, i) => i % 7 === 0 ? "// step " + i + " explains the next call" : "export function handler" + i + "(input: string, count = " + i + "): Promise<number> { return Promise.resolve(input.length + count); }");
  const long = [];
  const obs = new PerformanceObserver((l) => l.getEntries().forEach((e) => long.push(Math.round(e.duration))));
  obs.observe({ type: "longtask" });
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  f.inject({ role: "assistant", status: "complete", parts: [{ kind: "text", id: "code-" + Math.random(), text: "Here it is:\\n\\n\`\`\`ts\\n" + lines.join("\\n") + "\\n\`\`\`\\n" }] });
  const s = document.querySelector(".t-scroller");
  s.scrollTop = s.scrollHeight;
  const spans = () => document.querySelectorAll(".code-body span").length;
  for (let i = 0; i < 100 && !spans(); i++) await wait(100);
  await wait(1500);
  const settle = long.splice(0);
  const settledSpans = spans();
  const body = [...document.querySelectorAll(".code-body")].at(-1);
  const height = Math.round(body.getBoundingClientRect().height) - 20;
  let copied = null;
  const runs = body.querySelectorAll(".code-run");
  if (runs.length > 2) {
    const range = document.createRange();
    range.setStart(runs[0], 0);
    range.setEnd(runs[2], runs[2].childNodes.length);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    const want = lines.slice(0, 150).join("\\n") + "\\n";
    const got = getSelection().toString().replace(/\\n$/, "");
    copied = got === want.replace(/\\n$/, "") ? "exact" : "differs: " + JSON.stringify(getSelection().toString().slice(-80)) + " vs " + JSON.stringify(want.slice(-80));
    getSelection().removeAllRanges();
  }
  const nodes = document.querySelectorAll("*").length;
  s.dispatchEvent(new WheelEvent("wheel", { deltaY: -1 }));
  s.scrollTop = 0;
  await wait(800);
  long.splice(0);
  const spansAway = spans();
  s.scrollTop = s.scrollHeight;
  await wait(1500);
  const back = long.splice(0);
  obs.disconnect();
  return { lines: n, height, expectedHeight: n * 18, copied, settleLongTasks: settle, spans: settledSpans, spansWhileAway: spansAway, spansBack: spans(), domNodes: nodes, domNodesBack: document.querySelectorAll("*").length, scrollBackLongTasks: back };
}`;

try {
  await page.waitFor(`!!window.__fixture`);
  if (process.env.BENCH_CODE) {
    await page.eval(`window.__fixture.seed(40)`);
    await page.waitFor(`document.querySelectorAll(".t-row").length > 0`);
    if (throttle > 1) await page.call("Emulation.setCPUThrottlingRate", { rate: throttle });
    if (process.env.BENCH_PROFILE) {
      await page.call("Profiler.enable");
      await page.call("Profiler.start");
    }
    for (const n of process.env.BENCH_CODE.split(",")) console.log(JSON.stringify(await page.eval(`(${CODE})(${Number(n)})`)));
    if (process.env.BENCH_PROFILE) {
      const { result } = await page.call("Profiler.stop");
      const self = new Map();
      const dt = result.profile.timeDeltas;
      result.profile.samples.forEach((id, i) => self.set(id, (self.get(id) ?? 0) + (dt[i] ?? 0)));
      const byName = new Map();
      for (const node of result.profile.nodes) {
        const key = `${node.callFrame.functionName || "(anon)"} ${node.callFrame.url.split("/").pop()}:${node.callFrame.lineNumber}`;
        byName.set(key, (byName.get(key) ?? 0) + (self.get(node.id) ?? 0));
      }
      console.log([...byName].sort((a, b) => b[1] - a[1]).slice(0, 15).map(([k, v]) => `${Math.round(v / 1000)}ms ${k}`).join("\n"));
    }
  } else {
    await page.eval(`window.__fixture.seed(20)`);
    await page.waitFor(`document.querySelectorAll(".t-row").length > 0`);
    if (throttle > 1) await page.call("Emulation.setCPUThrottlingRate", { rate: throttle });
    const runs = (process.env.BENCH_RUNS ?? (throttle > 1 ? "list:200,list:400,table:200,para:300" : "list:1000,table:600,para:1500")).split(",");
    for (const run of runs) {
      const [kind, n] = run.split(":");
      console.log(JSON.stringify(await page.eval(`(${STREAM})(${JSON.stringify(kind)}, ${Number(n)})`)));
    }
  }
  if (page.errors.length) throw new Error(page.errors.join("\n"));
} finally {
  await page.close();
  server.close();
  rmSync(out, { recursive: true, force: true });
}
