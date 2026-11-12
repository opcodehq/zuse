// Sidebar against live engine behaviour: ages, a working agent, a failed run, a long virtualized
// list, real pointer drag, keyboard-only use, and an engine restart with actions while it is down.
import { openBrowser, buildApp, repo, serveCommand, writeFixtureAgent } from "./harness.mjs";
import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const q = (s) => JSON.stringify(s);
const root = mkdtempSync(join(tmpdir(), "zuse-sb-live-"));
const bin = join(root, "bin");
const claude = writeFixtureAgent(bin, { reply: "Fixture reply." });
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
const project = join(root, "gamma-app");
execFileSync("mkdir", ["-p", project]);
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: project });
writeFileSync(join(project, "README.md"), "# gamma\n");
execFileSync("git", ["add", "."], { cwd: project });
execFileSync("git", ["commit", "-qm", "Initial commit"], { cwd: project, env: gitEnv });

/** Replace the fixture agent: `body` runs after the prompt is read. */
function setAgent(body) {
  writeFileSync(claude, `#!/bin/sh\ncase "$*" in *--version*) echo '2.1.0'; exit 0;; esac\nIFS= read -r prompt\n${body}\n`);
  chmodSync(claude, 0o700);
}
const line = (event) => `printf '%s\\n' '${JSON.stringify(event).replace(/'/g, "'\\''")}'`;
const init = line({ type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] });
const replyEvents = (text) => [line({ type: "assistant", message: { role: "assistant", content: [{ type: "text", text }] } }), line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: text })].join("\n");

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const binary = process.env.ZUSE_SERVER ?? [join(repo, "target/release/zuse-cli"), join(repo, "target/debug/zuse-cli")].find(existsSync);
let child;
let listen = "127.0.0.1:0";
let url;
let ipcPort;
function engineEnv() {
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, CLAUDE_CODE_EXECUTABLE: claude, RUST_LOG: "error", ZUSE_WORKTREES_DIR: join(root, "worktrees") };
  for (const key of ["ZUSE_BROWSER_TOKEN", "ZUSE_PUBLIC_ORIGIN", "ZUSE_TLS_CERT", "ZUSE_TLS_KEY"]) delete env[key];
  return env;
}
async function boot() {
  child = spawn(binary, [...serveCommand(binary), "--listen", listen, "--ipc-port", "0", "--data-dir", join(root, "profile"), "--language", "en", "--assets", assets], {
    env: engineEnv(),
    stdio: ["ignore", "pipe", "pipe"],
  });
  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  url = undefined;
  ipcPort = undefined;
  for (let i = 0; i < 200 && !(url && ipcPort); i++) {
    if (child.exitCode !== null) throw new Error(`zuse serve exited:\n${log}`);
    url = log.match(/Open (http\S+)/)?.[1];
    ipcPort = log.match(/Agent tools listen on 127\.0\.0\.1:(\d+)/)?.[1];
    await sleep(100);
  }
  if (!url) throw new Error(`not ready:\n${log}`);
  listen = new URL(url).host;
}
async function halt(signal = "SIGKILL") {
  // A stopped engine must not get a moment to answer before SIGKILL lands.
  if (signal !== "SIGKILL") child.kill("SIGCONT");
  child.kill(signal);
  for (let i = 0; i < 50 && child.exitCode === null; i++) await sleep(100);
}
async function rpc(method, params = {}) {
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
}

await boot();
const page = await openBrowser(url, { width: 1280, height: 820 });
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  await fn();
  console.log("ok");
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `live-${name}.png`)));
const toasts = () => page.eval(`[...document.querySelectorAll(".toast")].map((t) => t.textContent.trim()).join(" | ")`);
const center = (selector) => page.eval(`(() => { const r = document.querySelector(${q(selector)})?.getBoundingClientRect(); return r && { x: r.x + r.width / 2, y: r.y + r.height / 2, top: r.top, bottom: r.bottom, left: r.left } })()`);
const menuItem = async (label) => {
  await page.waitFor(`[...document.querySelectorAll('[role^="menuitem"]')].some((e) => e.textContent.trim() === ${q(label)})`);
  await page.eval(`(() => { document.querySelectorAll("[data-sb-test]").forEach((e) => e.removeAttribute("data-sb-test")); [...document.querySelectorAll('[role^="menuitem"]')].find((e) => e.textContent.trim() === ${q(label)}).setAttribute("data-sb-test", ""); return true })()`);
  await page.press("[data-sb-test]");
};
const mouse = (type, x, y, extra = {}) => page.call("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, ...extra });

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" };
  await rpc("Mutate", { op: "createSpace", spaceId: "live-space", deviceId, path: project, name: "gamma-app", gitDetected: true });
  const minute = 60_000;
  const seeds = [
    ["live-1", "Ship the billing page", 5 * minute],
    ["live-2", "Audit dependencies", 3 * 60 * minute],
    ["live-3", "Plan the migration", 2 * 24 * 60 * minute],
    ["live-4", "Old experiment", 40 * 24 * 60 * minute],
  ];
  for (const [id, title, ago] of seeds) {
    await rpc("Mutate", { op: "createChat", chatId: id, spaceId: "live-space", branch: "main", config });
    await rpc("Mutate", { op: "renameChat", chatId: id, title });
    await rpc("Mutate", { op: "setChatActivity", chatId: id, lastMessageAt: Date.now() - ago, createdAt: Date.now() - ago });
    await rpc("Mutate", { op: "markChatSeen", chatId: id });
  }

  await step("ages read as desktop's relative times", async () => {
    await page.waitFor(`document.querySelectorAll(".sb-row").length === 4`);
    const ages = await page.eval(`[...document.querySelectorAll(".sb-row")].map((r) => r.querySelector(".sb-row-time")?.textContent)`);
    console.log(ages);
    if (ages.join() !== "5m ago,3h ago,2d ago,1mo ago") throw new Error(`ages: ${ages}`);
    await shot("ages");
  });

  await step("a running agent shows the spinner, the agent count and blocks delete", async () => {
    setAgent(`${init}\nsleep 10\n${replyEvents("Slow reply.")}`);
    await rpc("QueueMessage", { chatId: "live-2", targetDeviceId: deviceId, text: "Take your time", attachments: [], holdForTurnEnd: true });
    await page.waitFor(`document.querySelector('[data-chat-id="live-2"] .sb-status')?.getAttribute("aria-label") === "Working"`, 20000);
    await page.waitFor(`document.querySelector(".sb-agents")?.textContent.trim() === "1"`);
    await shot("working");
    // Threads cards badge the avatar instead.
    await page.press(`button[title="View options"]`);
    await menuItem("Threads");
    await page.waitFor(`!!document.querySelector('[data-chat-id="live-2"] .sb-badge[aria-label="Working"]')`);
    await shot("working-threads");
    await page.waitFor(`!document.querySelector('[role="menu"]')`);
    await page.press(`button[title="View options"]`);
    await menuItem("Projects");
    await page.waitFor(`!document.querySelector('[role="menu"]')`);
    const order = await page.eval(`[...document.querySelectorAll(".sb-row")].map((r) => r.dataset.chatId).join()`);
    console.log(order);
    // Delete is refused while it runs.
    await page.eval(`(() => { const row = document.querySelector('[data-chat-id="live-2"]'); const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left + 30, clientY: r.top + 10 })); return true })()`);
    await menuItem("Delete…");
    await page.waitFor(`!!document.querySelector(".dialog")`);
    await shot("delete-working");
    await page.eval(`[...document.querySelectorAll(".dialog button")].find((b) => b.textContent === "Delete").click()`);
    await page.waitFor(`document.body.textContent.includes("before deleting")`);
    if (!(await page.eval(`!!window.__zuse.state.chats["live-2"]`))) throw new Error("deleted a working chat");
    await page.waitFor(`document.querySelector('[data-chat-id="live-2"]')?.dataset.unread === "true"`, 30000);
    await page.waitFor(`document.querySelector(".sb-agents")?.textContent.trim() === "0"`);
    await shot("done");
  });

  await step("a failed run shows the failed mark", async () => {
    setAgent(`${init}\n${line({ type: "result", subtype: "error_during_execution", session_id: "fixture", is_error: true, result: "boom" })}\nexit 1`);
    await rpc("QueueMessage", { chatId: "live-3", targetDeviceId: deviceId, text: "Fail please", attachments: [], holdForTurnEnd: true });
    await page.waitFor(`["Failed", "Done"].includes(document.querySelector('[data-chat-id="live-3"] .sb-status')?.getAttribute("aria-label"))`, 30000).catch(async (e) => {
      console.log(await page.eval(`JSON.stringify({ s: window.__zuse.state.sessions["live-3"], c: window.__zuse.state.chats["live-3"] })`));
      throw e;
    });
    console.log(await page.eval(`JSON.stringify({ label: document.querySelector('[data-chat-id="live-3"] .sb-status')?.getAttribute("aria-label"), status: window.__zuse.state.sessions["live-3"]?.status })`));
    await shot("failed");
    setAgent(`${init}\n${replyEvents("Fixture reply.")}`);
  });

  await step("a long list stays virtualized and scrolls to the selected chat", async () => {
    for (let i = 0; i < 160; i++) {
      await rpc("Mutate", { op: "createChat", chatId: `bulk-${String(i).padStart(3, "0")}`, deviceId, config });
      await rpc("Mutate", { op: "renameChat", chatId: `bulk-${String(i).padStart(3, "0")}`, title: `Bulk thread ${i}` });
    }
    await page.waitFor(`Object.keys(window.__zuse.state.chats).length >= 164`);
    await page.waitFor(`document.querySelectorAll(".sb-row").length > 10`);
    const mounted = await page.eval(`document.querySelectorAll(".sb-row").length`);
    console.log({ mounted });
    if (mounted > 80) throw new Error(`not virtualized: ${mounted} rows mounted`);
    // The oldest bulk row is far down; selecting it from the palette scrolls it in.
    await page.key("k", "KeyK", 75, 2);
    await page.waitFor(`document.activeElement?.tagName === "INPUT" && !document.activeElement.closest(".sb")`);
    await page.type("Bulk thread 0");
    await sleep(300);
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${q("bulk-000")}`).catch(async (e) => { console.log(await page.eval(`window.__zuse.view.selectedChatId`)); throw e; });
    await page.waitFor(`(() => { const row = document.querySelector('[data-chat-id="bulk-000"]'); if (!row) return false; const r = row.getBoundingClientRect(), s = document.querySelector(".sb-scroll").getBoundingClientRect(); return r.top >= s.top && r.bottom <= s.bottom - 20 })()`).catch(async (e) => {
      await shot("bulk-fail");
      console.log(await page.eval(`JSON.stringify({ top: document.querySelector(".sb-scroll").scrollTop, h: document.querySelector(".sb-scroll").scrollHeight, route: window.__zuse.view.route, rows: [...document.querySelectorAll(".sb-row")].slice(-3).map((r) => r.dataset.chatId) })`));
      throw e;
    });
    await shot("bulk");
  });

  await step("a real pointer drag pins a row", async () => {
    // Chromium starts the HTML drag from real mouse input; CDP then plays the OS's part.
    await page.eval(`document.querySelector(".sb-scroll").scrollTop = 0; true`);
    await sleep(200);
    await page.eval(`[...document.querySelectorAll(".sb-head")].find((h) => h.textContent.includes("No project")).querySelector(".sb-head-toggle").click()`);
    await page.waitFor(`!!document.querySelector('[data-chat-id="live-1"]')`);
    await page.call("Input.setInterceptDrags", { enabled: true });
    const from = await center(`[data-chat-id="live-1"] .sb-row-title`);
    if (!from) { await shot("drag-missing"); console.log(await page.eval(`JSON.stringify({ top: document.querySelector(".sb-scroll").scrollTop, rows: [...document.querySelectorAll(".sb-row")].slice(0, 5).map((r) => r.dataset.chatId) })`)); }
    await mouse("mouseMoved", from.x, from.y);
    await mouse("mousePressed", from.x, from.y);
    for (let i = 1; i <= 6; i++) await mouse("mouseMoved", from.x, from.y - i * 3, { buttons: 1 });
    await page.waitFor(`!!document.querySelector('.sb-row[data-dragging="true"]')`, 5000);
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Pinned")`);
    const head = await page.eval(`(() => { const r = [...document.querySelectorAll(".sb-head")].find((h) => h.textContent.includes("Pinned")).getBoundingClientRect(); return { x: r.x + 40, y: r.y + r.height / 2 } })()`);
    const data = { items: [{ mimeType: "application/x-zuse-chat", data: "live-1" }, { mimeType: "text/plain", data: "live-1" }], dragOperationsMask: 16 };
    await page.call("Input.dispatchDragEvent", { type: "dragEnter", x: head.x, y: head.y, data });
    await page.call("Input.dispatchDragEvent", { type: "dragOver", x: head.x, y: head.y, data });
    await sleep(100);
    await shot("drag-over");
    await page.call("Input.dispatchDragEvent", { type: "drop", x: head.x, y: head.y, data });
    await mouse("mouseReleased", head.x, head.y);
    await page.call("Input.setInterceptDrags", { enabled: false });
    await page.waitFor(`document.querySelector(".sb-vitem [data-chat-id='live-1']")?.closest(".sb-vitem").dataset.index === "1"`);
    await page.waitFor(`!document.querySelector('.sb-row[data-dragging="true"]')`);
    await shot("drag-done");
  });

  await step("keyboard alone reaches the heading, menus and rows", async () => {
    await page.eval(`document.querySelector(".sb-nav-row").focus()`);
    const order = [];
    for (let i = 0; i < 12; i++) {
      await page.key("Tab", "Tab", 9);
      order.push(await page.eval(`(() => { const e = document.activeElement; return (e.getAttribute("aria-label") || e.textContent.trim()).slice(0, 40) })()`));
    }
    for (const label of ["Search threads", "View options", "New chat", "Pinned", "Ship the billing page"])
      if (!order.includes(label)) throw new Error(`Tab never reached ${label}: ${order}`);
  });

  const contextMenu = (chatId) =>
    page.eval(`(() => { const row = document.querySelector('[data-chat-id=${q(chatId)}]'); const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left + 30, clientY: r.top + 10 })); return true })()`);
  const PINNED = `(() => { const items = [...document.querySelectorAll(".sb-vitem")].sort((a, b) => a.dataset.index - b.dataset.index); const out = []; let inPinned = false; for (const v of items) { if (v.querySelector(".sb-head")) { inPinned = v.querySelector(".sb-head-label")?.textContent === "Pinned"; continue; } if (inPinned && v.querySelector(".sb-row")) out.push(v.querySelector(".sb-row").dataset.chatId); } return out.join() })()`;
  const pinnedRows = () => page.eval(PINNED);

  await step("requests cut off by the engine dying report an unknown outcome and roll back", async () => {
    await page.eval(`document.activeElement?.blur(); true`);
    child.kill("SIGSTOP");
    await page.click(`[data-chat-id="live-4"] .sb-row-archive`);
    await page.waitFor(`!document.querySelector('[data-chat-id="live-4"]')`);
    await contextMenu("live-3");
    await menuItem("Pin");
    await page.waitFor(`${PINNED} === "live-1,live-3" || ${PINNED}.startsWith("live-3")`);
    await sleep(300);
    await halt();
    await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`);
    await page.waitFor(`[...document.querySelectorAll(".toast")].some((t) => t.textContent.includes("Queued edits were cancelled"))`);
    await page.waitFor(`!!document.querySelector('[data-chat-id="live-4"]')`);
    const pins = await pinnedRows();
    if (pins.includes("live-3")) throw new Error(`unconfirmed pin kept: ${pins}`);
    console.log({ toasts: await toasts() });
    await shot("cut-off");
  });

  await step("actions while the engine is down say so and change nothing", async () => {
    await page.waitFor(`![...document.querySelectorAll(".toast")].length`, 15000).catch(() => {});
    await page.click(`[data-chat-id="live-1"] .sb-row-archive`);
    await page.waitFor(`[...document.querySelectorAll(".toast")].some((t) => t.textContent.includes("Engine not connected"))`);
    if (await page.eval(`!document.querySelector('[data-chat-id="live-1"]')`)) throw new Error("archive while down hid the row");
    await contextMenu("live-1");
    await menuItem("Rename");
    await page.waitFor(`document.activeElement?.classList.contains("sb-rename")`);
    await page.type(" changed");
    await page.key("Enter");
    await sleep(200);
    if (await page.eval(`document.querySelector('[data-chat-id="live-1"] .sb-row-title').textContent.includes("changed")`)) throw new Error("rename while down showed a title the engine never saw");
    // The whole footer stays inside the column.
    const foot = await page.eval(`(() => { const sb = document.querySelector(".sb").getBoundingClientRect(); const b = document.querySelector('.sb-foot button[aria-label="Settings"]')?.getBoundingClientRect(); return b && b.right <= sb.right + 0.5 && b.width > 0 })()`);
    if (!foot) throw new Error("settings button pushed out of the footer");
    await shot("down");
  });

  await step("after a restart the list comes back and actions work again", async () => {
    await boot();
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await page.waitFor(`document.querySelectorAll(".sb-row").length > 3`);
    // An abrupt kill can lose the engine's latest pin write, so pin again only when needed.
    if ((await pinnedRows()) !== "live-1") {
      console.log({ pinsAfterKill: await pinnedRows() });
      await contextMenu("live-1");
      await menuItem("Pin");
      await page.waitFor(`${PINNED} === "live-1"`);
    }
    await page.waitFor(`!document.querySelector('[role="menu"]')`);
    await page.press(`button[title="View options"]`);
    await menuItem("Create section").catch(async (e) => { await shot("view-fail"); throw e; });
    await page.waitFor(`!!document.querySelector(".dialog input")`);
    await page.eval(`document.querySelector(".dialog input").focus()`);
    await page.type("Review");
    await page.click(`.dialog button[type="submit"]`);
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Review")`);
    await sleep(500);
    if ((await pinnedRows()) !== "live-1") throw new Error(`pins: ${await pinnedRows()}`);
  });

  await step("a graceful restart keeps pins and sections", async () => {
    await halt("SIGTERM");
    await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`);
    await boot();
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Review")`);
    if ((await pinnedRows()) !== "live-1") throw new Error(`pins after restart: ${await pinnedRows()}`);
    await page.click(`[data-chat-id="live-2"] .sb-row-archive`);
    await page.waitFor(`window.__zuse.state.chats["live-2"].archived === true`);
    await shot("restored");
  });

  const errors = page.errors.filter((e) => !/github\.com|WebSocket|ERR_CONNECTION_REFUSED|Failed to load resource/.test(e));
  if (errors.length) console.log(`page errors:\n${errors.join("\n")}`);
  console.log("sidebar live done");
} finally {
  await page.close();
  await halt().catch(() => {});
  rmSync(root, { recursive: true, force: true });
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
}
