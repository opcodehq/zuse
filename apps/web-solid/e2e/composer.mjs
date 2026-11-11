// Composer end to end against a disposable engine and a scripted agent (e2e/composer-fixture.mjs).
// The engine gets a temporary HOME and a PATH without user tool directories, so no real agent can run.
// Usage: node e2e/composer.mjs   (ZUSE_SHOTS=/dir saves screenshots, ZUSE_ASSETS reuses a build)
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { agentPath, writeComposerAgent } from "./composer-fixture.mjs";

const shots = process.env.ZUSE_SHOTS;
const scratch = mkdtempSync(join(tmpdir(), "zuse-cmp-"));
const project = join(scratch, "demo");
mkdirSync(join(project, "src"), { recursive: true });
writeFileSync(join(project, "src/main.rs"), "fn main() {}\n");
writeFileSync(join(project, "README.md"), "# demo\n");
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: project });
execFileSync("git", ["add", "."], { cwd: project });
execFileSync("git", ["commit", "-qm", "init"], { cwd: project, env: gitEnv });
const image = join(scratch, "shot.png");
writeFileSync(image, Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==", "base64"));
const big = join(scratch, "big.png");
writeFileSync(big, "");
truncateSync(big, 33 * 1024 * 1024);
mkdirSync(join(scratch, "home"));
process.env.ZUSE_WORKTREES_DIR = join(scratch, "worktrees");
process.env.PATH = "/usr/local/bin:/usr/bin:/bin";

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets, env: { HOME: join(scratch, "home") } });
writeComposerAgent(agentPath(engine.root));
const page = await openBrowser(engine.url, { width: 1280, height: 820 });
// Lets the test drop the engine socket and hold reconnects off, and keeps sign-in pages from opening.
await page.call("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => { const Real = window.WebSocket; window.__sockets = []; window.__blockWS = false; window.WebSocket = class extends Real { constructor(url, p) { super(window.__blockWS ? "ws://127.0.0.1:9/blocked" : url, p); window.__sockets.push(this); } }; window.__dropWS = () => { window.__blockWS = true; window.__sockets.forEach((s) => s.close()); }; window.open = (url) => { (window.__opened ??= []).push(url); return null; }; })();`,
});

// The stubs only reach documents loaded after they are registered.
await page.call("Page.reload", { ignoreCache: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const q = JSON.stringify;
const E = (expression) => page.eval(expression);
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  await fn();
  console.log("ok");
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `composer-${name}.png`)));
const expect = (value, what) => {
  if (!value) throw new Error(what);
};
const input = `document.querySelector(".cmp-input")`;
const value = () => E(`${input}.value`);
const focusInput = () => E(`(() => { ${input}.focus(); return true })()`);
const clearInput = async () => {
  await E(`(() => { const t = ${input}; t.focus(); t.select(); return true })()`);
  await page.key("Backspace", "Backspace", 8);
};
const pills = () => E(`[...document.querySelectorAll(".cmp-pill")].map((e) => e.innerText.split("\\n").join(" | "))`);
const pick = async (selector, text) => {
  await page.waitFor(`[...document.querySelectorAll(${q(selector)})].some((e) => e.innerText.includes(${q(text)}))`);
  await E(`(() => { document.querySelectorAll("[data-cmp-test]").forEach((e) => e.removeAttribute("data-cmp-test")); const e = [...document.querySelectorAll(${q(selector)})].find((e) => e.innerText.includes(${q(text)})); if (!e) throw new Error("missing " + ${q(text)}); e.setAttribute("data-cmp-test", ""); return true })()`);
  await page.press("[data-cmp-test]");
};
const enter = (modifiers = 0) => page.key("Enter", "Enter", 13, modifiers);
const chatId = () => E(`window.__zuse.view.selectedChatId`);
const status = (id) => `window.__zuse.state.sessions[${q(id)}]?.status`;
const setFiles = async (files) => {
  const doc = await page.call("DOM.getDocument", {});
  const node = await page.call("DOM.querySelector", { nodeId: doc.result.root.nodeId, selector: ".cmp-root input[type=file]" });
  await page.call("DOM.setFileInputFiles", { nodeId: node.result.nodeId, files });
};

try {
  await page.waitFor(`typeof window.__dropWS === "function" && window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`);
  const deviceId = await E(`window.__zuse.state.localDeviceId`);

  await step("onboarding offers a project or a project-less start", async () => {
    await page.waitFor(`document.querySelector(".cmp-onboarding-title")?.textContent === "Add a project to get started"`);
    await shot("onboarding");
    await page.press(".cmp-onboarding .cmp-link");
    await page.waitFor(`document.querySelector(".cmp-heading")?.textContent === "What should we build today?" && document.activeElement === ${input}`);
  });

  await step("project chip searches and picks by keyboard", async () => {
    await engine.rpc("Mutate", { op: "createSpace", spaceId: "cmp-space", deviceId, path: project, name: "demo", gitDetected: true });
    await page.waitFor(`Object.keys(window.__zuse.state.spaces).length === 1`);
    await page.press(`button[aria-label="Project"]`);
    await page.waitFor(`document.activeElement?.placeholder === "Search projects…"`);
    await page.type("dem");
    await enter();
    await page.waitFor(`document.querySelector(".cmp-heading")?.textContent === "What should we build in demo?" && document.activeElement === ${input}`);
  });

  await step("access, plan and model choices shape the new chat", async () => {
    await page.press(`[aria-label^="Permission mode"]`);
    await pick("[role=menuitemradio]", "Full access");
    await page.waitFor(`document.querySelector('[aria-label^="Permission mode"]').innerText.includes("Full access")`);
    await page.press(`[aria-label="Add to message"]`);
    await pick("[role^=menuitem]", "Plan mode");
    await page.waitFor(`!!document.querySelector('[aria-label="Exit plan mode"]')`);
    await focusInput();
    await page.key("p", "KeyP", 80, 1);
    await page.waitFor(`document.activeElement?.getAttribute("aria-label") === "Search models"`);
    await page.type("sonnet");
    await page.waitFor(`document.querySelector(".cmp-model-row[data-active=true]")?.innerText.startsWith("Sonnet")`, 20000).catch(async (error) => {
      console.log(await E(`JSON.stringify({ picker: document.querySelector(".cmp-models")?.innerText, active: document.activeElement?.outerHTML.slice(0, 120), value: document.activeElement?.value })`));
      throw error;
    });
    await enter();
    await page.waitFor(`document.querySelector(".cmp-model")?.innerText.startsWith("Sonnet") && document.activeElement === ${input}`);
    await shot("landing");
    await page.type("hello project");
    await enter();
    await page.waitFor(`!!window.__zuse.view.selectedChatId`);
    const id = await chatId();
    await page.waitFor(`window.__zuse.state.chats[${q(id)}]?.config?.model === "claude-sonnet-5"`);
    const config = await E(`JSON.stringify(window.__zuse.state.chats[${q(id)}].config)`);
    expect(config.includes(`"sandbox":"danger-full-access"`) && config.includes(`"plan":true`), `config ${config}`);
  });

  await step("the reply streams in and the context ring reads it", async () => {
    await page.waitFor(`window.__zuse.state.transcript?.messages.some((m) => m.role === "assistant" && m.parts.some((p) => p.text === "Fixture reply."))`, 20000);
    await page.waitFor(`document.querySelector(".cmp-ring")?.getAttribute("aria-label") === "Context: 40K / 200K, 20% used"`);
    await page.press(".cmp-ring");
    await page.waitFor(`document.querySelector(".cmp-usage-card")?.innerText.includes("Available")`);
    await page.key("Escape", "Escape", 27);
  });

  await step("@ mentions search the project and send as file links", async () => {
    await focusInput();
    await page.type("check @mai");
    await page.waitFor(`[...document.querySelectorAll(".cmp-popup [role=option]")].some((e) => e.innerText.includes("main.rs"))`);
    await enter();
    expect((await value()) === "check @src/main.rs ", "mention inserted");
    await enter();
    await page.waitFor(`window.__zuse.state.transcript.messages.some((m) => m.role === "user" && m.parts.some((p) => p.text?.includes("(zuse-file:src/main.rs)")))`);
  });

  await step("/ lists agent commands and Zuse commands; /model opens the picker", async () => {
    await page.waitFor(`${status(await chatId())} !== "working"`, 20000);
    await focusInput();
    await page.type("/");
    await page.waitFor(`[...document.querySelectorAll(".cmp-popup [role=option]")].some((e) => e.innerText.startsWith("/review"))`, 20000);
    await shot("slash");
    await page.type("mod");
    await enter();
    await page.waitFor(`!!document.querySelector(".cmp-models")`);
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".cmp-models") && document.activeElement === ${input}`);
  });

  await step("a live run queues, reorders, edits, removes and stops", async () => {
    const id = await chatId();
    await clearInput();
    await page.type("please go slowly");
    await enter();
    await page.waitFor(`${status(id)} === "working"`);
    await page.waitFor(`document.querySelector(".cmp-stop")?.disabled === false`);
    for (const text of ["alpha", "bravo", "charlie"]) {
      await page.type(text);
      expect((await E(`document.querySelector(".cmp-send")?.getAttribute("aria-label")`)) === "Queue message", "queue label");
      await enter();
      await sleep(300);
    }
    await page.waitFor(`window.__zuse.state.queue[${q(id)}]?.length === 3`);
    await E(`document.querySelector(".cmp-queue-handle").focus()`);
    await page.key("ArrowDown", "ArrowDown", 40, 1);
    await page.waitFor(`window.__zuse.state.queue[${q(id)}][1]?.text === "alpha" && document.activeElement?.closest(".cmp-queue-row")?.innerText.startsWith("alpha")`);
    await E(`[...document.querySelectorAll(".cmp-queue-row")].find((r) => r.innerText.includes("charlie")).querySelector('[aria-label="Remove"]').click()`);
    await page.waitFor(`window.__zuse.state.queue[${q(id)}].length === 2`);
    await E(`[...document.querySelectorAll(".cmp-queue-row")].find((r) => r.innerText.includes("bravo")).querySelector('[aria-label="Edit"]').click()`);
    await page.waitFor(`${input}.value === "bravo" && document.activeElement === ${input} && !!document.querySelector(".cmp-queue-row[data-editing=true]")`);
    await shot("queue-editing");
    await page.type(" edited");
    await enter();
    await page.waitFor(`window.__zuse.state.queue[${q(id)}]?.[0]?.text === "bravo edited" && ${input}.value === ""`);
    await page.key("Escape", "Escape", 27);
    await sleep(100);
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`${status(id)} !== "working"`, 20000);
    // A stopped run holds the queue; the row's primary action delivers it and the rest follow.
    await page.waitFor(`window.__zuse.state.queue[${q(id)}]?.length === 2`);
    await page.press(`.cmp-queue-row [data-primary="true"]`);
    await page.waitFor(`!window.__zuse.state.queue[${q(id)}]?.length`, 30000);
  });

  await step("the next run gets a working Stop key", async () => {
    const id = await chatId();
    await page.waitFor(`${status(id)} !== "working" && !window.__zuse.state.queue[${q(id)}]?.length`, 30000);
    await page.type("please go slowly");
    await enter();
    await page.waitFor(`${status(id)} === "working"`);
    await page.waitFor(`document.querySelector(".cmp-stop")?.disabled === false`);
    await page.press(".cmp-stop");
    await page.waitFor(`${status(id)} !== "working"`, 20000);
  });

  await step("provider errors show a specific row with Retry and Dismiss", async () => {
    await focusInput();
    await page.type("hit the usagelimit");
    await enter();
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill")].some((e) => e.innerText.startsWith("Usage limit reached"))`, 20000);
    await shot("usage-error");
    const before = await E(`window.__zuse.state.transcript.messages.length`);
    await E(`[...document.querySelectorAll(".cmp-pill button")].find((b) => b.innerText === "Retry").click()`);
    await page.waitFor(`window.__zuse.state.transcript.messages.length > ${before} && window.__zuse.state.transcript.messages.filter((m) => m.role === "user").at(-1).parts[0].text === "hit the usagelimit"`, 20000);
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill")].some((e) => e.innerText.startsWith("Usage limit reached"))`, 20000);
    await E(`document.querySelector('.cmp-pill [aria-label="Dismiss"]').click()`);
    await page.waitFor(`!document.querySelector(".cmp-pill")`);
  });

  await step("a sign-in error offers the agent's sign-in and can be cancelled", async () => {
    await page.type("now authfail");
    await enter();
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill")].some((e) => e.innerText.startsWith("Signed out of Claude Code"))`, 20000);
    await E(`[...document.querySelectorAll(".cmp-pill button")].find((b) => b.innerText === "Sign in").click()`);
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill button")].some((b) => b.innerText.includes("Open browser again"))`, 20000);
    expect((await E(`window.__opened?.length`)) >= 1, "sign-in page opened");
    await E(`[...document.querySelectorAll(".cmp-pill button")].find((b) => b.innerText === "Cancel").click()`);
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill button")].some((b) => b.innerText === "Sign in")`);
  });

  await step("attachments upload from the picker, paste and drop; oversize files are refused", async () => {
    await setFiles([image]);
    await E(`(() => { const dt = new DataTransfer(); dt.items.add(new File(["notes"], "notes.txt", { type: "text/plain" })); ${input}.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true })); return true })()`);
    await E(`(() => { const dt = new DataTransfer(); dt.items.add(new File(["x"], "drop.txt", { type: "text/plain" })); document.querySelector(".cmp-column").dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true })); return true })()`);
    await page.waitFor(`document.querySelectorAll(".cmp-thumb[data-state=ready]").length === 3`);
    await setFiles([big]);
    await page.waitFor(`!!document.querySelector(".cmp-thumb[data-state=error]")`);
    await shot("attachments");
    await focusInput();
    await page.type("look at these");
    await enter();
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill")].some((e) => e.innerText.startsWith("Couldn't send"))`);
    await E(`document.querySelector(".cmp-thumb[data-state=error] .cmp-thumb-remove").click()`);
    await enter();
    await page.waitFor(`window.__zuse.state.transcript.messages.some((m) => m.role === "user" && m.parts.some((p) => p.text?.includes("Attached images (local files") && p.text.includes("notes.txt") && p.text.includes("shot.png")))`);
  });

  await step("Ctrl+S stashes a draft and the stash restores it; Alt+R searches sent messages", async () => {
    await page.waitFor(`${status(await chatId())} !== "working"`, 20000);
    await clearInput();
    await page.type("an idea for later");
    await page.key("s", "KeyS", 83, 2);
    await page.waitFor(`${input}.value === "" && document.querySelector(".cmp-stash")?.innerText.trim() === "1"`);
    await page.key("s", "KeyS", 83, 2);
    await page.waitFor(`document.querySelectorAll(".cmp-list-row").length === 1`);
    await enter();
    await page.waitFor(`${input}.value === "an idea for later" && !document.querySelector(".cmp-stash")`);
    await clearInput();
    await page.key("r", "KeyR", 82, 1);
    await page.type("project");
    await page.waitFor(`[...document.querySelectorAll(".cmp-list-row")].some((e) => e.innerText.startsWith("hello project"))`);
    await shot("history");
    await enter();
    await page.waitFor(`${input}.value === "hello project"`);
  });

  await step(":shortcodes complete to emoji", async () => {
    await clearInput();
    await page.type("nice :thu");
    await page.waitFor(`document.querySelector(".cmp-emoji [role=option]")?.innerText.includes(":thumbsup:")`);
    await enter();
    expect((await value()) === "nice 👍", "emoji accepted");
  });

  await step("losing the engine keeps the draft and says so; reconnecting clears it", async () => {
    await clearInput();
    await E(`window.__dropWS()`);
    await page.waitFor(`window.__zuse.state.connection.kind === "reconnecting"`);
    // The window's connection strip says it once; the composer does not repeat it.
    await page.waitFor(`document.querySelector(".cx-banner")?.innerText.includes("Reconnecting")`);
    expect(!(await E(`[...document.querySelectorAll(".cmp-pill")].some((e) => e.innerText.startsWith("Reconnecting"))`)), "one reconnecting indicator");
    await focusInput();
    await page.type("while offline");
    await enter();
    await page.waitFor(`[...document.querySelectorAll(".cmp-pill")].some((e) => e.innerText.startsWith("Engine not connected"))`);
    expect((await value()) === "while offline", "draft kept");
    await shot("offline");
    await E(`window.__blockWS = false`);
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await page.waitFor(`!document.querySelector(".cmp-pill")`);
    await focusInput();
    await enter();
    await page.waitFor(`window.__zuse.state.transcript?.messages.some((m) => m.role === "user" && m.parts.some((p) => p.text === "while offline"))`);
  });

  await step("an archived chat shows the archive footer and unarchives", async () => {
    const id = await chatId();
    await engine.rpc("Mutate", { op: "setChatArchived", chatId: id, archived: true });
    await page.waitFor(`document.querySelector(".cmp-archived")?.innerText.includes("This chat is archived.")`);
    await page.press(`[aria-label="Unarchive thread"]`);
    await page.waitFor(`!document.querySelector(".cmp-archived") && window.__zuse.state.chats[${q(id)}].archived === false`);
  });

  await step("drafts are stored per chat", async () => {
    await page.waitFor(`${status(await chatId())} !== "working"`, 20000);
    await clearInput();
    await page.type("keep me");
    await sleep(400);
    expect((await E(`localStorage.getItem("zuse.web.composer.drafts.v1")`)).includes("keep me"), "draft stored");
  });

  if (page.errors.length) throw new Error(`page errors:\n${page.errors.join("\n")}`);
  console.log("composer ok");
} finally {
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(scratch, { recursive: true, force: true });
}
