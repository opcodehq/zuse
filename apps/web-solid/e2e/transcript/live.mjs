// The transcript against a real, disposable engine: a fixture agent streams thinking, tool calls and a
// markdown reply; the built app must show them through WatchDocMessages. Usage: node e2e/transcript/live.mjs
import { mkdtempSync, rmSync, writeFileSync, chmodSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "../harness.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const project = mkdtempSync(join(tmpdir(), "zuse-transcript-project-"));
writeFileSync(join(project, "README.md"), "# Fixture\n\nversion = 1\n");

const delta = (type, key, text) => ({ type: "stream_event", event: { type: "content_block_delta", delta: { type, [key]: text } } });
const reply = [
  "## Summary\n\nThe project has **one** file. Here is what changed:\n\n",
  "| File | Change |\n| --- | --- |\n| `README.md` | version bumped |\n\n",
  "```ts\nexport const version = 2;\n```\n\n",
  "See [the docs](https://example.com/docs) for more.",
];
const events = [
  { type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] },
  { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } } },
  ...["Let me look ", "at the project ", "first. Then I will ", "bump the version."].map((t) => delta("thinking_delta", "thinking", t)),
  { type: "assistant", message: { role: "assistant", content: [{ type: "thinking", thinking: "Let me look at the project first. Then I will bump the version.", signature: "x" }] } },
  { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "ls -la" } }] } },
  { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: "total 8\n-rw-r--r-- 1 user user 30 README.md" }] } },
  { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "t2", name: "Read", input: { file_path: join(project, "README.md") } }] } },
  { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t2", content: "# Fixture\n\nversion = 1\n" }] } },
  { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "t3", name: "Edit", input: { file_path: join(project, "README.md"), old_string: "version = 1", new_string: "version = 2" } }] } },
  { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t3", content: "The file was updated." }] } },
  { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } },
  ...reply.flatMap((chunk) => chunk.match(/.{1,6}/gs)).map((t) => delta("text_delta", "text", t)),
  { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: reply.join("") }] } },
  { type: "result", subtype: "success", session_id: "fixture", is_error: false, result: reply.join("") },
];

// A prompt mentioning "permission" asks to run a command and waits for the answer on stdin.
const approval = [
  { type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] },
  { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "p1", name: "Bash", input: { command: "rm -rf build" } }] } },
  { type: "control_request", request_id: "perm-1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "rm -rf build" } } },
  "WAIT",
  { type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "p1", content: "removed" }] } },
  { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } },
  delta("text_delta", "text", "Cleaned the build folder."),
  { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: "Cleaned the build folder." }] } },
  { type: "result", subtype: "success", session_id: "fixture", is_error: false, result: "Cleaned the build folder." },
];

const emit = (list) => list.map((e) => e === "WAIT"
  ? `while IFS= read -r line; do case "$line" in *control_response*) break;; esac; done`
  : `printf '%s\\n' '${JSON.stringify(e).replace(/'/g, "'\\''")}'\nsleep 0.02`).join("\n");

function writeAgent(path) {
  writeFileSync(path, `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
while IFS= read -r line; do case "$line" in *'"type":"user"'*) prompt="$line"; break;; esac; done
case "$prompt" in
*permission*)
${emit(approval)}
;;
*)
${emit(events)}
;;
esac
`);
  chmodSync(path, 0o700);
}

const assets = buildApp();
const engine = await startEngine({ assets });
writeAgent(join(engine.root, "bin", "claude"));
mkdirSync(join(engine.root, "shots"), { recursive: true });
const shots = process.env.ZUSE_SHOTS ?? join(engine.root, "shots");
const page = await openBrowser(engine.url, { width: 1280, height: 860 });
const failures = [];
const results = {};
const check = (cond, what) => { if (!cond) failures.push(what); };
try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`);
  const device = await page.eval(`window.__zuse.state.localDeviceId`);
  const chatId = crypto.randomUUID();
  const config = { harness: "claude-code", sandbox: "workspace-write" };
  await engine.rpc("Mutate", { op: "createChat", chatId, deviceId: device, cwd: project, config });
  const run = (prompt) => engine.rpc("QueueCommand", {
    chatId, targetDeviceId: device,
    command: { kind: "run", messageId: crypto.randomUUID(), request: { prompt, harness: "claude-code", cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null, attachments: [] } },
  });
  await page.waitFor(`!!window.__zuse.state.chats[${JSON.stringify(chatId)}]`);
  // Open it the way a user does: its row in the sidebar.
  const row = `[...document.querySelectorAll('.app-sidebar button, .app-sidebar [role="option"]')].find((e) => e.textContent.trim().startsWith("New session"))`;
  await page.waitFor(`!!${row}`, 8000);
  await page.eval(`${row}.click()`);
  await page.waitFor(`window.__zuse.view.selectedChatId === ${JSON.stringify(chatId)}`, 8000).catch(async (error) => {
    await page.screenshot(join(shots, "live-sidebar.png"));
    console.error(await page.eval(`[...document.querySelectorAll('.app-sidebar [role="option"], .app-sidebar button')].map((e) => e.getAttribute("role") + ":" + e.textContent.slice(0, 40)).join("\\n")`));
    throw error;
  });
  await run("Bump the version in the README.");
  await page.waitFor(`document.querySelector(".t-entry[data-role='user']")?.textContent.includes("Bump the version")`);
  // Live: the turn's work shows as it happens.
  await page.waitFor(`!!document.querySelector(".tool-group, .tool-fold")`, 15000);
  await page.screenshot(join(shots, "live-streaming.png"));
  await page.waitFor(`document.querySelector(".t-entry[data-role='assistant'] .md table") !== null`, 20000);
  await page.waitFor(`window.__zuse.state.sessions[${JSON.stringify(chatId)}]?.status !== "working"`, 20000);
  await sleep(500);
  const fold = await page.eval(`document.querySelector(".tool-fold .tool-group-head")?.textContent ?? null`);
  check(fold && /3 tool calls/.test(fold), `settled turn folds its work (${fold})`);
  check(await page.eval(`!!document.querySelector(".code-block .code-body")`), "code block renders");
  check(await page.eval(`document.querySelector(".md a[href='https://example.com/docs']")?.getAttribute("rel") === "noopener noreferrer"`), "links open safely");
  await page.click(".tool-fold .tool-group-head");
  await sleep(200);
  await page.eval(`[...document.querySelectorAll(".tool-fold-body .tool-group-head[aria-expanded='false']")].forEach((b) => b.click())`);
  await sleep(200);
  const chips = await page.eval(`[...document.querySelectorAll(".tool-fold-body .chip-label")].map((e) => e.textContent)`);
  check(chips.includes("Run") && chips.includes("Read") && chips.includes("Edited"), `chips: ${chips.join(", ")}`);
  await page.eval(`[...document.querySelectorAll(".tool-fold-body button.chip-head")].forEach((b) => b.click())`);
  await sleep(300);
  await page.screenshot(join(shots, "live-settled.png"));
  const actions = await page.eval(`[...document.querySelectorAll(".t-entry[data-role='assistant'] .t-meta button")].map((b) => b.getAttribute("aria-label"))`);
  check(actions.includes("Copy message") && actions.includes("Fork chat"), `message actions: ${actions.join(", ")}`);
  // A second turn streams while pinned to the bottom.
  await run("And once more.");
  await page.waitFor(`document.querySelectorAll(".t-entry[data-role='assistant']").length >= 2`, 15000);
  await page.waitFor(`window.__zuse.state.sessions[${JSON.stringify(chatId)}]?.status !== "working"`, 20000);
  await sleep(500);
  const distance = await page.eval(`(() => { const s = document.querySelector(".t-scroller"); return s.scrollHeight - s.scrollTop - s.clientHeight })()`);
  check(distance < 4, `pinned to the latest reply (distance ${distance})`);
  await page.screenshot(join(shots, "live-second.png"));
  // Full tool output loads from the host on request.
  await page.eval(`[...document.querySelectorAll(".chip-more")].find((b) => b.textContent.includes("full output"))?.click()`);
  await page.waitFor(`[...document.querySelectorAll(".chip-output pre")].some((p) => p.textContent.includes("README.md"))`, 8000).catch(() => failures.push("full output did not load"));
  await page.eval(`[...document.querySelectorAll(".chip-more")].find((b) => b.textContent.includes("full diff"))?.click()`);
  await page.waitFor(`!!document.querySelector(".diff .diff-line[data-kind='add']")`, 8000).catch(() => failures.push("full diff did not load"));
  await page.screenshot(join(shots, "live-blobs.png"));

  // A permission request docks above the composer and answers through the engine.
  await run("Clean up, permission needed.");
  await page.waitFor(`!!document.querySelector(".t-permission")`, 15000).catch(() => failures.push("permission card did not appear"));
  await page.screenshot(join(shots, "live-permission.png"));
  const card = await page.eval(`document.querySelector(".t-permission")?.textContent ?? ""`);
  check(card.includes("Run shell command?") && card.includes("rm -rf build"), `permission card: ${card}`);
  await page.eval(`[...document.querySelectorAll(".t-permission button")].find((b) => b.textContent === "Allow once")?.click()`);
  await page.waitFor(`!document.querySelector(".t-permission")`, 8000).catch(() => failures.push("permission card stayed after answering"));
  await page.waitFor(`document.querySelector(".t-scroller").textContent.includes("Cleaned the build folder.")`, 15000).catch(() => failures.push("turn did not continue after approval"));
  await page.screenshot(join(shots, "live-approved.png"));

  // An attached image shows as a thumbnail read from the host.
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64");
  writeFileSync(join(project, "shot.png"), png);
  await run(`Look at this\n\nAttached images (local files — open them to view):\n- ${join(project, "shot.png")}`);
  await page.waitFor(`!!document.querySelector(".t-thumb")`, 15000).catch(() => failures.push("attachment thumbnail missing"));
  await page.waitFor(`!!document.querySelector(".t-thumb img")`, 8000).catch(() => failures.push("attachment image did not load from the host"));
  await page.waitFor(`window.__zuse.state.sessions[${JSON.stringify(chatId)}]?.status !== "working"`, 20000);

  // Fork from a reply opens the copy as a new tab of this thread.
  await page.eval(`(() => { const b = [...document.querySelectorAll(".t-meta button[aria-label='Fork chat']")].at(-1); b.id = "fork-target"; b.scrollIntoView({ block: "center" }); })()`);
  await sleep(200);
  await page.press("#fork-target");
  await page.waitFor(`[...document.querySelectorAll(".menu-item")].some((m) => m.textContent.includes("Fork in new tab"))`, 4000).catch(() => failures.push("fork menu did not open"));
  await page.eval(`[...document.querySelectorAll(".menu-item")].find((m) => m.textContent.includes("Fork in new tab")).id = "fork-item"`);
  await page.press("#fork-item");
  await page.waitFor(`window.__zuse.view.selectedChatId !== ${JSON.stringify(chatId)}`, 10000).catch(() => failures.push("fork did not open a new chat"));
  results.forkTabs = await page.eval(`document.querySelectorAll(".t-tab").length`);
  await page.screenshot(join(shots, "live-fork.png"));
  // `+` adds a conversation to the thread; closing a tab keeps it in history.
  await page.click(".t-strip-btn[aria-label='New chat in this thread']");
  await page.waitFor(`document.querySelectorAll(".t-tab").length >= ${(results.forkTabs ?? 1) + 1}`, 10000).catch(() => failures.push("new conversation tab did not appear"));
  results.tabsAfterAdd = await page.eval(`document.querySelectorAll(".t-tab").length`);
  await page.eval(`document.querySelector(".t-tab[data-active] .t-tab-close")?.click()`);
  await page.waitFor(`document.querySelectorAll(".t-tab").length === ${results.tabsAfterAdd} - 1`, 5000).catch(() => failures.push("closing a tab did not remove it"));
  await page.press(".t-strip-btn[aria-label='Earlier chats']");
  await page.waitFor(`document.querySelectorAll(".t-history-row").length === 1`, 4000).catch(() => failures.push("closed tab missing from Earlier chats"));
  await page.key("Escape", "Escape", 27);
  await page.screenshot(join(shots, "live-tabs.png"));

  check(page.errors.length === 0, `page errors: ${page.errors.join(" | ")}`);
  console.log(JSON.stringify({ fold, chips, actions, distance, ...results }, null, 2));
  if (failures.length) throw new Error(`failed:\n- ${failures.join("\n- ")}`);
  console.log("live ok; screenshots in", shots);
} finally {
  await page.close();
  await engine.close();
  rmSync(assets, { recursive: true, force: true });
  rmSync(project, { recursive: true, force: true });
}
