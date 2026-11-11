// Regression checks for review findings, against a disposable engine and a scripted agent in headless Chromium.
// Usage: node e2e/defects.mjs [name ...]   (ZUSE_SHOTS=/dir saves screenshots, ZUSE_ASSETS reuses a build)
import { execFileSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "./harness.mjs";

const only = process.argv.slice(2);
const shots = process.env.ZUSE_SHOTS;
const scratch = mkdtempSync(join(tmpdir(), "zuse-defects-"));
const project = join(scratch, "demo");
mkdirSync(project, { recursive: true });
writeFileSync(join(project, "README.md"), "# demo\n");
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
writeFileSync(join(project, "pixel.png"), png);
// Declared Latin-1 with bytes that are not UTF-8, so the engine treats it as binary and the panel previews it.
const svg = `<?xml version="1.0" encoding="ISO-8859-1"?>\n<!-- \xFF\xFE -->\n<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><script>localStorage.setItem('pwned', location.origin)</script><rect width="8" height="8" fill="red"/></svg>`;
writeFileSync(join(project, "latin.svg"), Buffer.from(svg, "latin1"));
writeFileSync(join(project, "valid.svg"), `<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><script>localStorage.setItem('pwned', location.origin)</script><rect width="8" height="8" fill="red"/></svg>`);
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: project });
execFileSync("git", ["add", "."], { cwd: project });
execFileSync("git", ["commit", "-qm", "init"], { cwd: project, env: gitEnv });
mkdirSync(join(scratch, "home"));
process.env.ZUSE_WORKTREES_DIR = join(scratch, "worktrees");
process.env.PATH = "/usr/local/bin:/usr/bin:/bin";

// "slowly" keeps a run live; "permission" asks to run a command and waits; anything else replies at once.
const line = (event) => `printf '%s\\n' '${JSON.stringify(event).replace(/'/g, "'\\''")}'`;
const say = (text) => [
  line({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } } }),
  line({ type: "assistant", message: { role: "assistant", model: "fixture", content: [{ type: "text", text }] } }),
  line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: text }),
].join("\n");
function writeAgent(path) {
  writeFileSync(path, `#!/bin/sh
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
${line({ type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] })}
case "$prompt" in
  *slowly*) sleep 20
${say("Slow reply done.")};;
  *longlist*)
${Array.from({ length: 600 }, (_, i) => line({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: `- item ${i} with **bold** words and \`code\` in it\n` } } }) + "\nsleep 0.01").join("\n")}
${line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: "done" })};;
  *permission*)
${line({ type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "p1", name: "Bash", input: { command: "rm -rf build" } }] } })}
${line({ type: "control_request", request_id: "perm-1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "rm -rf build" } } })}
while IFS= read -r answer; do case "$answer" in *control_response*) break;; esac; done
${line({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "p1", content: "removed" }] } })}
${say("Cleaned the build folder.")};;
  *)
${say("Fixture reply.")};;
esac
`);
  chmodSync(path, 0o700);
}

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets, env: { HOME: join(scratch, "home") } });
writeAgent(join(engine.root, "bin", "claude"));
const cdpPort = 29000 + Math.floor(Math.random() * 900);
const page = await openBrowser(engine.url, { width: 1280, height: 820, port: cdpPort });
/** A second tab of the same browser profile, driven over its own CDP socket. */
async function secondTab(url) {
  const created = await page.call("Target.createTarget", { url });
  const targetId = created.result.targetId;
  let info;
  for (let i = 0; i < 50 && !info; i++) {
    info = (await (await fetch(`http://127.0.0.1:${cdpPort}/json`)).json()).find((t) => t.id === targetId);
    if (!info) await sleep(100);
  }
  if (!info) throw new Error("second tab did not appear");
  const ws = new WebSocket(info.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => ((ws.onopen = resolve), setTimeout(() => reject(new Error("second tab socket did not open")), 10000)));
  let id = 0;
  const waiting = new Map();
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (!d.id || !waiting.has(d.id)) return;
    waiting.get(d.id)(d);
    waiting.delete(d.id);
  };
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const i = ++id;
      const timer = setTimeout(() => (waiting.delete(i), reject(new Error(`second tab: ${method} timed out`))), 15000);
      waiting.set(i, (value) => (clearTimeout(timer), resolve(value)));
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  return {
    call,
    async eval(expression) {
      const r = await call("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      return r.result?.result?.value;
    },
    async close() {
      ws.close();
      await page.call("Target.closeTarget", { targetId });
    },
  };
}
// Socket rules: drop a matching request (the socket closes before it is sent), send it then close, or delay it.
const SOCKETS = `(() => {
  const Real = window.WebSocket;
  window.__ws = { sockets: [], rules: [] };
  window.WebSocket = class extends Real {
    constructor(url, protocols) { super(url, protocols); window.__ws.sockets.push(this); }
    send(data) {
      const text = typeof data === "string" ? data : "";
      const rule = window.__ws.rules.find((r) => text.includes(r.match));
      if (!rule) return super.send(data);
      window.__ws.rules = window.__ws.rules.filter((r) => r !== rule);
      if (rule.action === "drop") return this.close();
      if (rule.action === "sendThenClose") { super.send(data); return setTimeout(() => this.close(), 50); }
      setTimeout(() => { try { super.send(data); } catch {} }, rule.ms);
    }
  };
})();`;
await page.call("Page.addScriptToEvaluateOnNewDocument", { source: SOCKETS });
await page.call("Page.reload", { ignoreCache: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const q = JSON.stringify;
const E = (expression) => page.eval(expression);
const results = [];
const shot = async (name) => shots && (await page.screenshot(join(shots, `defects-${name}.png`)));
const expect = (value, what) => {
  if (!value) throw new Error(what);
};
const input = `document.querySelector(".cmp-input")`;
const status = (id) => `window.__zuse.state.sessions[${q(id)}]?.status`;
const connected = `window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`;
const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" };
let deviceId;
const newChat = async (chatId, title) => {
  await engine.rpc("Mutate", { op: "createChat", chatId, spaceId: "demo-space", config });
  if (title) await engine.rpc("Mutate", { op: "renameChat", chatId, title });
  await page.waitFor(`!!document.querySelector('[data-chat-id=${q(chatId)}] .sb-row-open')`);
};
const ensureChat = async (chatId, title) => {
  if (!(await E(`!!window.__zuse.state.chats[${q(chatId)}]`))) await newChat(chatId, title);
};
const openChat = async (chatId) => {
  await page.waitFor(`!!document.querySelector('[data-chat-id=${q(chatId)}] .sb-row-open')`);
  await page.click(`[data-chat-id=${q(chatId)}] .sb-row-open`);
  await page.waitFor(`window.__zuse.view.selectedChatId === ${q(chatId)}`);
};
const run = (chatId, prompt) =>
  engine.rpc("QueueCommand", {
    chatId,
    targetDeviceId: deviceId,
    command: { kind: "run", messageId: crypto.randomUUID(), request: { prompt, harness: "claude-code", cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null, attachments: [] } },
  });
const typeDraft = async (text) => {
  await E(`(() => { const t = ${input}; t.focus(); t.select(); return true })()`);
  await page.key("Backspace", "Backspace", 8);
  await page.type(text);
};

async function check(name, fn) {
  if (only.length && !only.includes(name)) return;
  process.stdout.write(`- ${name} … `);
  try {
    await fn();
    console.log("ok");
    results.push([name, "ok"]);
  } catch (error) {
    console.log(`FAILED: ${error.message}`);
    results.push([name, error.message]);
    await shot(`${name}-failed`);
  }
}

try {
  await page.waitFor(`typeof window.__ws === "object" && ${connected}`, 30000);
  deviceId = await E(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "demo-space", deviceId, path: project, name: "demo", gitDetected: true });

  await check("lost-answer", async () => {
    await newChat("chat-perm", "Approvals");
    await openChat("chat-perm");
    await run("chat-perm", "Clean up, permission needed.");
    await page.waitFor(`!!document.querySelector(".t-permission")`, 15000);
    expect((await E(status("chat-perm"))) === "awaitingInput", "session awaits input");
    // The answer never leaves: the socket closes instead of sending it.
    let epoch = await E(`window.__zuse.state.epoch`);
    await E(`window.__ws.rules.push({ match: '"respondInput"', action: "drop" })`);
    await E(`[...document.querySelectorAll(".t-permission button")].find((b) => b.textContent === "Allow once").click()`);
    await page.waitFor(`window.__zuse.state.epoch > ${epoch} && ${connected}`, 20000);
    await page.waitFor(`!!document.querySelector(".t-permission")`, 8000).catch(() => {
      throw new Error("the card did not come back after a lost answer");
    });
    expect((await E(status("chat-perm"))) === "awaitingInput", "still awaiting input");
    await shot("lost-answer-back");
    // A landed answer resolves the part after the reconnect, so the card stays gone.
    epoch = await E(`window.__zuse.state.epoch`);
    await E(`window.__ws.rules.push({ match: '"respondInput"', action: "sendThenClose" })`);
    await E(`[...document.querySelectorAll(".t-permission button")].find((b) => b.textContent === "Allow once").click()`);
    await page.waitFor(`window.__zuse.state.epoch > ${epoch} && ${connected}`, 20000);
    await page.waitFor(`document.querySelector(".t-scroller")?.textContent.includes("Cleaned the build folder.")`, 15000);
    await sleep(500);
    expect(!(await E(`!!document.querySelector(".t-permission")`)), "card hidden after a landed answer");
  });

  await check("deleted-chat", async () => {
    await newChat("chat-gone", "Doomed");
    await openChat("chat-gone");
    await run("chat-gone", "hello");
    await page.waitFor(`document.querySelector(".t-scroller")?.textContent.includes("Fixture reply.")`, 15000);
    await page.waitFor(`${status("chat-gone")} !== "working"`, 15000);
    await typeDraft("draft for the doomed chat");
    const chatsBefore = await E(`Object.keys(window.__zuse.state.chats).length`);
    await engine.rpc("Mutate", { op: "deleteChat", chatId: "chat-gone" });
    await page.waitFor(`!window.__zuse.state.chats["chat-gone"]`, 8000);
    await page.waitFor(`window.__zuse.view.selectedChatId === null`, 4000).catch(() => {
      throw new Error("selection still points at the deleted chat");
    });
    expect((await E(`window.__zuse.view.draftProjectPath`)) === project, "lands on the project's new-chat canvas");
    expect(!(await E(`document.querySelector(".t-scroller")?.textContent.includes("Fixture reply.") ?? false`)), "old transcript cleared");
    await page.waitFor(`${input}?.value === "draft for the doomed chat"`, 4000).catch(async () => {
      throw new Error(`draft not carried over: ${await E(`${input}?.value`)}`);
    });
    await shot("deleted-chat");
    expect((await E(`Object.keys(window.__zuse.state.chats).length`)) === chatsBefore - 1, "no chat was created");
    await typeDraft("");
  });

  await check("files-unsaved", async () => {
    await newChat("chat-files-a", "Files A");
    await newChat("chat-files-b", "Files B");
    await openChat("chat-files-a");
    await page.click(`button[aria-label="Show panel"]`);
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="README.md"]')`, 15000);
    await page.click('.ws-tree-row[data-path="README.md"]');
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("# demo")`);
    await page.press(".cm-content .cm-line");
    await page.key("End", "End", 35);
    await page.type(" UNSAVED EDIT");
    await page.waitFor(`!!document.querySelector(".ws-dirty")`);
    await openChat("chat-files-b");
    await openChat("chat-files-a");
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("UNSAVED EDIT")`, 8000).catch(() => {
      throw new Error("edit lost after switching chats");
    });
    expect(await E(`!!document.querySelector(".ws-dirty")`), "still marked unsaved after switching");
    await page.click(`button[aria-label="Hide panel"]`);
    await page.waitFor(`!document.querySelector(".cm-content")`);
    await page.click(`button[aria-label="Show panel"]`);
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("UNSAVED EDIT")`, 8000).catch(() => {
      throw new Error("edit lost after hiding the panel");
    });
    // Closing the page while dirty asks first.
    const asked = await E(`(() => { const e = new Event("beforeunload", { cancelable: true }); window.dispatchEvent(e); return e.defaultPrevented })()`);
    expect(asked, "beforeunload is not guarded while a file is dirty");
    await shot("files-unsaved");
    const save = `[...document.querySelectorAll(".ws-editor-head button")].find((b) => b.textContent.includes("Save"))`;
    await page.waitFor(`${save} && !${save}.disabled`, 10000);
    await E(`${save}.click()`);
    await page.waitFor(`!document.querySelector(".ws-dirty")`, 8000);
    const clean = await E(`(() => { const e = new Event("beforeunload", { cancelable: true }); window.dispatchEvent(e); return e.defaultPrevented })()`);
    expect(!clean, "beforeunload still guarded after saving");
    await page.click(`button[aria-label="Hide panel"]`);
  });

  await check("svg-preview", async () => {
    await page.eval(`localStorage.removeItem("pwned")`);
    await ensureChat("chat-files-a", "Files A");
    await openChat("chat-files-a");
    await page.click(`button[aria-label="Show panel"]`);
    await E(`document.querySelector('button[aria-label="Back to files"]')?.click()`);
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="latin.svg"]')`, 15000);
    await page.click('.ws-tree-row[data-path="latin.svg"]');
    await page.waitFor(`!!document.querySelector(".ws-image-preview img")`, 15000);
    const src = await E(`document.querySelector(".ws-image-preview img").src`);
    expect(src.startsWith("data:image/svg+xml"), `SVG preview uses ${src.slice(0, 30)}`);
    // Chromium cannot decode this non-UTF-8 SVG from any URL; the attachment below checks a valid one renders.
    // Opening the preview as a page must not run its script as this origin.
    const target = await page.call("Target.createTarget", { url: src });
    await sleep(1500);
    if (target.result?.targetId) await page.call("Target.closeTarget", { targetId: target.result.targetId });
    expect((await E(`localStorage.getItem("pwned")`)) === null, "SVG script ran as the app origin");
    await E(`document.querySelector('button[aria-label="Back to files"]')?.click()`);
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="pixel.png"]')`);
    await page.click('.ws-tree-row[data-path="pixel.png"]');
    await page.waitFor(`document.querySelector(".ws-image-preview img")?.src.startsWith("blob:") && document.querySelector(".ws-image-preview img").naturalWidth === 1`, 15000);
    await shot("svg-preview");
    await page.click(`button[aria-label="Hide panel"]`);
    // An attached SVG in the transcript renders, but never from a same-origin blob.
    await run("chat-files-a", `Look\n\nAttached images (local files — open them to view):\n- ${join(project, "valid.svg")}`);
    await page.waitFor(`!!document.querySelector(".t-thumb img")`, 15000);
    await page.waitFor(`document.querySelector(".t-thumb img").naturalWidth === 8`, 8000);
    const thumb = await E(`document.querySelector(".t-thumb img").src`);
    expect(thumb.startsWith("data:image/svg+xml"), `attachment SVG uses ${thumb.slice(0, 30)}`);
    const tab = await page.call("Target.createTarget", { url: thumb });
    await sleep(1500);
    if (tab.result?.targetId) await page.call("Target.closeTarget", { targetId: tab.result.targetId });
    expect((await E(`localStorage.getItem("pwned")`)) === null, "attachment SVG script ran as the app origin");
  });

  await check("queue-edit-race", async () => {
    await newChat("chat-q", "Queue");
    await newChat("chat-q-other", "Elsewhere");
    await openChat("chat-q");
    await run("chat-q", "please reply slowly");
    await page.waitFor(`${status("chat-q")} === "working"`, 15000);
    await engine.rpc("QueueMessage", { chatId: "chat-q", targetDeviceId: deviceId, text: "queued one", attachments: [], holdForTurnEnd: true });
    await page.waitFor(`[...document.querySelectorAll(".cmp-queue-row")].some((r) => r.innerText.includes("queued one"))`, 8000);
    await typeDraft("my precious draft");
    await sleep(400);
    await E(`window.__ws.rules.push({ match: '"BeginQueuedMessageEdit"', action: "delay", ms: 1500 })`);
    await E(`[...document.querySelectorAll(".cmp-queue-row")].find((r) => r.innerText.includes("queued one")).querySelector('[aria-label="Edit"]').click()`);
    await openChat("chat-q-other");
    await sleep(2500);
    await openChat("chat-q");
    await sleep(300);
    const text = await E(`${input}.value`);
    expect(text === "my precious draft", `draft became ${q(text)}`);
    expect(!(await E(`!!document.querySelector('[aria-label="Save to queue"]')`)), "an edit lease is still held");
    await sleep(400);
    const stored = JSON.parse((await E(`localStorage.getItem("zuse.web.composer.drafts.v1")`)) ?? "{}");
    expect(stored["chat-q"]?.text === "my precious draft", `stored draft ${q(stored["chat-q"])}`);
    // The lease went back to the host: editing again acquires it.
    await E(`[...document.querySelectorAll(".cmp-queue-row")].find((r) => r.innerText.includes("queued one")).querySelector('[aria-label="Edit"]').click()`);
    await page.waitFor(`${input}.value === "queued one" && !!document.querySelector('[aria-label="Save to queue"]:not([disabled])')`, 8000).catch(async () => {
      throw new Error(`could not edit again: ${await E(`document.querySelector(".cmp-pill")?.innerText ?? ""`)}`);
    });
    await E(`document.querySelector('.cmp-queue-row [aria-label="Cancel"]').click()`);
    await page.waitFor(`${input}.value === "my precious draft"`, 8000);
    await typeDraft("");
    await engine.rpc("QueueCommand", { chatId: "chat-q", targetDeviceId: deviceId, command: { kind: "interrupt" } }).catch(() => {});
  });

  await check("drafts-two-tabs", async () => {
    await ensureChat("chat-files-a", "Files A");
    await ensureChat("chat-files-b", "Files B");
    await openChat("chat-files-a");
    await typeDraft("draft typed in tab A");
    await sleep(500);
    const tab = await secondTab(engine.origin + "/");
    const evalB = (expression) => tab.eval(expression);
    try {
    for (let i = 0; i < 100 && !(await evalB(connected)); i++) await sleep(200);
    await evalB(`document.querySelector('[data-chat-id="chat-files-b"] .sb-row-open').click()`);
    for (let i = 0; i < 50 && (await evalB(`window.__zuse.view.selectedChatId`)) !== "chat-files-b"; i++) await sleep(100);
    await evalB(`(() => { const t = document.querySelector(".cmp-input"); t.focus(); return true })()`);
    await tab.call("Input.insertText", { text: "draft typed in tab B" });
    await sleep(800);
    const stored = JSON.parse((await E(`localStorage.getItem("zuse.web.composer.drafts.v1")`)) ?? "{}");
    expect(stored["chat-files-a"]?.text === "draft typed in tab A", `tab A's draft lost from storage: ${q(stored)}`);
    expect(stored["chat-files-b"]?.text === "draft typed in tab B", `tab B's draft missing: ${q(stored)}`);
    // Tab A learns tab B's draft without a reload.
    await openChat("chat-files-b");
    await page.waitFor(`${input}.value === "draft typed in tab B"`, 4000).catch(() => {
      throw new Error("tab A did not merge tab B's draft");
    });
    // Sending clears it in both.
    await evalB(`(() => { const t = document.querySelector(".cmp-input"); t.focus(); t.select(); return true })()`);
    await tab.call("Input.dispatchKeyEvent", { type: "keyDown", key: "Backspace", code: "Backspace", windowsVirtualKeyCode: 8 });
    await tab.call("Input.dispatchKeyEvent", { type: "keyUp", key: "Backspace", code: "Backspace", windowsVirtualKeyCode: 8 });
    await sleep(600);
    } finally {
      await tab.close().catch(() => {});
    }
    await sleep(600);
    await page.call("Page.reload", { ignoreCache: false });
    await page.waitFor(connected, 20000);
    await openChat("chat-files-a");
    await page.waitFor(`${input}.value === "draft typed in tab A"`, 4000).catch(async () => {
      throw new Error(`tab A's draft lost after reload: ${q(await E(`${input}.value`))}`);
    });
    await openChat("chat-files-b");
    await sleep(300);
    expect((await E(`${input}.value`)) === "", "a cleared draft came back");
    await openChat("chat-files-a");
    await typeDraft("");
  });

  await check("rename-focus", async () => {
    await ensureChat("chat-files-a", "Files A");
    const row = `[data-chat-id="chat-files-a"] .sb-row-open`;
    const rename = async () => {
      await page.eval(`document.querySelector('${row}').focus()`);
      await page.key("F2", "F2", 113);
      await page.waitFor(`document.activeElement?.classList.contains("sb-rename")`, 3000).catch(() => {
        throw new Error("F2 did not start renaming");
      });
    };
    for (const [key, code] of [["Escape", 27], ["Enter", 13]]) {
      await rename();
      await page.key(key, key, code);
      await page.waitFor(`document.activeElement?.matches('${row}')`, 3000).catch(async () => {
        throw new Error(`${key} left focus on ${await E(`document.activeElement?.tagName`)}`);
      });
    }
    // App shortcuts still work while renaming.
    await rename();
    await page.key("k", "KeyK", 75, 2);
    await page.waitFor(`!!document.querySelector('[role="dialog"]')`, 3000).catch(() => {
      throw new Error("Ctrl+K did nothing while renaming");
    });
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector('[role="dialog"]')`, 3000);
  });

  await check("phone-layout", async () => {
    await ensureChat("chat-perm", "Approvals");
    await ensureChat("chat-files-a", "Files A");
    await openChat("chat-perm");
    await page.call("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
    await sleep(500);
    expect(!(await E(`!!document.querySelector(".app-sidebar")`)), "sidebar shows beside the chat on a phone");
    const pane = await E(`Math.round(document.querySelector(".app-pane").getBoundingClientRect().width)`);
    expect(pane >= 370, `chat pane is ${pane}px wide`);
    const titleLines = await E(`(() => { const e = document.querySelector(".ws-topbar"); return Math.round(e.getBoundingClientRect().height) })()`);
    await shot("phone-chat");
    await page.click(`button[aria-label="Show sidebar"]`);
    await page.waitFor(`!!document.querySelector(".app-sidebar")`);
    const drawer = await E(`(() => { const r = document.querySelector(".app-sidebar").getBoundingClientRect(); return { w: Math.round(r.width), pos: getComputedStyle(document.querySelector(".app-sidebar")).position } })()`);
    expect(drawer.pos === "fixed" && drawer.w <= 340, `drawer ${q(drawer)}`);
    expect(await E(`document.querySelector(".app-sidebar").contains(document.activeElement)`), "focus moves into the drawer");
    await shot("phone-drawer");
    await page.click(`[data-chat-id="chat-files-a"] .sb-row-open`);
    await page.waitFor(`!document.querySelector(".app-sidebar") && window.__zuse.view.selectedChatId === "chat-files-a"`, 3000);
    await E(`document.querySelector('button[aria-label="Show sidebar"]').focus()`);
    await page.click(`button[aria-label="Show sidebar"]`);
    await page.waitFor(`!!document.querySelector(".app-sidebar")`);
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".app-sidebar")`, 3000);
    expect(await E(`document.activeElement?.getAttribute("aria-label") === "Show sidebar"`), "Escape returns focus to the sidebar toggle");
    await page.click(`button[aria-label="Show panel"]`);
    await page.waitFor(`!!document.querySelector(".app-right")`);
    const right = await E(`Math.round(document.querySelector(".app-right").getBoundingClientRect().width)`);
    expect(right >= 370, `right panel is ${right}px wide`);
    await shot("phone-panel");
    await page.click(`button[aria-label="Close panel"]`);
    results.push(["phone-layout: top bar height", String(titleLines)]);
    await page.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 820, deviceScaleFactor: 1, mobile: false });
    await sleep(300);
    expect(await E(`!!document.querySelector(".app-sidebar")`), "sidebar returns on a wide window");
  });

  await check("stream-list", async () => {
    await ensureChat("chat-list", "Long list");
    await openChat("chat-list");
    await page.call("Emulation.setCPUThrottlingRate", { rate: 4 });
    await E(`(() => { window.__long = []; window.__lost = 0; window.__samples = 0; new PerformanceObserver((l) => l.getEntries().forEach((e) => window.__long.push(Math.round(e.duration)))).observe({ type: "long-animation-frame" }); return true })()`);
    await run("chat-list", "stream a longlist please");
    await page.waitFor(`document.querySelectorAll(".md li").length > 20`, 20000);
    // While it streams, an item the reader is looking at keeps its DOM.
    await E(`(() => { const tick = () => { const items = document.querySelectorAll(".md li"); if (window.__watched) { window.__samples++; if (!window.__watched.isConnected) window.__lost++; } window.__watched = items[items.length - 2]; if (window.__samples < 20) setTimeout(tick, 100); }; tick(); return true })()`);
    await page.waitFor(`${status("chat-list")} !== "working"`, 60000);
    await page.call("Emulation.setCPUThrottlingRate", { rate: 1 });
    const long = await E(`window.__long`);
    const lost = await E(`window.__lost + "/" + window.__samples`);
    const items = await E(`document.querySelectorAll(".md li").length`);
    const sorted = [...long].sort((a, b) => a - b);
    results.push(["stream-list: 4x long frames (count, p95, max, sum)", `${long.length}, ${sorted[Math.floor(sorted.length * 0.95)] ?? 0}, ${sorted.at(-1) ?? 0}, ${long.reduce((a, b) => a + b, 0)}`]);
    results.push(["stream-list: watched item replaced", lost]);
    expect(items === 600, `${items} list items rendered`);
    expect(lost.startsWith("0/"), `streaming replaced the item being read ${lost}`);
  });

  await check("reconnect-banner", async () => {
    await ensureChat("chat-perm", "Approvals");
    await openChat("chat-perm");
    await page.click(`button[aria-label="Show panel"]`);
    await page.waitFor(`!!document.querySelector(".app-right")`);
    // Hold reconnects off so the banner stays up.
    await E(`(() => { const Real = WebSocket; window.WebSocket = class extends Real { constructor(u, p) { super("ws://127.0.0.1:9/blocked", p); } }; window.__ws.sockets.forEach((s) => s.close()); return true })()`);
    await page.waitFor(`!!document.querySelector(".cx-banner")`, 5000);
    await sleep(300);
    const covered = await E(`[...document.querySelectorAll(".ws-topbar-actions button, .ws-right-strip button")].filter((b) => { const r = b.getBoundingClientRect(); const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2); return !b.contains(hit); }).map((b) => b.getAttribute("aria-label"))`);
    expect(covered.length === 0, `covered controls: ${covered.join(", ")}`);
    const indicators = await E(`[...document.querySelectorAll(".cx-banner, .cmp-pill")].filter((e) => /Reconnecting/.test(e.textContent)).length`);
    expect(indicators === 1, `${indicators} reconnecting indicators`);
    await shot("reconnect-banner");
    await page.call("Page.reload", { ignoreCache: false });
    await page.waitFor(connected, 20000);
  });
} finally {
  console.log(JSON.stringify(results, null, 1));
  if (page.errors.length) console.log("page errors:", page.errors.join("\n"));
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(scratch, { recursive: true, force: true });
}
const failed = results.filter(([name, outcome]) => !name.includes(":") && outcome !== "ok");
if (failed.length) process.exit(1);
