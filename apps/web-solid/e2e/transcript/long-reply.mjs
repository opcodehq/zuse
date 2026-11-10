// Main-thread cost of streaming one very long markdown reply (headings, prose, rust fences, lists) from a real
// engine, in headless Chromium. Usage: node e2e/transcript/long-reply.mjs [cpuThrottle] [sections]
// LONG_REPLY_BASELINE=1 also runs with settled-block containment switched off, for comparison. LONG_REPLY_TRACE=1 buckets
// style, layout and paint per update by time decile (with invalidation reasons); LONG_REPLY_PROFILE=1 lists hot
// functions; LONG_REPLY_EXTRA_CSS injects a stylesheet before the stream.
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "../harness.mjs";

const throttle = Number(process.argv[2] ?? 4);
const sections = Number(process.argv[3] ?? 400);
const scratch = mkdtempSync(join(tmpdir(), "zuse-long-reply-"));
const project = join(scratch, "demo");
mkdirSync(project, { recursive: true });
mkdirSync(join(scratch, "home"));
process.env.PATH = "/usr/local/bin:/usr/bin:/bin";

let reply = "";
for (let i = 0; i < sections; i++) {
  reply += `## Section ${i}\n\nThis paragraph explains part ${i} of the change, with **bold** words, \`inline code\` and a [link](https://example.com/${i}).\n\n`;
  reply += "```rust\nfn handler_" + i + "(input: &str) -> usize {\n    input.lines().filter(|l| !l.is_empty()).count()\n}\n```\n\n";
  reply += `- first point for ${i}\n- second point for ${i}\n- third point\n\n`;
}
const events = [];
for (let i = 0; i < reply.length; i += 7) events.push(JSON.stringify({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: reply.slice(i, i + 7) } } }));
const data = join(scratch, "events.jsonl");
writeFileSync(data, events.join("\n") + "\n");

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets, env: { HOME: join(scratch, "home") } });
const agent = join(engine.root, "bin", "claude");
const tail = [
  { type: "assistant", message: { role: "assistant", model: "fixture", content: [{ type: "text", text: reply }] } },
  { type: "result", subtype: "success", session_id: "fixture", is_error: false, result: "done" },
];
writeFileSync(agent, `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
while IFS= read -r prompt; do
  case "$prompt" in
    *'"subtype":"initialize"'*)
      id=$(printf '%s' "$prompt" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"commands":[]}}}\\n' "$id"
      [ "$id" = "zuse-command-probe" ] && exit 0;;
    *) break;;
  esac
done
printf '%s\\n' '{"type":"system","subtype":"init","session_id":"fixture","model":"fixture","tools":[]}'
n=0
while IFS= read -r l; do
  printf '%s\\n' "$l"
  n=$((n+1))
  [ $((n % 20)) -eq 0 ] && sleep 0.016
done < '${data}'
${tail.map((e) => `printf '%s\\n' '${JSON.stringify(e).replace(/'/g, "'\\''")}'`).join("\n")}
`);
chmodSync(agent, 0o700);

const page = await openBrowser(engine.url, { width: 1440, height: 900 });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const connected = `window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`;
const results = {};
try {
  await page.waitFor(connected, 30000);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "s", deviceId, path: project, name: "demo", gitDetected: false });
  const variants = process.env.LONG_REPLY_BASELINE ? ["contained", "baseline"] : ["contained"];
  for (const variant of variants) {
    const chatId = `chat-${variant}`;
    await engine.rpc("Mutate", { op: "createChat", chatId, spaceId: "s", config: { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" } });
    await page.waitFor(`!!document.querySelector('[data-chat-id="${chatId}"] .sb-row-open')`);
    await page.click(`[data-chat-id="${chatId}"] .sb-row-open`);
    await page.waitFor(`window.__zuse.view.selectedChatId === "${chatId}"`);
    await page.eval(`(() => {
      document.getElementById("bench-style")?.remove();
      if (${!!process.env.LONG_REPLY_EXTRA_CSS}) { const x = document.createElement("style"); x.textContent = ${JSON.stringify(process.env.LONG_REPLY_EXTRA_CSS ?? "")}; document.head.appendChild(x); }
      if (${variant === "baseline"}) {
        const s = document.createElement("style");
        s.id = "bench-style";
        s.textContent = ".t-root .md-live > * { content-visibility: visible !important; contain-intrinsic-size: none !important; }";
        document.head.appendChild(s);
      }
      return true;
    })()`);
    await page.call("Emulation.setCPUThrottlingRate", { rate: throttle });
    await page.eval(`(() => {
      window.__bench = { frames: [], long: [], last: 0, stop: false };
      new PerformanceObserver((l) => l.getEntries().forEach((e) => window.__bench.long.push(e.duration))).observe({ type: "long-animation-frame" });
      const tick = (t) => { const b = window.__bench; if (b.last) b.frames.push(t - b.last); b.last = t; if (!b.stop) requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
      return true;
    })()`);
    await engine.rpc("QueueCommand", {
      chatId,
      targetDeviceId: deviceId,
      command: { kind: "run", messageId: crypto.randomUUID(), request: { prompt: "long reply", harness: "claude-code", cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null, attachments: [] } },
    });
    const started = Date.now();
    await page.waitFor(`window.__zuse.state.sessions["${chatId}"]?.status === "working"`, 20000);
    // Sample a 20 s window of the stream.
    if (process.env.LONG_REPLY_PROFILE) {
      await page.call("Profiler.enable");
      await page.call("Profiler.start");
    }
    const trace = [];
    let traced;
    if (process.env.LONG_REPLY_TRACE) {
      const done = new Promise((r) => (traced = r));
      page.on("Tracing.dataCollected", (p) => trace.push(...p.value));
      page.on("Tracing.tracingComplete", () => traced());
      traced.done = done;
      await page.call("Tracing.start", { categories: "devtools.timeline,disabled-by-default-devtools.timeline.invalidationTracking", transferMode: "ReportEvents" });
    }
    await sleep(20000);
    if (process.env.LONG_REPLY_TRACE) {
      const done = traced.done;
      await page.call("Tracing.end");
      await done;
      const names = { UpdateLayoutTree: "style", Layout: "layout", Paint: "paint", PrePaint: "prepaint", FireAnimationFrame: "raf", FunctionCall: "js", TimerFire: "timer", EventDispatch: "event", RunMicrotasks: "micro", HitTest: "hit", Layerize: "layerize", Commit: "commit", ParseHTML: "parse" };
      const totals = {};
      for (const e of trace) if (names[e.name] && e.ph === "X" && e.dur) totals[names[e.name]] = (totals[names[e.name]] ?? 0) + e.dur / 1000;
      const tasks = trace.filter((e) => e.name === "RunTask" && e.ph === "X" && e.dur).reduce((a, e) => a + e.dur / 1000, 0);
      console.log("totals ms", JSON.stringify(Object.fromEntries(Object.entries(totals).map(([k, v]) => [k, Math.round(v)]))), "RunTask", Math.round(tasks));
      const events = trace.filter((e) => names[e.name] && e.ph === "X" && e.dur);
      const t0 = Math.min(...events.map((e) => e.ts));
      const t1 = Math.max(...events.map((e) => e.ts));
      const deciles = Array.from({ length: 10 }, () => ({}));
      for (const e of events) {
        const d = deciles[Math.min(9, Math.floor(((e.ts - t0) / (t1 - t0)) * 10))];
        const k = names[e.name];
        d[k] ??= [0, 0];
        d[k][0] += e.dur / 1000;
        d[k][1]++;
      }
      const styles = events.filter((e) => e.name === "UpdateLayoutTree");
      const inval = trace.filter((e) => /Invalidation|ScheduleStyle/.test(e.name)).slice(-4000);
      const reasons = new Map();
      for (const e of inval) { const k = e.name + " " + (e.args?.data?.reason ?? e.args?.data?.invalidationList?.[0]?.classes ?? "") + " " + (e.args?.data?.nodeName ?? ""); reasons.set(k, (reasons.get(k) ?? 0) + 1); }
      console.log([...reasons].sort((a, b) => b[1] - a[1]).slice(0, 15).map(([k, n]) => n + " " + k).join("\n"));
      const counts = Array.from({ length: 10 }, () => [0, 0]);
      for (const e of styles) { const c = counts[Math.min(9, Math.floor(((e.ts - t0) / (t1 - t0)) * 10))]; c[0] += e.args?.elementCount ?? 0; c[1]++; }
      console.log("restyled elements per recalc by decile", counts.map(([n, k]) => (k ? Math.round(n / k) : 0)).join(" "));
      for (const [i, d] of deciles.entries()) console.log(`decile ${i}: ` + Object.entries(d).map(([k, [ms, n]]) => `${k} ${(ms / n).toFixed(2)}ms x${n}`).join(", "));
    }
    if (process.env.LONG_REPLY_PROFILE) {
      const { result } = await page.call("Profiler.stop");
      const self = new Map();
      const byId = new Map(result.profile.nodes.map((n) => [n.id, n]));
      const dt = result.profile.timeDeltas;
      result.profile.samples.forEach((id, i) => {
        const n = byId.get(id);
        const key = `${n.callFrame.functionName || "(anon)"} ${n.callFrame.url.split("/").pop()}:${n.callFrame.lineNumber}`;
        self.set(key, (self.get(key) ?? 0) + (dt[i] ?? 0) / 1000);
      });
      console.log([...self].sort((a, b) => b[1] - a[1]).slice(0, 25).map(([k, v]) => `${v.toFixed(0)}ms ${k}`).join("\n"));
    }
    const r = await page.eval(`(() => {
      const b = window.__bench; b.stop = true;
      const f = [...b.frames].sort((x, y) => x - y);
      const chars = window.__zuse.state.transcript.messages.at(-1)?.parts.filter((p) => p.kind === "text").reduce((n, p) => n + p.text.length, 0) ?? 0;
      return { frames: f.length, p95: Math.round(f[Math.floor(f.length * 0.95)] ?? 0), max: Math.round(f.at(-1) ?? 0), longTasks: b.long.length, longSum: Math.round(b.long.reduce((a, x) => a + x, 0)), chars, nodes: document.querySelectorAll("*").length };
    })()`);
    await page.call("Emulation.setCPUThrottlingRate", { rate: 1 });
    await page.waitFor(`window.__zuse.state.sessions["${chatId}"]?.status !== "working"`, 300000);
    const shown = await page.eval(`document.querySelectorAll('.t-entry[data-role="assistant"] .md h2').length`);
    results[variant] = { ...r, seconds: Math.round((Date.now() - started) / 1000), headingsAfter: shown };
    console.log(variant, JSON.stringify(results[variant]));
  }
} finally {
  if (page.errors.length) console.log("page errors:", page.errors.join("\n"));
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(scratch, { recursive: true, force: true });
}
