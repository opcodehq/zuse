// Disposable engine plus headless Chromium for end-to-end checks. Never touches real profiles.
// Requires Node 22+. Usage: import { startEngine, openBrowser, buildApp } from "./harness.mjs".
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, chmodSync, rmSync, existsSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const repo = join(here, "../../..");
export const CHROME = process.env.ZUSE_CHROME ?? `${process.env.HOME}/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Build the app into a private directory so parallel runs never share output. */
export function buildApp(outDir = mkdtempSync(join(tmpdir(), "zuse-solid-"))) {
  // Run Vite with this Node: tests may narrow PATH, and the shim would then find an older system node.
  execFileSync(process.execPath, [join(here, "../node_modules/vite/bin/vite.js"), "build", "--logLevel", "error"], {
    cwd: join(here, ".."),
    env: { ...process.env, ZUSE_WEB_OUT: outDir },
    stdio: "inherit",
  });
  return outDir;
}

/** A fixture agent CLI: answers every prompt with `reply` streamed in `chunks` pieces. No paid calls. */
export function writeFixtureAgent(dir, { reply = "Fixture reply.", chunks = 1 } = {}) {
  mkdirSync(dir, { recursive: true });
  const parts = [];
  const size = Math.max(1, Math.ceil(reply.length / chunks));
  for (let i = 0; i < reply.length; i += size) parts.push(reply.slice(i, i + size));
  const events = [
    { type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] },
    ...parts.map((text) => ({ type: "assistant", message: { role: "assistant", content: [{ type: "text", text }] } })),
    { type: "result", subtype: "success", session_id: "fixture", is_error: false, result: reply },
  ];
  const script = `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
IFS= read -r prompt
${events.map((e) => `printf '%s\\n' '${JSON.stringify(e).replace(/'/g, "'\\''")}'`).join("\n")}
`;
  const path = join(dir, "claude");
  writeFileSync(path, script);
  chmodSync(path, 0o700);
  return path;
}

/** Start zuse-server on ephemeral ports with a fresh profile. */
/** `prepare(profileDir)` seeds the fresh profile before boot; `env` adds engine environment. */
export async function startEngine({ assets, server, extraArgs = [], agent = {}, prepare, env: extraEnv = {} } = {}) {
  const root = mkdtempSync(join(tmpdir(), "zuse-e2e-"));
  const bin = join(root, "bin");
  const claude = writeFixtureAgent(bin, agent);
  const binary = server ?? process.env.ZUSE_SERVER ?? [join(repo, "target/release/zuse-server"), join(repo, "target/debug/zuse-server")].find(existsSync);
  if (!binary) throw new Error("Build zuse-server first (cargo build -p zuse-cli --bin zuse-server)");
  if (prepare) { mkdirSync(join(root, "profile"), { recursive: true }); prepare(join(root, "profile")); }
  const child = spawn(binary, ["--listen", "127.0.0.1:0", "--ipc-port", "0", "--data-dir", join(root, "profile"), "--language", "en", ...(assets ? ["--assets", assets] : []), ...extraArgs], {
    env: (({ ZUSE_BROWSER_TOKEN, ZUSE_PUBLIC_ORIGIN, ZUSE_TLS_CERT, ZUSE_TLS_KEY, ...env }) => ({ ...env, PATH: `${bin}:${process.env.PATH}`, CLAUDE_CODE_EXECUTABLE: claude, RUST_LOG: "error", ...extraEnv }))(process.env),
    stdio: ["ignore", "pipe", "pipe"],
  });
  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  let url, ipcPort;
  for (let i = 0; i < 200 && !(url && ipcPort); i++) {
    if (child.exitCode !== null) throw new Error(`zuse-server exited:\n${log}`);
    url = log.match(/Open (http\S+)/)?.[1];
    ipcPort = log.match(/Agent tools listen on 127\.0\.0\.1:(\d+)/)?.[1];
    await sleep(100);
  }
  if (!url) throw new Error(`zuse-server did not become ready:\n${log}`);
  return {
    root,
    url,
    origin: new URL(url).origin,
    ipcPort: Number(ipcPort),
    log: () => log,
    /** One RPC call on the engine's loopback IPC port. */
    async rpc(method, params = {}) {
      const ws = new WebSocket(`ws://127.0.0.1:${ipcPort}`);
      await new Promise((resolve, reject) => ((ws.onopen = resolve), (ws.onerror = reject)));
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`${method} timed out`)), 15000);
        ws.onmessage = (event) => {
          const frame = JSON.parse(event.data);
          if (frame.id !== 1) return;
          clearTimeout(timer);
          ws.close();
          if (frame.err) reject(new Error(frame.err));
          else resolve(frame.ok ?? frame.item);
        };
        ws.send(JSON.stringify({ id: 1, method, params }));
      });
    },
    async close() {
      if (child.exitCode === null) {
        child.kill("SIGTERM");
        for (let i = 0; i < 50 && child.exitCode === null; i++) await sleep(100);
        if (child.exitCode === null) child.kill("SIGKILL");
      }
      rmSync(root, { recursive: true, force: true });
    },
  };
}

/** Headless Chromium driven over CDP. The app is DOM, so screenshots and queries work headless. */
export async function openBrowser(url, { width = 1440, height = 900, port = 29000 + Math.floor(Math.random() * 900), args = [] } = {}) {
  const profile = mkdtempSync(join(tmpdir(), "zuse-chrome-"));
  const chrome = spawn(CHROME, ["--headless=new", "--no-sandbox", `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--no-proxy-server", "--no-first-run", `--window-size=${width},${height}`, ...args, "about:blank"], { stdio: "ignore", detached: true });
  let target;
  for (let i = 0; i < 60 && !target; i++) {
    await sleep(200);
    try { target = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find((t) => t.type === "page"); } catch {}
  }
  if (!target) throw new Error("Chromium did not start");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  let id = 0;
  const waiting = new Map();
  const errors = [];
  const listeners = new Map();
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.method) listeners.get(d.method)?.forEach((fn) => fn(d.params));
    if (d.id && waiting.has(d.id)) { waiting.get(d.id)(d); waiting.delete(d.id); }
    if (d.method === "Runtime.exceptionThrown") errors.push(d.params.exceptionDetails.exception?.description ?? d.params.exceptionDetails.text);
    if (d.method === "Runtime.consoleAPICalled" && d.params.type === "error") errors.push(d.params.args.map((a) => a.value ?? a.description).join(" "));
  };
  const call = (method, params = {}) => new Promise((resolve) => { const i = ++id; waiting.set(i, resolve); ws.send(JSON.stringify({ id: i, method, params })); });
  await call("Page.enable");
  await call("Runtime.enable");
  await call("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  const page = {
    errors,
    call,
    /** Listen for a CDP event. */
    on(method, fn) { listeners.set(method, [...(listeners.get(method) ?? []), fn]); },
    async goto(next) { await call("Page.navigate", { url: next }); await sleep(300); },
    async eval(expression) {
      const r = await call("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (r.result?.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description ?? "evaluation failed");
      return r.result?.result?.value;
    },
    async waitFor(expression, timeout = 15000) {
      for (const end = Date.now() + timeout; Date.now() < end; await sleep(100)) {
        try { if (await page.eval(expression)) return; } catch {}
      }
      throw new Error(`Timed out waiting for ${expression}`);
    },
    async click(selector) { await page.eval(`(() => { const e = document.querySelector(${JSON.stringify(selector)}); if (!e) throw new Error("missing ${selector.replace(/"/g, "")}"); e.click(); return true })()`); },
    /** Real pointer click at an element's centre, for menus that listen to pointer events. */
    async press(selector) {
      const box = await page.eval(`(() => { const r = document.querySelector(${JSON.stringify(selector)})?.getBoundingClientRect(); return r && { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
      if (!box) throw new Error(`missing ${selector}`);
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await call("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 });
    },
    async type(text) { await call("Input.insertText", { text }); },
    async key(key, code = key, keyCode = 13, modifiers = 0) {
      for (const type of ["keyDown", "keyUp"]) await call("Input.dispatchKeyEvent", { type, key, code, windowsVirtualKeyCode: keyCode, modifiers });
    },
    async screenshot(path) { const r = await call("Page.captureScreenshot", { format: "png" }); writeFileSync(path, Buffer.from(r.result.data, "base64")); return path; },
    async close() { ws.close(); try { process.kill(-chrome.pid); } catch {} await sleep(200); rmSync(profile, { recursive: true, force: true }); },
  };
  await page.goto(url);
  return page;
}
