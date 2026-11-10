// Transcript controls against a real, disposable engine: questions, plan approval, private secrets, errors,
// forks into a worktree, tab history, side chats, the lightbox, the rail, and a reconnect mid-stream.
// Usage: node e2e/transcript/interactions.mjs (ZUSE_SHOTS=dir keeps screenshots).
import { execFileSync, spawn } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { buildApp, openBrowser, startEngine } from "../harness.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const work = mkdtempSync(join(tmpdir(), "zuse-transcript-ui-"));
const project = join(work, "repo");
mkdirSync(project);
writeFileSync(join(project, "README.md"), "# Fixture\n");
const git = (...args) => execFileSync("git", ["-C", project, "-c", "user.email=t@t", "-c", "user.name=t", ...args], { stdio: "ignore" });
git("init", "-q", "-b", "main");
git("add", ".");
git("commit", "-qm", "init");
// A 64x40 PNG for the attachment thumbnail.
const chunk = (type, data) => {
  const body = Buffer.concat([Buffer.from(type), data]);
  const out = Buffer.alloc(body.length + 8);
  out.writeUInt32BE(data.length, 0);
  body.copy(out, 4);
  out.writeUInt32BE(crc32(body), body.length + 4);
  return out;
};
const header = Buffer.alloc(13);
header.writeUInt32BE(64, 0);
header.writeUInt32BE(40, 4);
header.set([8, 2, 0, 0, 0], 8);
const pixels = Buffer.concat(Array.from({ length: 40 }, () => Buffer.from([0, ...Array.from({ length: 64 }, () => [200, 80, 40]).flat()])));
writeFileSync(join(project, "shot.png"), Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk("IHDR", header), chunk("IDAT", deflateSync(pixels)), chunk("IEND", Buffer.alloc(0))]));

const init = { type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] };
const text = (t) => [
  { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } },
  { type: "stream_event", event: { type: "content_block_delta", delta: { type: "text_delta", text: t } } },
  { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: t }] } },
];
const done = (t) => [...text(t), { type: "result", subtype: "success", session_id: "fixture", is_error: false, result: t }];
const use = (id, name, input) => ({ type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id, name, input }] } });
const result = (id, content) => ({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content }] } });
const ask = { questions: [
  { header: "Stack", question: "Which database should I use?", multiSelect: false, options: [{ label: "Postgres" }, { label: "SQLite" }, { label: "Redis" }] },
  { header: "Extras", question: "Which extras do you want?", multiSelect: true, options: [{ label: "Tests" }, { label: "Docs" }, { label: "CI" }] },
] };
const plan = "# Plan\n\n1. Add a `version` field\n2. Bump it to **2**";
const scenarios = {
  default: [init, ...done("Fixture reply.")],
  question: [init, use("q1", "AskUserQuestion", ask), { type: "control_request", request_id: "ask-1", request: { subtype: "can_use_tool", tool_name: "AskUserQuestion", input: ask } }, "WAIT", result("q1", "ok"), ...done("Thanks for answering.")],
  plan: [init, use("x1", "ExitPlanMode", { plan }), { type: "control_request", request_id: "plan-1", request: { subtype: "can_use_tool", tool_name: "ExitPlanMode", input: { plan } } }, "WAIT", result("x1", "ok"), ...done("Plan handled.")],
  crash: [init, ...text("About to crash"), "echo 'fatal: something broke' >&2; exit 3"],
  hang: [init, ...text("Working on it"), "sleep 120"],
  slow: [init, { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } },
    ...Array.from({ length: 40 }, (_, i) => [{ type: "stream_event", event: { type: "content_block_delta", delta: { type: "text_delta", text: `Token ${i} ` } } }, "sleep 0.1"]).flat(),
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: Array.from({ length: 40 }, (_, i) => `Token ${i} `).join("") }] } },
    { type: "result", subtype: "success", session_id: "fixture", is_error: false, result: "done" }],
};
const responses = join(work, "responses.log");
const emit = (list) => list.map((e) => e === "WAIT"
  ? `while IFS= read -r line; do case "$line" in *control_response*) printf '%s\\n' "$line" >> '${responses}'; break;; esac; done`
  : typeof e === "string" ? e : `printf '%s\\n' '${JSON.stringify(e).replace(/'/g, "'\\''")}'\nsleep 0.02`).join("\n");

const assets = buildApp();
const engine = await startEngine({ assets, env: { HOME: join(work, "home") } });
writeFileSync(join(engine.root, "bin", "claude"), `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
while IFS= read -r line; do case "$line" in *'"type":"user"'*) prompt="$line"; break;; esac; done
case "$prompt" in
${Object.entries(scenarios).filter(([n]) => n !== "default").map(([name, list]) => `*scn:${name}*)\n${emit(list)}\n;;`).join("\n")}
*)
${emit(scenarios.default)}
;;
esac
`);
chmodSync(join(engine.root, "bin", "claude"), 0o700);
const shots = process.env.ZUSE_SHOTS ?? join(work, "shots");
mkdirSync(shots, { recursive: true });
const page = await openBrowser(engine.url, { width: 1280, height: 860 });
if (process.env.ZUSE_VERBOSE) {
  const call = page.call;
  page.call = (method, params) => { console.log(" ", method, JSON.stringify(params ?? {}).slice(0, 140)); return call(method, params); };
}
// Restart the engine in place (same port, profile and environment) so the page sees a real disconnect.
let respawned = null;
async function restartEngine(downFor = async () => {}) {
  const port = new URL(engine.url).port;
  const pid = execFileSync("pgrep", ["-f", `data-dir ${engine.root}/profile`]).toString().trim().split("\n")[0];
  const argv = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0").filter(Boolean);
  const env = Object.fromEntries(readFileSync(`/proc/${pid}/environ`, "utf8").split("\0").filter(Boolean).map((kv) => [kv.slice(0, kv.indexOf("=")), kv.slice(kv.indexOf("=") + 1)]));
  process.kill(Number(pid), "SIGTERM");
  for (let i = 0; i < 50; i++) { try { process.kill(Number(pid), 0); await sleep(100); } catch { break; } }
  await downFor();
  const args = argv.slice(1).map((a, i, all) => (all[i - 1] === "--listen" ? `127.0.0.1:${port}` : all[i - 1] === "--ipc-port" ? String(engine.ipcPort) : a));
  respawned = spawn(argv[0], args, { env, stdio: ["ignore", "pipe", "pipe"] });
  let log = "";
  respawned.stdout.on("data", (d) => (log += d));
  respawned.stderr.on("data", (d) => (log += d));
  for (let i = 0; i < 100 && !/Open http/.test(log); i++) await sleep(100);
}
const failures = [];
const check = (cond, what) => { if (!cond) failures.push(what); };
const step = async (name, fn) => {
  if (process.env.ZUSE_ONLY && !process.env.ZUSE_ONLY.split(",").includes(name)) return;
  if (process.env.ZUSE_VERBOSE) console.log("step", name);
  try { await fn(); } catch (error) { if (process.env.ZUSE_VERBOSE) console.log(error); failures.push(`${name}: ${error.message}`); await page.screenshot(join(shots, `fail-${name}.png`)).catch(() => {}); }
};
const enter = async () => {
  await page.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" });
  await page.call("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
};
const lastResponse = () => { try { return readFileSync(responses, "utf8").trim().split("\n").at(-1); } catch { return ""; } };

let closing = false;
process.on("SIGINT", async () => {
  if (closing) return;
  closing = true;
  await page.close();
  if (respawned && respawned.exitCode === null) { respawned.kill("SIGTERM"); await new Promise((r) => respawned.once("exit", r)); }
  await engine.close();
  rmSync(assets, { recursive: true, force: true });
  rmSync(work, { recursive: true, force: true });
  process.exit(130);
});
try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`);
  const device = await page.eval(`window.__zuse.state.localDeviceId`);
  const spaceId = crypto.randomUUID();
  await engine.rpc("Mutate", { op: "createSpace", spaceId, deviceId: device, path: project, name: "repo", gitDetected: true });
  const newChat = async (title, extra = {}) => {
    const chatId = crypto.randomUUID();
    await engine.rpc("Mutate", { op: "createChat", chatId, spaceId, config: { harness: "claude-code", sandbox: "workspace-write" }, branch: "main", ...extra });
    await engine.rpc("Mutate", { op: "renameChat", chatId, title });
    await page.waitFor(`window.__zuse.state.chats[${JSON.stringify(chatId)}]?.title === ${JSON.stringify(title)}`);
    return chatId;
  };
  const open = async (title, chatId) => {
    const row = `[...document.querySelectorAll('.app-sidebar button, .app-sidebar [role="option"]')].find((e) => e.textContent.trim().startsWith(${JSON.stringify(title)}))`;
    await page.waitFor(`!!${row}`, 8000);
    await page.eval(`${row}.click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(chatId)}`, 8000);
  };
  const run = (chatId, prompt) => engine.rpc("QueueCommand", {
    chatId, targetDeviceId: device,
    command: { kind: "run", messageId: crypto.randomUUID(), request: { prompt, harness: "claude-code", cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null, attachments: [] } },
  });
  const idle = (chatId) => page.waitFor(`window.__zuse.state.sessions[${JSON.stringify(chatId)}]?.status !== "working"`, 20000);

  const main = await newChat("Main");
  await open("Main", main);

  await step("question", async () => {
    await run(main, "scn:question");
    await page.waitFor(`!!document.querySelector(".t-question")`, 15000);
    check(await page.eval(`getComputedStyle(document.querySelector(".cmp-root")).display === "none"`), "composer hides while the question tray is open");
    await page.screenshot(join(shots, "question.png"));
    await page.key("1", "Digit1", 49);
    await page.waitFor(`document.querySelector(".t-question-text")?.textContent.includes("extras")`, 3000);
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`document.querySelector(".t-question-text")?.textContent.includes("database")`, 3000);
    await page.key("2", "Digit2", 50);
    await page.key("1", "Digit1", 49);
    await page.key("3", "Digit3", 51);
    await enter();
    await page.waitFor(`!document.querySelector(".t-question")`, 5000);
    await page.waitFor(`document.querySelector(".t-scroller").textContent.includes("Thanks for answering")`, 15000);
    const answer = JSON.parse(lastResponse()).response.response.updatedInput.answers;
    check(answer["Which database should I use?"] === "SQLite" && JSON.stringify(answer["Which extras do you want?"]) === '["Tests","CI"]', `question answers ${JSON.stringify(answer)}`);
    check(await page.eval(`getComputedStyle(document.querySelector(".cmp-root")).display !== "none"`), "composer returns after answering");
    await idle(main);
  });

  await step("plan", async () => {
    await run(main, "scn:plan");
    await page.waitFor(`!!document.querySelector(".t-permission-plan h1")`, 15000);
    check(await page.eval(`document.querySelector(".t-permission-title").textContent === "Approve plan?"`), "plan approval titles itself");
    await page.screenshot(join(shots, "plan.png"));
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".t-permission")`, 5000);
    for (let i = 0; i < 50 && !lastResponse().includes("plan-1"); i++) await sleep(100);
    check(lastResponse().includes("plan-1") && lastResponse().includes('"behavior":"deny"'), `Escape denies the plan (${lastResponse()})`);
    await idle(main);
  });

  await step("error", async () => {
    await run(main, "scn:crash");
    await page.waitFor(`!!document.querySelector(".t-error")`, 15000);
    const message = await page.eval(`document.querySelector(".t-error-message").textContent`);
    check(message.includes("exit code 3") && message.includes("fatal: something broke"), `error message ${message}`);
    check(await page.eval(`!!document.querySelector(".t-error button[aria-label='Copy message']")`), "error offers copy");
    await page.screenshot(join(shots, "error.png"));
    await idle(main);
  });

  await step("secret", async () => {
    await run(main, "scn:hang");
    await page.waitFor(`document.querySelector(".t-scroller").textContent.includes("Working on it")`, 15000);
    const operation = { label: "API token", reason: "Check access", url: "https://zuse-test.invalid/user", method: "GET" };
    const declined = engine.rpc("RequestPrivateOperation", { chatId: main, targetDeviceId: device, operation });
    await page.waitFor(`document.activeElement?.type === "password"`, 8000);
    await page.screenshot(join(shots, "secret.png"));
    await page.eval(`[...document.querySelectorAll(".t-question button")].find((b) => b.textContent === "Decline").click()`);
    check(JSON.stringify(await declined).includes("declined"), "declining a secret reaches the engine");
    const approved = engine.rpc("RequestPrivateOperation", { chatId: main, targetDeviceId: device, operation }).catch((e) => String(e));
    await page.waitFor(`document.activeElement?.type === "password"`, 8000);
    await page.type("s3cret-value");
    await enter();
    await page.waitFor(`!document.querySelector(".t-dock")`, 5000);
    await approved;
    check(!(await page.eval(`document.body.innerText.includes("s3cret-value") || JSON.stringify(window.__zuse.state).includes("s3cret-value")`)), "secret stays out of the page");
    await engine.rpc("QueueCommand", { chatId: main, targetDeviceId: device, command: { kind: "interrupt" } });
    await idle(main);
  });

  await step("lightbox", async () => {
    await run(main, `Look\n\nAttached images (local files — open them to view):\n- ${join(project, "shot.png")}`);
    await page.waitFor(`!!document.querySelector(".t-thumb img")`, 15000);
    await page.eval(`document.querySelector(".t-thumb").focus()`);
    await enter();
    await page.waitFor(`!!document.querySelector(".dialog .t-lightbox img")`, 5000);
    await page.screenshot(join(shots, "lightbox.png"));
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`document.activeElement?.classList.contains("t-thumb")`, 3000);
    await idle(main);
  });

  await step("fork-thread", async () => {
    await page.eval(`(() => { const b = [...document.querySelectorAll(".t-meta button[aria-label='Fork chat']")].at(-1); b.id = "fork-target"; b.focus(); })()`);
    // Enter then an immediate ArrowDown, as a fast keyboard user does: this froze the page with a modal menu.
    await enter();
    await page.key("ArrowDown", "ArrowDown", 40);
    await page.waitFor(`document.activeElement?.getAttribute("role") === "menuitem"`, 3000);
    if (!(await page.eval(`document.activeElement.textContent.includes("Fork into new thread")`))) await page.key("ArrowDown", "ArrowDown", 40);
    await page.waitFor(`document.activeElement?.textContent.includes("Fork into new thread")`, 3000);
    await enter();
    await page.waitFor(`window.__zuse.view.selectedChatId !== ${JSON.stringify(main)} && !!window.__zuse.state.chats[window.__zuse.view.selectedChatId]?.cwd`, 15000);
    const fork = JSON.parse(await page.eval(`JSON.stringify(window.__zuse.state.chats[window.__zuse.view.selectedChatId])`));
    check(fork.cwd?.startsWith(join(work, "home")) && fork.branch?.startsWith("zuse/"), `fork runs in its own worktree (${fork.cwd} ${fork.branch})`);
    await page.waitFor(`!!document.querySelector(".t-fork")`, 8000);
    await page.screenshot(join(shots, "fork-thread.png"));
  });

  await step("tabs", async () => {
    await open("Main", main);
    await page.click(".t-strip-btn[aria-label='New chat in this thread']");
    await page.waitFor(`document.querySelectorAll(".t-tab").length === 2`, 10000);
    await page.eval(`document.querySelector(".t-tab[data-active]").focus()`);
    await page.key("Delete", "Delete", 46);
    await page.waitFor(`document.querySelectorAll(".t-tab").length === 1`, 3000);
    check(await page.eval(`document.activeElement?.classList.contains("t-tab")`), "focus stays in the tab strip after closing");
    await page.press(".t-strip-btn[aria-label='Earlier chats']");
    await page.waitFor(`document.querySelectorAll(".t-history-row").length === 1`, 3000);
    await page.screenshot(join(shots, "history.png"));
    await page.eval(`document.querySelector(".t-history-row").id = "restore"`);
    await page.press("#restore");
    await page.waitFor(`document.querySelectorAll(".t-tab").length === 2`, 3000);
    await page.eval(`document.querySelector(".t-tab").click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(main)}`, 3000);
    const side = await newChat("Helper", { parentChatId: main });
    await page.eval(`document.querySelector(".t-tab[title^='Helper']").click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(side)}`, 3000);
    check(await page.eval(`document.querySelector(".t-tab[data-active]")?.getAttribute("title").startsWith("Helper") && document.querySelectorAll(".t-tab").length === 3`), "an open side chat keeps its parent's strip");
    await page.eval(`document.querySelector(".t-tab").click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(main)}`, 3000);
  });

  await step("rail", async () => {
    await open("Main", main);
    for (let i = 0; i < 6; i++) { await run(main, `Prompt number ${i}`); await sleep(200); await idle(main); }
    await page.waitFor(`!!document.querySelector(".t-rail-trigger")`, 8000);
    await page.eval(`document.querySelector(".t-rail-trigger").focus()`);
    await enter();
    await page.waitFor(`document.activeElement?.classList.contains("t-rail-list")`, 3000);
    await page.screenshot(join(shots, "rail.png"));
    for (let i = 0; i < 3; i++) await page.key("ArrowUp", "ArrowUp", 38);
    await enter();
    await sleep(800);
    check(await page.eval(`document.activeElement?.classList.contains("t-scroller")`), "focus lands in the conversation after picking a prompt");
    check(await page.eval(`!!document.querySelector(".t-jump")`), "picking an earlier prompt leaves the end");
    // The reading position survives switching to another chat and back, to the pixel.
    const anchor = await page.eval(`(() => { const r = [...document.querySelectorAll(".t-row")].find((r) => r.getBoundingClientRect().top >= 80); return { id: r.dataset.entry, top: r.getBoundingClientRect().top }; })()`);
    const other = await newChat("Elsewhere");
    await open("Elsewhere", other);
    await open("Main", main);
    await sleep(800);
    const back = await page.eval(`document.querySelector(".t-row[data-entry='${anchor.id}']")?.getBoundingClientRect().top ?? null`);
    check(back !== null && Math.abs(back - anchor.top) < 2, `reading position restored (${anchor.top} → ${back})`);
    await page.eval(`document.querySelector(".t-jump").focus()`);
    await enter();
    await sleep(1200);
    check(await page.eval(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight < 4 })()`), "Jump to latest reaches the end");
  });

  await step("failed-answer", async () => {
    const other = await newChat("Offline");
    await open("Offline", other);
    await run(other, "scn:question");
    await page.waitFor(`!!document.querySelector(".t-question")`, 15000);
    await restartEngine(async () => {
      await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 10000);
      await page.key("1", "Digit1", 49);
      await page.key("1", "Digit1", 49);
      await enter();
      await sleep(500);
      check(await page.eval(`!!document.querySelector(".t-question")`), "the tray stays when the answer could not be sent");
      check(await page.eval(`document.body.innerText.includes("Answer failed")`), "a failed answer says so");
      await page.screenshot(join(shots, "answer-offline.png"));
    });
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    // The restarted engine closes the orphaned request; the tray goes with it.
    await page.waitFor(`!document.querySelector(".t-question")`, 15000);
  });

  await step("stream-reconnect", async () => {
    await open("Main", main);
    await run(main, "scn:slow");
    await page.waitFor(`document.querySelector(".t-scroller").textContent.includes("Token 5 ")`, 15000);
    await restartEngine(async () => {
      await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 10000);
      await page.screenshot(join(shots, "disconnected.png"));
    });
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await page.waitFor(`window.__zuse.state.transcript?.ready && window.__zuse.state.sessions[${JSON.stringify(main)}]?.status !== "working"`, 20000);
    await sleep(500);
    const shown = await page.eval(`document.querySelector(".t-scroller").textContent`);
    check(/Token 5 /.test(shown) && (shown.match(/Token 5 /g) ?? []).length === 1, "the partial reply survives the reconnect exactly once");
    check(await page.eval(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight < 4 })()`), "still pinned to the end after reconnecting");
    await page.screenshot(join(shots, "reconnected.png"));
  });

  check(page.errors.length === 0, `page errors: ${page.errors.join(" | ")}`);
  if (failures.length) throw new Error(`failed:\n- ${failures.join("\n- ")}`);
  console.log("interactions ok", process.env.ZUSE_SHOTS ? `; screenshots in ${shots}` : "");
} finally {
  await page.close();
  if (respawned && respawned.exitCode === null) { respawned.kill("SIGTERM"); await new Promise((r) => respawned.once("exit", r)); }
  await engine.close();
  rmSync(assets, { recursive: true, force: true });
  rmSync(work, { recursive: true, force: true });
}
