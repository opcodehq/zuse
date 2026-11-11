// Settings and Usage end to end in headless Chromium against a disposable engine.
// Usage: ZUSE_SHOTS=/tmp/dir node e2e/settings.mjs
// The engine runs with a temporary HOME and no user tool directories on PATH, so only fixture accounts, fixture
// usage logs and the fixture agent are visible. Nothing here installs or updates a real agent.
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const scratch = mkdtempSync(join(tmpdir(), "zuse-settings-"));
const home = join(scratch, "home");
const claudeDir = join(home, ".claude");
const project = join(scratch, "orbit");
mkdirSync(project, { recursive: true });
writeFileSync(join(project, "README.md"), "# Orbit\n");
const git = (...args) => execFileSync("git", args, { cwd: project, env: { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" } });
git("init", "-q", "-b", "main");
git("add", ".");
git("commit", "-qm", "init");

// Usage history: ten days of fixture records in the project.
const logs = join(claudeDir, "projects", "-orbit");
mkdirSync(logs, { recursive: true });
const rows = [];
for (let d = 0; d < 10; d++)
  rows.push({ type: "assistant", sessionId: `sess-${d}`, cwd: project, timestamp: new Date(Date.now() - d * 86400000).toISOString(), requestId: `r${d}`, message: { id: `m${d}`, model: "claude-sonnet-4-5", usage: { input_tokens: 1000 + d * 100, output_tokens: 300, cache_read_input_tokens: 5000, cache_creation_input_tokens: 200 } } });
writeFileSync(join(logs, "fixture.jsonl"), rows.map((r) => JSON.stringify(r)).join("\n") + "\n");
// One fixture Claude login. Its token is fake, so usage probes fail and the row shows why.
const identity = (n, email, extra = {}) => {
  writeFileSync(join(claudeDir, ".claude.json"), JSON.stringify({ userID: `fixture-${n}`, oauthAccount: { accountUuid: `${n}${n}${n}${n}${n}${n}${n}${n}-0000-4000-8000-000000000000`, emailAddress: email, organizationName: "Fixture Org", ...extra } }));
  writeFileSync(join(claudeDir, ".credentials.json"), JSON.stringify({ claudeAiOauth: { accessToken: `fixture-token-${n}`, refreshToken: `fixture-refresh-${n}`, expiresAt: Date.now() + 30 * 86400000, scopes: ["user:inference"] } }));
};
identity(1, "first@example.invalid");
mkdirSync(join(home, ".codex"), { recursive: true });

const assets = process.env.ZUSE_ASSETS ?? buildApp(join(scratch, "build"));
const saved = { ...process.env };
Object.assign(process.env, { HOME: home, CLAUDE_CONFIG_DIR: claudeDir, CODEX_HOME: join(home, ".codex"), XDG_CONFIG_HOME: join(home, ".config"), XDG_DATA_HOME: join(home, ".local/share"), XDG_CACHE_HOME: join(home, ".cache") });
process.env.PATH = process.env.PATH.split(":").filter((dir) => !dir.startsWith(saved.HOME)).join(":");
const engine = await startEngine({ assets, agent: { reply: "Fixture reply.", chunks: 1 } });
for (const key of ["HOME", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "PATH"]) {
  if (saved[key] === undefined) delete process.env[key];
  else process.env[key] = saved[key];
}

const page = await openBrowser("about:blank", { width: 1440, height: 900 });
await page.call("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => {
    const spy = (window.__spy = { banners: [], tones: 0, clips: [], perm: "default" });
    class FakeNotification { static get permission() { return spy.perm; } static requestPermission() { spy.perm = spy.nextPerm || "granted"; return Promise.resolve(spy.perm); } constructor(title, options) { spy.banners.push({ title, body: options?.body }); } close() {} }
    window.Notification = FakeNotification;
    const osc = AudioContext.prototype.createOscillator;
    AudioContext.prototype.createOscillator = function () { spy.tones++; return osc.call(this); };
    HTMLMediaElement.prototype.play = function () { spy.clips.push(this.src.split("/").pop()); return Promise.resolve(); };
    window.open = () => null;
  })()`,
});
await page.goto(engine.url);

let failures = 0;
const expect = (ok, message) => {
  if (!ok) throw new Error(message);
};
async function step(name, run) {
  try {
    await run();
    console.log(`ok   ${name}`);
  } catch (error) {
    failures++;
    console.log(`FAIL ${name}\n     ${error.message}`);
  }
}
const shot = async (name) => shots && (await page.screenshot(join(shots, `${name}.png`)));
const q = (s) => JSON.stringify(s);
const ctrl = (key, code, keyCode) => page.key(key, code, keyCode, 2);
const text = (selector) => page.eval(`document.querySelector(${q(selector)})?.innerText ?? null`);
/** A real pointer click on the visible element whose text or label matches; Kobalte menus act on pointer events. */
async function pressText(label, scope = "button, [role=option], [role=menuitem], [role=radio], [role=tab]") {
  const box = await page.eval(`(() => {
    const e = [...document.querySelectorAll(${q(scope)})].find((e) => (e.textContent.trim() === ${q(label)} || e.getAttribute("aria-label") === ${q(label)}) && e.getClientRects().length);
    if (!e) return null;
    e.scrollIntoView({ block: "center" });
    const r = e.getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
  })()`);
  expect(box, `nothing labelled ${label}`);
  for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await page.call("Input.dispatchMouseEvent", { type, x: box.x, y: box.y, button: "left", clickCount: 1 });
  await sleep(250);
}
const section = async (id) => {
  await page.click(`#settings-tab-${id}`);
  await sleep(500);
};
const run = async (chatId) => {
  const before = await page.eval(`window.__zuse.state.sessions[${q(chatId)}]?.lastCompletedTurn ?? null`);
  await engine.rpc("QueueCommand", { chatId, command: { kind: "run", messageId: `m-${Date.now()}`, request: { prompt: "hello", harness: "claude-code", model: null, reasoning: null, cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null } } });
  await page.waitFor(`(window.__zuse.state.sessions[${q(chatId)}]?.lastCompletedTurn ?? null) !== ${q(before)}`, 30000);
  await sleep(600);
};

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected"`, 30000);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "orbit", deviceId, path: project, name: "orbit", gitDetected: true });
  await engine.rpc("Mutate", { op: "createChat", chatId: "chat-1", spaceId: "orbit", branch: "main", config: { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" } });
  await engine.rpc("Mutate", { op: "renameChat", chatId: "chat-1", title: "Fixture chat" }).catch(() => {});
  await page.waitFor(`!!window.__zuse.state.chats["chat-1"]`);

  await step("Ctrl+, opens Settings; every section renders; arrows move between tabs; Esc closes", async () => {
    await ctrl(",", "Comma", 188);
    await page.waitFor(`!!document.querySelector(".st-page")`);
    for (const id of ["general", "providers", "defaults", "plugins", "devices", "cloud", "appearance", "notifications", "shortcuts", "archived"]) {
      await section(id);
      expect(await page.eval(`document.querySelector("#settings-tab-${id}").getAttribute("aria-selected") === "true"`), `${id} not selected`);
      expect(await page.eval(`document.querySelector(".st-pane").innerText.trim().length > 0`), `${id} empty`);
    }
    await page.eval(`document.querySelector("#settings-tab-archived").focus()`);
    await page.key("ArrowDown", "ArrowDown", 40);
    expect(await page.eval(`document.activeElement?.id === "settings-tab-general"`), "ArrowDown did not wrap to General");
    await page.key("End", "End", 35);
    expect(await page.eval(`window.__zuse.view.route.section === "archived"`), "End did not open the last section");
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".st-page")`);
  });

  await step("General: theme, language, completion sound and preview", async () => {
    await ctrl(",", "Comma", 188);
    await page.waitFor(`!!document.querySelector(".st-page")`);
    await section("general");
    await pressText("Light", "[role=radio]");
    expect(await page.eval(`window.__zuse.view.preferences.theme === "light"`), "theme not saved");
    await shot("general-light");
    await pressText("Dark", "[role=radio]");
    await pressText("Français", "[role=radio]");
    await page.waitFor(`document.querySelector(".st-title h1")?.textContent === "Général"`);
    await shot("general-french");
    await pressText("Système", "[role=radio]");
    await page.waitFor(`document.querySelector(".st-title h1")?.textContent === "General"`);
    const tones = await page.eval(`window.__spy.tones`);
    await pressText("Preview", "button");
    expect((await page.eval(`window.__spy.tones`)) > tones, "preview played nothing");
    await page.press(`button[aria-label="Completion sound preset"]`);
    await pressText("Bell", "[role=option]");
    expect(await page.eval(`window.__zuse.view.preferences.completionPreset === "bell"`), "preset not saved");
  });

  await step("General: thread naming and the worktree folder save through the engine", async () => {
    await page.press(`button[aria-label="Thread naming model"]`);
    await page.waitFor(`[...document.querySelectorAll('[role=option]')].length > 3`);
    const option = await page.eval(`[...document.querySelectorAll('[role=option]')].map((e) => e.textContent.trim()).find((t) => /^Haiku|^Sonnet/.test(t))`);
    expect(option, "no Claude models listed");
    await pressText(option, "[role=option]");
    await sleep(800);
    const naming = await engine.rpc("GetTitleSettings", {});
    expect(naming.harness === "claude-code" && naming.model, `naming ${JSON.stringify(naming)}`);
    await pressText("Follow thread agent", "button.st-action");
    await sleep(800);
    expect((await engine.rpc("GetTitleSettings", {})).harness === null, "follow thread agent not saved");

    const target = join(scratch, "worktrees");
    mkdirSync(join(target, "nested"), { recursive: true });
    await page.click(`button[aria-label="Use a custom worktree folder"]`);
    await page.eval(`(() => { const e = document.querySelector('input[aria-label="Worktree folder"]'); e.focus(); e.select(); })()`);
    await page.type(target);
    await pressText("Browse…", "button");
    await page.waitFor(`[...document.querySelectorAll('.st-folder')].some((e) => e.textContent.trim() === "nested")`);
    await pressText("Use this folder", "button");
    await pressText("Save", "button.st-action");
    await sleep(800);
    const saved = await engine.rpc("GetWorktreeSettings", {});
    expect(saved.effectiveDirectory === target, `worktrees ${JSON.stringify(saved)}`);
    await pressText("Reset to default", "button");
    await sleep(800);
    expect(!(await engine.rpc("GetWorktreeSettings", {})).settings.useCustomDirectory, "reset failed");
    await page.click(`button[aria-label="Use a custom worktree folder"]`);
    await page.eval(`(() => { const e = document.querySelector('input[aria-label="Worktree folder"]'); e.focus(); e.select(); })()`);
    await page.type("relative/path");
    await pressText("Save", "button.st-action");
    await sleep(800);
    expect(/absolute/i.test((await text(".st-error")) ?? ""), "relative path accepted");
    await page.click(`button[aria-label="Use a custom worktree folder"]`);
    expect(!(await page.eval(`!!document.querySelector(".st-error")`)), "error stayed after turning the folder off");
  });

  await step("Providers: accounts list, switch by row, policy and the last-agent rule", async () => {
    await section("providers");
    await page.waitFor(`document.querySelectorAll(".st-provider").length > 3`);
    await page.click(`button[aria-label="Claude Code preferences"]`);
    await page.waitFor(`document.querySelectorAll(".st-account-row").length === 1`, 20000);
    identity(2, "second@example.invalid", { displayName: "Second Person" });
    await page.click(`button[aria-label="Refresh accounts"]`);
    await page.waitFor(`document.querySelectorAll(".st-account-row").length === 2`, 20000);
    await sleep(500);
    await shot("providers-accounts");
    await page.press(`.st-account-row[data-switchable]`);
    await page.waitFor(`document.querySelector('.st-account-row[data-active]')?.innerText.includes("f•••@example.invalid")`, 20000);
    expect(!(await page.eval(`window.__zuse && document.querySelector(".st-provider-body .st-dialog-error")`)), "switch reported an error");

    await pressText("Agent options", "button");
    await page.press(`button[aria-label="Update policy"]`);
    await pressText("Off", "[role=option]");
    await sleep(800);
    const policy = async () => (await engine.rpc("WatchHarnessUpdates", {})).find((u) => u.harness === "claude-code")?.policy;
    expect((await policy()) === "off", "policy not saved");
    await page.press(`button[aria-label="Update policy"]`);
    await pressText("Notify", "[role=option]");
    await sleep(800);
    expect((await policy()) === "notify", "policy not restored");

    await page.click(`button[role=switch][aria-label="Zuse"]`);
    await sleep(800);
    expect(await page.eval(`document.querySelector('button[role=switch][aria-label="Claude Code"]').disabled`), "last enabled agent can be turned off");
    await page.click(`button[role=switch][aria-label="Zuse"]`);
    await sleep(800);
    expect((await engine.rpc("ListHarnesses", {})).find((h) => h.id === "zuse").enabled, "Zuse not re-enabled");
  });

  await step("Custom ACP agent: validation, save with literal arguments, failed test, remove", async () => {
    await pressText("Add", "button");
    await page.waitFor(`!!document.querySelector(".dialog")`);
    await pressText("Custom command…", "button");
    await pressText("Save", ".dialog button");
    expect(/name/i.test((await text(".dialog .st-dialog-error")) ?? ""), "empty form accepted");
    const fill = async (placeholder, value) => {
      await page.eval(`document.querySelector('.dialog [placeholder=${q(placeholder)}]').focus()`);
      await page.type(value);
    };
    await fill("My agent", "Broken agent");
    await fill("my-agent", "/bin/false");
    await fill("--acp", `--acp --label 'two words'`);
    await pressText("Save", ".dialog button");
    await page.waitFor(`!document.querySelector(".dialog")`);
    const agents = (await engine.rpc("GetCustomAcpAgents", {})).agents;
    expect(agents.length === 1 && agents[0].args.join("|") === "--acp|--label|two words", `agents ${JSON.stringify(agents)}`);
    await page.waitFor(`[...document.querySelectorAll(".st-provider-head")].some((e) => e.innerText.includes("Broken agent") && /error|unavailable/i.test(e.innerText))`, 30000);
    await shot("providers-custom");
    await page.press(`button[aria-label="Actions for Broken agent"]`);
    await pressText("Remove", "[role=menuitem]");
    await pressText("Remove", ".dialog button");
    await sleep(800);
    expect((await engine.rpc("GetCustomAcpAgents", {})).agents.length === 0, "agent not removed");
  });

  await step("Default models: model and permission mode feed the composer's new-chat defaults", async () => {
    await section("defaults");
    await page.press(`button[aria-label="Default model"]`);
    await page.waitFor(`[...document.querySelectorAll('[role=option]')].length > 3`);
    const option = await page.eval(`[...document.querySelectorAll('[role=option]')].map((e) => e.textContent.trim()).find((t) => /^Haiku/.test(t))`);
    await pressText(option, "[role=option]");
    await page.press(`button[aria-label="Default permission mode"]`);
    await pressText("Full accessRuns actions without approval.", "[role=option]");
    const defaults = JSON.parse(await page.eval(`JSON.stringify(window.__zuse.view.preferences.composerDefaults)`));
    expect(defaults.harness === "claude-code" && /haiku/.test(defaults.models["claude-code"]?.id) && defaults.sandbox === "danger-full-access", `defaults ${JSON.stringify(defaults)}`);
    await shot("defaults");
  });

  await step("Notifications: permission prompt, banner and chime on a finished run, switches silence them", async () => {
    await section("notifications");
    await page.eval(`window.__spy.nextPerm = "denied"`);
    await page.click(`button[role=switch][aria-label="Desktop notifications"]`);
    await sleep(300);
    expect(await page.eval(`document.querySelector('button[role=switch][aria-label="Desktop notifications"]').disabled`), "denied permission left the switch active");
    await page.eval(`window.__spy.perm = "default"; window.__spy.nextPerm = "granted"`);
    await page.goto(engine.url.replace(/#.*/, ""));
    await page.waitFor(`window.__zuse?.state.connection.kind === "connected"`);
    await ctrl(",", "Comma", 188);
    await page.waitFor(`!!document.querySelector(".st-page")`);
    await section("notifications");
    await page.eval(`window.__spy.perm = "default"; window.__spy.nextPerm = "granted"`);
    await page.click(`button[role=switch][aria-label="Desktop notifications"]`);
    await sleep(300);
    // Headless Chromium always reports focus, so banners are checked with background-only off.
    await page.click(`button[role=switch][aria-label="Only notify when Zuse is in the background"]`);
    await shot("notifications");
    const tones = await page.eval(`window.__spy.tones`);
    await run("chat-1");
    const after = await page.eval(`({ tones: window.__spy.tones - ${tones}, banners: window.__spy.banners })`);
    expect(after.tones === 2 && after.banners.length === 1 && after.banners[0].body === "Finished", `after run ${JSON.stringify(after)}`);
    await page.click(`button[role=switch][aria-label="Session sounds"]`);
    const silent = await page.eval(`window.__spy.tones`);
    await run("chat-1");
    expect((await page.eval(`window.__spy.tones`)) === silent, "sounds played with session sounds off");
    await page.click(`button[role=switch][aria-label="Session sounds"]`);
  });

  await step("Usage: dashboard loads fixture history; chart keys, sessions, provider filter, Esc closes", async () => {
    await page.key("Escape", "Escape", 27);
    await page.eval(`(() => { const v = window.__zuse.view; return v.route.kind })()`);
    await page.press(`button[aria-label="Usage limits"]`);
    await pressText("Full usage", "[role=menuitem]");
    await page.waitFor(`!!document.querySelector(".us-root .us-total")`, 30000);
    await page.eval(`document.querySelector(".us-chart").focus()`);
    await page.key("Home", "Home", 36);
    await page.key("ArrowRight", "ArrowRight", 39);
    expect(/View day/.test((await text(".us-foot")) ?? ""), "chart keys did not pick a day");
    await page.eval(`document.querySelector(".us-sessions-toggle").click()`);
    await page.waitFor(`document.querySelectorAll("#usage-sessions tbody tr").length > 1`, 20000);
    expect((await text(`button[aria-label="Provider"]`))?.includes("All providers"), "provider filter shows no choice");
    await shot("usage");
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`);
  });

  const pageErrors = page.errors.filter((e) => !/Failed to load resource/.test(e));
  await step("No page errors", async () => expect(pageErrors.length === 0, pageErrors.join("\n")));
} finally {
  await page.close();
  await engine.close();
  rmSync(scratch, { recursive: true, force: true });
}
console.log(failures ? `\n${failures} failed` : "\nall passed");
process.exit(failures ? 1 : 0);
