// A whole user journey through the production build against a disposable engine and a scripted agent:
// open the link, add a project, chat with streaming and tools, switch chats, open terminal, files and
// settings, reload, restart the engine. Also measures first load and a long streamed reply in a long chat.
// Usage: node e2e/journey.mjs [--assets dir]   ZUSE_SHOTS=dir keeps screenshots.
import { execFileSync, spawn } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "./harness.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const argAssets = process.argv.indexOf("--assets");
const ownAssets = argAssets < 0;
const assets = ownAssets ? buildApp() : process.argv[argAssets + 1];
const scratch = mkdtempSync(join(tmpdir(), "zuse-journey-"));
const home = join(scratch, "home");
const project = join(home, "code", "journey-app");
mkdirSync(join(project, "src"), { recursive: true });
writeFileSync(join(project, "README.md"), "# Journey\n\nversion = 1\n");
writeFileSync(join(project, "src", "main.ts"), "export const answer = 42;\n");
writeFileSync(join(project, "zuse.json"), JSON.stringify({ actions: [{ name: "Say hi", command: "echo action-ran-ok", icon: "play" }] }));
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: project });
execFileSync("git", ["-c", "user.name=Journey", "-c", "user.email=journey@example.invalid", "add", "."], { cwd: project });
execFileSync("git", ["-c", "user.name=Journey", "-c", "user.email=journey@example.invalid", "commit", "-qm", "Initial"], { cwd: project });
const shots = process.env.ZUSE_SHOTS ?? join(scratch, "shots");
mkdirSync(shots, { recursive: true });

// --- Scripted agent -------------------------------------------------------------------------------
const line = (event) => JSON.stringify(event);
const delta = (type, key, text) => ({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type, [key]: text } } });
const start = (block) => ({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: block } });
const init = { type: "system", subtype: "init", session_id: "journey", model: "fixture", tools: [] };
const done = (text) => [
  { type: "assistant", message: { role: "assistant", model: "fixture", content: [{ type: "text", text }], usage: { input_tokens: 4000, output_tokens: 50 } } },
  { type: "result", subtype: "success", session_id: "journey", is_error: false, result: text, usage: { input_tokens: 1, output_tokens: 1 }, modelUsage: { fixture: { contextWindow: 200000 } } },
];
const textEvents = (text, size = 6) => [start({ type: "text", text: "" }), ...text.match(new RegExp(`[\\s\\S]{1,${size}}`, "g")).map((t) => delta("text_delta", "text", t))];
const toolTurn = (id, name, input, output) => [
  { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id, name, input }] } },
  { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content: output }] } },
];

const toolsReply = "## Done\n\nI read the project and bumped the version.\n\n- `README.md`: version 1 → 2\n- `src/main.ts`: unchanged\n\n```ts\nexport const answer = 42;\n```\n\nAnything else?";
const toolsScript = [
  init,
  start({ type: "thinking", thinking: "" }),
  ...["Let me look ", "at the project ", "first."].map((t) => delta("thinking_delta", "thinking", t)),
  { type: "assistant", message: { role: "assistant", content: [{ type: "thinking", thinking: "Let me look at the project first.", signature: "x" }] } },
  ...toolTurn("t1", "Bash", { command: "ls -la" }, "total 8\n-rw-r--r-- 1 user user 30 README.md\ndrwxr-xr-x 2 user user 60 src"),
  ...toolTurn("t2", "Read", { file_path: join(project, "README.md") }, "# Journey\n\nversion = 1\n"),
  ...toolTurn("t3", "Edit", { file_path: join(project, "README.md"), old_string: "version = 1", new_string: "version = 2" }, "The file was updated."),
  ...textEvents(toolsReply),
  ...done(toolsReply),
];
// A seed turn: a realistic block of markdown with a code fence and two tool calls, played without pauses.
const seedReply = (n) => `### Step ${n}\n\nChecked the module and its callers. The change keeps the public API.\n\n${"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(6)}\n\n\`\`\`ts\nexport function step${n}(input: number): number {\n  return input * ${n} + 1;\n}\n\`\`\`\n\n1. First point\n2. Second point with \`inline code\`\n`;
const seedScript = (n) => [init, ...toolTurn(`s${n}a`, "Grep", { pattern: `step${n}` }, `src/main.ts:${n}: step${n}`), ...toolTurn(`s${n}b`, "Read", { file_path: join(project, "src/main.ts") }, "export const answer = 42;\n"), ...textEvents(seedReply(n), 40), ...done(seedReply(n))];
// The long reply: ~3000 deltas of a few characters, paragraphs, lists and code fences, as a fast model streams.
const longParts = [];
for (let i = 0; i < 60; i++) {
  longParts.push(`#### Section ${i}\n\n${"The quick brown fox jumps over the lazy dog while the stream keeps flowing. ".repeat(3)}\n\n`);
  if (i % 3 === 0) longParts.push(`\`\`\`rust\nfn section_${i}(value: u64) -> u64 {\n    value.wrapping_mul(${i + 3}).rotate_left(7)\n}\n\`\`\`\n\n`);
  if (i % 4 === 1) longParts.push(`- item one for ${i}\n- item two with **bold** text\n- item three\n\n`);
}
const longReply = longParts.join("");
const longEvents = [init, ...textEvents(longReply, 7), ...done(longReply)];
const shortReply = "Second chat reply.";

const scriptDir = join(scratch, "agent");
mkdirSync(scriptDir, { recursive: true });
const writeLines = (name, events) => writeFileSync(join(scriptDir, name), events.map(line).join("\n") + "\n");
writeLines("tools.jsonl", toolsScript);
writeLines("long.jsonl", longEvents);
writeLines("short.jsonl", [init, ...textEvents(shortReply), ...done(shortReply)]);
for (let i = 1; i <= 40; i++) writeLines(`seed${i}.jsonl`, seedScript(i));
writeFileSync(join(scriptDir, "commands.json"), JSON.stringify({ type: "control_response", response: { subtype: "success", request_id: "@ID@", response: { commands: [] } } }));
// play FILE EVERY PAUSE: print lines, pausing PAUSE seconds after every EVERY lines.
const agentScript = `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
dir='${scriptDir}'
while IFS= read -r prompt; do
  case "$prompt" in
    *'"subtype":"initialize"'*)
      id=$(printf '%s' "$prompt" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
      sed "s/@ID@/$id/" "$dir/commands.json"; echo
      [ "$id" = "zuse-command-probe" ] && exit 0;;
    *'"type":"user"'*) break;;
  esac
done
play() { i=0; while IFS= read -r l; do printf '%s\\n' "$l"; i=$((i+1)); if [ $((i % $2)) -eq 0 ]; then sleep $3; fi; done < "$dir/$1"; }
case "$prompt" in
  *journey-tools*) play tools.jsonl 1 0.04;;
  *journey-long*) play long.jsonl 20 0.016;;
  *journey-seed-*) n=$(printf '%s' "$prompt" | sed -n 's/.*journey-seed-\\([0-9]*\\).*/\\1/p'); play "seed$n.jsonl" 1000 0;;
  *) play short.jsonl 1 0.03;;
esac
`;

// --- Engine ---------------------------------------------------------------------------------------
const engine = await startEngine({ assets, env: { HOME: home } });
const agent = join(engine.root, "bin", "claude");
writeFileSync(agent, agentScript);
chmodSync(agent, 0o700);

let respawned = null;
async function restartEngine(downFor = async () => {}) {
  const port = new URL(engine.url).port;
  const pid = execFileSync("pgrep", ["-f", `data-dir ${engine.root}/profile`]).toString().trim().split("\n")[0];
  const argv = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0").filter(Boolean);
  const env = Object.fromEntries(readFileSync(`/proc/${pid}/environ`, "utf8").split("\0").filter(Boolean).map((kv) => [kv.slice(0, kv.indexOf("=")), kv.slice(kv.indexOf("=") + 1)]));
  process.kill(Number(pid), "SIGTERM");
  for (let i = 0; i < 80; i++) { try { process.kill(Number(pid), 0); await sleep(100); } catch { break; } }
  await downFor();
  const args = argv.slice(1).map((a, i, all) => (all[i - 1] === "--listen" ? `127.0.0.1:${port}` : all[i - 1] === "--ipc-port" ? String(engine.ipcPort) : a));
  respawned = spawn(argv[0], args, { env, stdio: ["ignore", "pipe", "pipe"] });
  let log = "";
  respawned.stdout.on("data", (d) => (log += d));
  respawned.stderr.on("data", (d) => (log += d));
  for (let i = 0; i < 150 && !/Open http/.test(log); i++) await sleep(100);
  if (!/Open http/.test(log)) throw new Error(`engine did not restart:\n${log}`);
}

// --- Browser --------------------------------------------------------------------------------------
// First-load marks recorded by the page itself, so CDP polling latency never inflates them.
const marks = `(() => {
  const m = (window.__journeyMarks = {});
  const tick = () => {
    const z = window.__zuse;
    if (!m.sidebar && document.querySelector(".app-sidebar .sb-nav")) m.sidebar = performance.now();
    if (!m.connected && z?.state.connection.kind === "connected") m.connected = performance.now();
    if (!m.synced && z?.state.workspaceSynced) m.synced = performance.now();
    if (!m.composer && document.querySelector(".cmp-input, .cmp-onboarding")) m.composer = performance.now();
    if (!(m.sidebar && m.synced && m.composer)) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})()`;
const page = await openBrowser("about:blank", { width: 1440, height: 900 });
await page.call("Page.addScriptToEvaluateOnNewDocument", { source: marks });
await page.call("Network.enable");

const failures = [];
const results = {};
const check = (cond, what) => { if (!cond) failures.push(what); };
const step = async (name, fn) => {
  const t0 = Date.now();
  try {
    await fn();
    console.log(`ok   ${name} (${Date.now() - t0} ms)`);
  } catch (error) {
    failures.push(`${name}: ${error.message}`);
    console.log(`FAIL ${name}: ${error.message}`);
    console.log("  state:", await page.eval(`JSON.stringify({ sel: window.__zuse.view.selectedChatId, route: window.__zuse.view.route.kind, panel: window.__zuse.view.rightPanel, tr: window.__zuse.state.transcript?.chatId, ready: window.__zuse.state.transcript?.ready, n: window.__zuse.state.transcript?.messages.length, conn: window.__zuse.state.connection.kind, sessions: Object.fromEntries(Object.entries(window.__zuse.state.sessions).map(([k, v]) => [k.slice(0, 8), v.status])), users: [...document.querySelectorAll(".t-entry[data-role='user']")].map((e) => e.textContent.slice(0, 24)) })`).catch((e) => e.message));
    await page.screenshot(join(shots, `fail-${name.replace(/\W+/g, "-")}.png`)).catch(() => {});
  }
};
const q = (sel) => `document.querySelector(${JSON.stringify(sel)})`;
const CTRL = 2;
const enter = () => page.key("Enter", "Enter", 13);
const typeInComposer = async (text) => {
  await page.waitFor(`!!${q(".cmp-input")}`);
  await page.eval(`(${q(".cmp-input")}.focus(), true)`);
  await page.type(text);
};
const chatOf = (prompt) => `Object.values(window.__zuse.state.chats).find((c) => (c.title ?? "").includes(${JSON.stringify(prompt)}) || c.lastMessagePreview?.includes(${JSON.stringify(prompt)}))`;
const working = (id) => `["working", "awaitingInput"].includes(window.__zuse.state.sessions[${JSON.stringify(id)}]?.status)`;
const selected = () => page.eval(`window.__zuse.view.selectedChatId`);
const firstLoad = async (label) => {
  const t = await page.eval(`(() => {
    const nav = performance.getEntriesByType("navigation")[0];
    const fcp = performance.getEntriesByName("first-contentful-paint")[0]?.startTime ?? null;
    const res = performance.getEntriesByType("resource");
    const sum = (k) => res.reduce((n, r) => n + (r[k] || 0), 0) + (nav[k] || 0);
    return { ...window.__journeyMarks, domContentLoaded: nav.domContentLoadedEventEnd, load: nav.loadEventEnd, fcp,
      requests: res.length + 1, transferBytes: sum("transferSize"), encodedBytes: sum("encodedBodySize"), decodedBytes: sum("decodedBodySize"),
      byType: res.reduce((acc, r) => { const ext = (r.name.split("?")[0].match(/\\.(\\w+)$/)?.[1] ?? "other"); acc[ext] = (acc[ext] || 0) + r.encodedBodySize; return acc; }, {}),
      wasm: res.some((r) => r.name.endsWith(".wasm")) };
  })()`);
  const round = (v) => (typeof v === "number" ? Math.round(v) : v);
  results[label] = Object.fromEntries(Object.entries(t).map(([k, v]) => [k, round(v)]));
};

let firstChat = null;
let secondChat = null;
let longChat = null;
try {
  await step("open the access link (cold load)", async () => {
    await page.call("Network.clearBrowserCache");
    await page.goto(engine.url);
    await page.waitFor(`window.__journeyMarks?.synced && window.__journeyMarks?.composer && window.__journeyMarks?.sidebar`, 20000);
    await sleep(300);
    await firstLoad("coldLoad");
    check(!(await page.eval(`location.hash.includes("token")`)), "access token stays out of the address bar");
  });

  await step("sidebar shows on an empty profile", async () => {
    const nav = await page.eval(`[...document.querySelectorAll(".sb-nav-row")].map((b) => b.textContent.trim())`);
    check(nav.includes("New chat") && nav.includes("New project"), `nav rows: ${nav}`);
    await page.waitFor(`${q(".cmp-onboarding-title")}?.textContent === "Add a project to get started"`, 8000);
    await page.screenshot(join(shots, "01-empty.png"));
  });

  await step("create a project from the sidebar", async () => {
    await page.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("New project")).click()`);
    await page.waitFor(`!!${q(".pj-card")} && document.activeElement === ${q(".pj-input")}`, 8000);
    const row = (needle) => `[...document.querySelectorAll(".pj-row")].find((r) => r.textContent.includes(${JSON.stringify(needle)}))`;
    await page.eval(`${row("Open folder")}.click()`);
    await page.waitFor(`!!${row("code")}`, 8000);
    await page.eval(`${row("code")}.click()`);
    await page.waitFor(`!!${row("journey-app")}`, 8000);
    await page.eval(`${row("journey-app")}.click()`);
    await page.waitFor(`[...document.querySelectorAll(".pj-crumb")].some((c) => c.textContent.includes("journey-app"))`, 8000);
    await page.screenshot(join(shots, "02-folder.png"));
    await page.click(".pj-submit-add");
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && Object.values(window.__zuse.state.spaces).some((s) => s.path.endsWith("journey-app"))`, 10000);
    await page.waitFor(`${q(".cmp-heading")}?.textContent === "What should we build in journey-app?"`, 8000);
    // Desktop lists a project without chats only while it is the project filter, so the sidebar stays empty here.
  });

  await step("send a message and watch the reply stream with tools", async () => {
    await typeInComposer("journey-tools: bump the version");
    // Sample the transcript while the turn runs to prove intermediate states reach the DOM.
    await page.eval(`(() => {
      const seen = (window.__streamSamples = []);
      const tick = () => {
        const a = [...document.querySelectorAll(".t-entry[data-role='assistant']")].pop();
        const len = a ? a.textContent.length : 0;
        if (seen[seen.length - 1] !== len) seen.push(len);
        if (seen.length < 2000) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      return true;
    })()`);
    await enter();
    await page.waitFor(`!!${chatOf("journey-tools")} || !!window.__zuse.view.selectedChatId`, 10000);
    await page.waitFor(`document.querySelector(".t-entry[data-role='user']")?.textContent.includes("journey-tools")`, 10000);
    firstChat = await selected();
    await page.waitFor(`!!document.querySelector(".tool-group, .tool-fold")`, 15000);
    await page.screenshot(join(shots, "03-streaming.png"));
    await page.waitFor(`!(${working(firstChat)}) && [...document.querySelectorAll(".t-entry[data-role='assistant']")].some((e) => e.textContent.includes("Anything else?"))`, 30000);
    const samples = await page.eval(`window.__streamSamples`);
    results.toolsTurnDistinctLengths = samples.length;
    check(samples.length >= 5, `reply streamed in steps (saw ${samples.length} distinct lengths)`);
    await sleep(400);
    const fold = await page.eval(`document.querySelector(".tool-fold .tool-group-head")?.textContent ?? ""`);
    check(/3 tool calls/.test(fold), `turn folds its 3 tool calls (${fold})`);
    await page.click(".tool-fold .tool-group-head");
    await sleep(200);
    await page.eval(`[...document.querySelectorAll(".tool-fold-body .tool-group-head[aria-expanded='false']")].forEach((b) => b.click())`);
    await sleep(200);
    const chips = await page.eval(`[...document.querySelectorAll(".tool-fold-body .chip-label")].map((e) => e.textContent)`);
    check(chips.includes("Run") && chips.includes("Read") && chips.includes("Edited"), `tool chips: ${chips}`);
    await page.eval(`[...document.querySelectorAll(".tool-fold-body button.chip-head")].forEach((b) => b.click())`);
    await sleep(300);
    check(await page.eval(`!!document.querySelector(".t-entry[data-role='assistant'] .md h2") && !!document.querySelector(".code-block")`), "markdown heading and code block render");
    await page.screenshot(join(shots, "04-tools.png"));
  });

  await step("start a second chat and switch between chats", async () => {
    await page.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("New chat")).click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === null && !!${q(".cmp-heading")}`, 8000);
    await typeInComposer("second chat hello");
    await enter();
    await page.waitFor(`window.__zuse.view.selectedChatId && window.__zuse.view.selectedChatId !== ${JSON.stringify(firstChat)}`, 10000);
    secondChat = await selected();
    await page.waitFor(`[...document.querySelectorAll(".t-entry[data-role='assistant']")].some((e) => e.textContent.includes("${shortReply}"))`, 20000);
    await page.waitFor(`!(${working(secondChat)})`, 15000);
    // Back to the first chat through its sidebar row.
    await page.waitFor(`!!document.querySelector('.sb-row[data-chat-id="${firstChat}"]')`, 8000);
    await page.press(`.sb-row[data-chat-id="${firstChat}"]`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(firstChat)} && [...document.querySelectorAll(".t-entry[data-role='assistant']")].some((e) => e.textContent.includes("Anything else?"))`, 8000);
    check(!(await page.eval(`document.body.textContent.includes("${shortReply}")`)), "first chat no longer shows the second chat's reply");
    await page.press(`.sb-row[data-chat-id="${secondChat}"]`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(secondChat)} && document.body.textContent.includes("${shortReply}")`, 8000);
    await page.press(`.sb-row[data-chat-id="${firstChat}"]`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(firstChat)}`, 8000);
    await page.screenshot(join(shots, "05-switched.png"));
  });

  await step("open the terminal and run a command", async () => {
    await page.key("j", "KeyJ", 74, CTRL);
    await page.waitFor(`window.__zuse.view.rightPanel === "terminal" && !!document.querySelector(".xterm-helper-textarea")`, 15000);
    await page.waitFor(`document.querySelector(".xterm-rows")?.textContent.trim().length > 0`, 15000);
    await page.eval(`(document.querySelector(".xterm-helper-textarea").focus(), true)`);
    await page.type("echo journey-$((40+2))");
    await enter();
    await page.waitFor(`document.querySelector(".xterm-rows")?.textContent.includes("journey-42")`, 10000);
    await page.screenshot(join(shots, "06-terminal.png"));
  });

  await step("open files and read one", async () => {
    // The terminal keeps Ctrl+E for the shell (end of line), as on desktop; leave it first.
    await page.eval(`(${q(".cmp-input")}.focus(), true)`);
    await page.key("e", "KeyE", 69, CTRL);
    await page.waitFor(`window.__zuse.view.rightPanel === "files"`, 8000);
    const fileRow = `[...document.querySelectorAll(".app-right [role='treeitem'], .app-right button")].find((e) => e.textContent.trim().endsWith("README.md"))`;
    await page.waitFor(`!!${fileRow}`, 10000);
    await page.eval(`${fileRow}.click()`);
    // The agent's Edit was fixture output only, so the file on disk still says version 1.
    await page.waitFor(`document.querySelector(".app-right .cm-content")?.textContent.includes("version = 1")`, 10000);
    await page.screenshot(join(shots, "07-files.png"));
  });

  await step("open settings and come back", async () => {
    await page.key(",", "Comma", 188, CTRL);
    await page.waitFor(`window.__zuse.view.route.kind === "settings" && !!document.querySelector(".st-page")`, 8000);
    await page.waitFor(`document.querySelectorAll(".st-page nav button, .st-page [role='tab'], .st-nav button").length >= 5`, 8000);
    const appearance = `[...document.querySelectorAll(".st-page button")].find((b) => b.textContent.trim() === "Appearance")`;
    await page.eval(`${appearance}.click()`);
    await page.waitFor(`document.querySelector(".st-page")?.textContent.includes("Theme")`, 8000);
    await page.screenshot(join(shots, "08-settings.png"));
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && window.__zuse.view.selectedChatId === ${JSON.stringify(firstChat)}`, 8000);
  });

  await step("reload keeps the open chat and panel (warm load)", async () => {
    await page.call("Page.reload", { ignoreCache: false });
    await sleep(300);
    await page.waitFor(`window.__journeyMarks?.synced && window.__journeyMarks?.composer`, 20000);
    await sleep(300);
    await firstLoad("warmLoad");
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(firstChat)} && [...document.querySelectorAll(".t-entry[data-role='assistant']")].some((e) => e.textContent.includes("Anything else?"))`, 10000);
    await page.waitFor(`window.__zuse.view.rightPanel === "files"`, 5000);
  });

  await step("restart the engine and reconnect", async () => {
    let sawDown = false;
    await restartEngine(async () => {
      await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 10000);
      sawDown = true;
      await page.screenshot(join(shots, "09-reconnecting.png"));
      await sleep(1500);
    });
    check(sawDown, "page noticed the engine going away");
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await page.waitFor(`[...document.querySelectorAll(".t-entry[data-role='assistant']")].some((e) => e.textContent.includes("Anything else?"))`, 10000);
    const banner = await page.eval(`document.body.textContent.includes("may have completed")`);
    results.falseOutcomeBannerAfterRestart = banner;
    // The chat keeps working after reconnecting.
    await typeInComposer("after restart hello");
    await enter();
    await page.waitFor(`[...document.querySelectorAll(".t-entry[data-role='user']")].some((e) => e.textContent.includes("after restart hello"))`, 10000);
    await page.waitFor(`!(${working(firstChat)}) && [...document.querySelectorAll(".t-entry[data-role='assistant']")].some((e) => e.textContent.includes("${shortReply}"))`, 20000);
    await page.screenshot(join(shots, "10-after-restart.png"));
    // Labels come from the model catalog, which reloads after a reconnect.
    await page.waitFor(`!/claude-|fixture/.test(document.querySelector(".cmp-model")?.textContent ?? "")`, 8000).catch(async () => failures.push(`model pill shows a raw id after restart: ${await page.eval(`document.querySelector(".cmp-model")?.textContent`)}`));
  });

  await step("terminal works again after the restart", async () => {
    // The restarted engine has no PTYs left, so the panel opens a fresh shell.
    await page.eval(`(${q(".cmp-input")}.focus(), true)`);
    await page.key("j", "KeyJ", 74, CTRL);
    await page.waitFor(`window.__zuse.view.rightPanel === "terminal" && !!document.querySelector(".app-right .xterm-helper-textarea")`, 15000);
    await page.waitFor(`document.querySelector(".app-right .xterm-rows")?.textContent.trim().length > 0`, 15000);
    await page.eval(`(document.querySelector(".app-right .xterm-helper-textarea").focus(), true)`);
    await page.type("echo again-$((6*7))");
    await enter();
    await page.waitFor(`document.querySelector(".app-right .xterm-rows")?.textContent.includes("again-42")`, 10000);
  });

  await step("project actions run from the environment summary", async () => {
    await page.press('[aria-label="Toggle environment summary"]');
    await page.waitFor(`!!document.querySelector(".ws-session-card .pj-actions")`, 10000);
    // The card starts the Actions section folded, as desktop does.
    await page.eval(`(() => { const head = document.querySelector(".ws-session-card .pj-actions-head"); if (head?.getAttribute("aria-expanded") === "false") head.click(); return true })()`);
    await page.screenshot(join(shots, "10b-session-card.png"));
    const imported = `[...document.querySelectorAll(".ws-session-card button")].find((b) => /import/i.test(b.textContent))`;
    const run = `[...document.querySelectorAll(".ws-session-card .pj-actions button")].find((b) => b.textContent.includes("Say hi"))`;
    await page.waitFor(`!!(${run}) || !!(${imported})`, 10000);
    if (!(await page.eval(`!!(${run}) && !${run}.textContent.includes("Import")`))) {
      await page.eval(`${imported}.click()`);
      await page.waitFor(`!!document.querySelector(".pj-editor")`, 8000);
      await page.eval(`[...document.querySelectorAll(".pj-editor button")].find((b) => b.textContent.trim() === "Save action").click()`);
      await page.waitFor(`!document.querySelector(".pj-editor")`, 8000);
      if (!(await page.eval(`!!document.querySelector(".ws-session-card")`))) await page.press('[aria-label="Toggle environment summary"]');
      await page.eval(`(() => { const head = document.querySelector(".ws-session-card .pj-actions-head"); if (head?.getAttribute("aria-expanded") === "false") head.click(); return true })()`);
      await page.waitFor(`!!document.querySelector('.ws-session-card [aria-label="Run Say hi"]')`, 10000);
    }
    const runButton = `document.querySelector('.ws-session-card [aria-label="Run Say hi"]')`;
    await page.eval(`${runButton}.click()`);
    await page.waitFor(`window.__zuse.view.rightPanel === "terminal"`, 8000);
    await page.waitFor(`[...document.querySelectorAll(".app-right .xterm-rows")].some((r) => r.textContent.includes("action-ran-ok"))`, 15000);
    await page.screenshot(join(shots, "10c-action-terminal.png"));
  });

  await step("long chat: seed 40 turns with tools and code", async () => {
    await page.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("New chat")).click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === null && !!${q(".cmp-heading")}`, 8000);
    await typeInComposer("journey-seed-1");
    await enter();
    await page.waitFor(`window.__zuse.view.selectedChatId && ![${JSON.stringify(firstChat)}, ${JSON.stringify(secondChat)}].includes(window.__zuse.view.selectedChatId)`, 10000);
    longChat = await selected();
    const device = await page.eval(`window.__zuse.state.chats[${JSON.stringify(longChat)}].deviceId`);
    await page.waitFor(`!(${working(longChat)}) && document.body.textContent.includes("Step 1")`, 20000);
    const t0 = Date.now();
    for (let i = 2; i <= 40; i++) {
      await engine.rpc("QueueCommand", {
        chatId: longChat, targetDeviceId: device,
        command: { kind: "run", messageId: crypto.randomUUID(), request: { prompt: `journey-seed-${i}`, harness: "claude-code", cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null, attachments: [] } },
      });
      await page.waitFor(`window.__zuse.state.transcript?.messages.some((m) => JSON.stringify(m).includes("Step ${i}")) && !(${working(longChat)})`, 20000);
    }
    results.longChatSeed = { turns: 40, seconds: Math.round((Date.now() - t0) / 100) / 10, storeMessages: await page.eval(`window.__zuse.state.transcript.messages.length`), domRows: await page.eval(`document.querySelectorAll(".t-row").length`) };
  });

  await step("long chat: stream ~3000 deltas and measure", async () => {
    const deltas = (longReply.match(/[\s\S]{1,7}/g) ?? []).length;
    await page.eval(`(() => {
      const scroller = document.querySelector(".t-scroller");
      const firstRow = scroller.querySelector(".t-row");
      firstRow.__journeyMarker = true;
      const p = (window.__perf = { frames: [], longTasks: [], mutationBatches: 0, mutationRecords: 0, outsideRecords: 0, start: 0, end: 0, firstRow, samples: [] });
      new PerformanceObserver((list) => { for (const e of list.getEntries()) p.longTasks.push(e.duration); }).observe({ type: "longtask", buffered: false });
      const last = () => [...scroller.querySelectorAll(".t-entry[data-role='assistant']")].pop();
      const mo = new MutationObserver((records) => {
        p.mutationBatches++;
        p.mutationRecords += records.length;
        const live = last()?.closest(".t-row");
        for (const r of records) if (live && !live.contains(r.target) && !scroller.querySelector(".t-jump")?.contains(r.target)) p.outsideRecords++;
      });
      mo.observe(scroller, { subtree: true, childList: true, characterData: true });
      p.mo = mo;
      let prev = performance.now();
      const tick = (t) => { p.frames.push(t - prev); prev = t; if (!p.end) requestAnimationFrame(tick); };
      requestAnimationFrame(tick);
      return true;
    })()`);
    // ZUSE_CPU_THROTTLE=4 slows the page's CPU fourfold, like a modest laptop on battery.
    const throttle = Number(process.env.ZUSE_CPU_THROTTLE ?? 1);
    if (throttle > 1) await page.call("Emulation.setCPUThrottlingRate", { rate: throttle });
    results.cpuThrottle = throttle;
    await typeInComposer("journey-long");
    await page.eval(`(window.__perf.start = performance.now(), true)`);
    await enter();
    await page.waitFor(`${working(longChat)}`, 10000);
    await page.waitFor(`!(${working(longChat)}) && [...document.querySelectorAll(".t-entry[data-role='assistant']")].pop()?.textContent.includes("Section 59")`, 60000);
    await sleep(300);
    const perf = await page.eval(`(() => {
      const p = window.__perf;
      p.end = performance.now();
      p.mo.disconnect();
      const f = p.frames.slice(1).sort((a, b) => a - b);
      const pct = (x) => Math.round(f[Math.min(f.length - 1, Math.floor(f.length * x))] * 10) / 10;
      const scroller = document.querySelector(".t-scroller");
      return {
        seconds: Math.round((p.end - p.start) / 100) / 10,
        frames: f.length, frameP50: pct(0.5), frameP95: pct(0.95), frameP99: pct(0.99), frameMax: Math.round(f[f.length - 1]),
        framesOver50ms: f.filter((x) => x > 50).length,
        longTasks: p.longTasks.length, longTaskMaxMs: Math.round(Math.max(0, ...p.longTasks)), longTaskTotalMs: Math.round(p.longTasks.reduce((a, b) => a + b, 0)),
        mutationBatches: p.mutationBatches, mutationRecords: p.mutationRecords, recordsOutsideStreamingRow: p.outsideRecords,
        domRows: document.querySelectorAll(".t-row").length, storeMessages: window.__zuse.state.transcript.messages.length,
        replyChars: [...document.querySelectorAll(".t-entry[data-role='assistant']")].pop().textContent.length,
        pinnedDistance: Math.round(scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight),
        heapMB: performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : null,
      };
    })()`);
    if (throttle > 1) await page.call("Emulation.setCPUThrottlingRate", { rate: 1 });
    results.longStream = { deltas, ...perf };
    check(perf.domRows < 80, `long chat stays virtualized (${perf.domRows} rows)`);
    check(perf.pinnedDistance < 4, `stays pinned to the end (${perf.pinnedDistance}px)`);
    check(perf.mutationBatches < deltas / 2, `DOM updates batched below one per delta (${perf.mutationBatches} batches for ${deltas} deltas)`);
    await page.screenshot(join(shots, "11-long-stream.png"));
  });

  await step("hide the sidebar and bring it back from the top bar", async () => {
    await page.eval(`(${q(".cmp-input")}.focus(), true)`);
    await page.key("b", "KeyB", 66, CTRL);
    await page.waitFor(`!document.querySelector(".app-sidebar")`, 5000);
    await page.press('.ws-topbar [aria-label="Show sidebar"]');
    await page.waitFor(`!!document.querySelector(".app-sidebar .sb-nav")`, 5000);
    // The sidebar slides open (desktop's panel tween); wait until its rows take pointer hits.
    await page.waitFor(`(() => { const r = document.querySelector(".app-sidebar .sb-row")?.getBoundingClientRect(); return !!r && !!document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2)?.closest(".app-sidebar") })()`, 5000);
  });

  await step("light theme renders the chat", async () => {
    await page.eval(`(window.__zuse.view.preferences.theme, document.documentElement.dataset.theme = "light", true)`);
    await page.press(`.sb-row[data-chat-id="${firstChat}"]`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(firstChat)}`, 8000);
    await sleep(400);
    await page.screenshot(join(shots, "12-light.png"));
    await page.eval(`(document.documentElement.dataset.theme = "dark", true)`);
  });

  const unexpected = page.errors.filter((e) => !/WebSocket|ERR_CONNECTION_REFUSED|Failed to load resource|net::ERR/.test(e));
  results.pageErrors = unexpected;
  check(unexpected.length === 0, `page errors: ${unexpected.join(" | ").slice(0, 600)}`);
} finally {
  console.log(JSON.stringify(results, null, 2));
  await page.close();
  if (respawned && respawned.exitCode === null) { respawned.kill("SIGTERM"); await new Promise((r) => respawned.once("exit", r)); }
  await engine.close();
  if (ownAssets) rmSync(assets, { recursive: true, force: true });
  if (!process.env.ZUSE_SHOTS) rmSync(scratch, { recursive: true, force: true });
  else rmSync(join(scratch, "home"), { recursive: true, force: true });
}
if (failures.length) {
  console.log(`\n${failures.length} failure(s):\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("\njourney ok");
