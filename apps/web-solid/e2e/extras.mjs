// Extras end to end: palette, shortcut help, import chat, visuals, voice failure, notifications, toasts and
// reconnects, in headless Chromium against a disposable engine. Usage: ZUSE_SHOTS=/tmp/dir node e2e/extras.mjs
// The engine runs with a temporary HOME and no user tool directories on PATH, so no real agent or voice runtime is reachable.
import { buildApp, openBrowser, repo, startEngine } from "./harness.mjs";
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const git = (cwd, ...args) => execFileSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" } });

const scratch = mkdtempSync(join(tmpdir(), "zuse-extras-"));
const project = join(scratch, "orbit");
mkdirSync(join(project, "src"), { recursive: true });
writeFileSync(join(project, "README.md"), "# Orbit\n\nA fixture project.\n");
writeFileSync(join(project, "src", "launch.ts"), "export function launchSequence() {\n  return 'ignition';\n}\n");
writeFileSync(join(project, "src", "telemetry.ts"), "// telemetry\nexport const ignitionDelay = 3;\n");
git(project, "init", "-q", "-b", "main");
git(project, "add", ".");
git(project, "commit", "-qm", "Initial commit");

// Provider history the import dialog reads: one importable conversation in the fixture project.
const home = join(scratch, "home");
const claudeDir = join(home, ".claude");
const historyDir = join(claudeDir, "projects", "-orbit");
mkdirSync(historyDir, { recursive: true });
const session = "8f3c2a1e-0000-4000-8000-000000000001";
const now = new Date().toISOString();
writeFileSync(join(historyDir, `${session}.jsonl`), [
  { type: "user", sessionId: session, cwd: project, uuid: "u1", timestamp: now, message: { role: "user", content: "Plan the telemetry dashboard" } },
  { type: "assistant", sessionId: session, cwd: project, uuid: "a1", timestamp: now, message: { role: "assistant", content: [{ type: "text", text: "Here is a plan for the dashboard." }] } },
  { type: "summary", summary: "Telemetry dashboard plan" },
].map((row) => JSON.stringify(row)).join("\n") + "\n");
mkdirSync(join(home, ".codex"), { recursive: true });

const savedEnv = { HOME: process.env.HOME, PATH: process.env.PATH, CLAUDE_CONFIG_DIR: process.env.CLAUDE_CONFIG_DIR, CODEX_HOME: process.env.CODEX_HOME };
const assets = process.env.ZUSE_ASSETS ?? buildApp(join(scratch, "build"));
const userHome = process.env.HOME;
process.env.HOME = home;
process.env.CLAUDE_CONFIG_DIR = claudeDir;
process.env.CODEX_HOME = join(home, ".codex");
process.env.PATH = process.env.PATH.split(":").filter((dir) => !dir.startsWith(userHome)).join(":");
const engine = await startEngine({ assets, agent: { reply: "Fixture reply.", chunks: 1 } });
Object.assign(process.env, Object.fromEntries(Object.entries(savedEnv).filter(([, v]) => v !== undefined)));
const enginePid = Number(execFileSync("pgrep", ["-f", join(engine.root, "profile")], { encoding: "utf8" }).trim().split("\n")[0]);

const page = await openBrowser(engine.url, { width: 1440, height: 900, args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream", "--autoplay-policy=no-user-gesture-required"] });
await page.call("Emulation.setFocusEmulationEnabled", { enabled: true });
// Record banners and sounds instead of producing them.
await page.call("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => {
    const spy = (window.__spy = { banners: [], tones: 0, files: 0 });
    class FakeNotification { static permission = "granted"; static requestPermission() { return Promise.resolve("granted"); } constructor(title, options) { spy.banners.push({ title, body: options?.body }); spy.lastBanner = this; } close() {} }
    window.Notification = FakeNotification;
    const osc = AudioContext.prototype.createOscillator;
    spy.log = [];
    AudioContext.prototype.createOscillator = function () {
      spy.tones++;
      const sessions = window.__zuse?.state.sessions ?? {};
      spy.log.push(JSON.stringify(Object.fromEntries(Object.entries(sessions).map(([id, s]) => [id, [s.status, s.lastCompletedTurn, s.updatedAt]]))));
      return osc.call(this);
    };
    const src = AudioContext.prototype.createBufferSource;
    AudioContext.prototype.createBufferSource = function () { spy.files++; return src.call(this); };
  })();`,
});
await page.call("Page.reload", {});
await sleep(500);

let respawned = null;
const results = [];
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  try {
    await fn();
    results.push([name, true]);
    console.log("ok");
  } catch (error) {
    results.push([name, false, error.message]);
    console.log(`FAIL\n    ${error.message.split("\n").join("\n    ")}`);
  }
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `${name}.png`)));
const expect = (cond, what) => { if (!cond) throw new Error(what); };
const CTRL = 2;
const SHIFT = 8;
const key = (k, code, keyCode, mods = 0) => page.key(k, code, keyCode, mods);
const esc = () => key("Escape", "Escape", 27);
const ctrl = (k, code, keyCode, mods = 0) => key(k, code, keyCode, CTRL | mods);
const text = (sel) => page.eval(`document.querySelector(${JSON.stringify(sel)})?.textContent ?? null`);
const all = (sel) => page.eval(`[...document.querySelectorAll(${JSON.stringify(sel)})].map((e) => e.textContent.trim())`);
const focused = () => page.eval(`(() => { const a = document.activeElement; return a ? (a.getAttribute("aria-label") || a.className || a.tagName) : null })()`);
const paletteOpen = () => page.eval(`!!document.querySelector(".x-palette")`);
const setTheme = (theme) => page.eval(`(() => { const p = JSON.parse(localStorage.getItem("zuse.web.preferences.v1") || "{}"); p.theme = ${JSON.stringify(theme)}; localStorage.setItem("zuse.web.preferences.v1", JSON.stringify(p)); return true })()`);

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`, 30000);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "orbit", deviceId, path: project, name: "orbit", gitDetected: true });
  const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" };
  const chats = [
    ["chat-launch", "Launch checklist review"],
    ["chat-telemetry", "Telemetry ingestion bug"],
    ["chat-docs", "Write onboarding docs"],
    ["chat-old", "Archived experiment"],
  ];
  for (const [chatId, title] of chats) {
    await engine.rpc("Mutate", { op: "createChat", chatId, spaceId: "orbit", branch: "main", config });
    await engine.rpc("Mutate", { op: "renameChat", chatId, title }).catch(() => engine.rpc("Mutate", { op: "setChatTitle", chatId, title }));
  }
  await engine.rpc("Mutate", { op: "setChatArchived", chatId: "chat-old", archived: true });
  await page.waitFor(`Object.keys(window.__zuse.state.chats).length >= 4`);
  const titles = await page.eval(`Object.values(window.__zuse.state.chats).map((c) => c.title)`);
  console.log("chat titles:", titles);
  await sleep(500);
  await shot("00-frame");

  await step("Ctrl+K opens the palette on All with the search box focused", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    expect((await page.eval(`document.activeElement?.classList.contains("x-palette-input")`)), "input not focused");
    expect((await page.eval(`document.querySelector('.x-palette-tab[aria-selected="true"]')?.textContent`)).startsWith("All"), "All tab not selected");
    const heads = await all(".x-section-head > span:first-child");
    expect(heads.includes("Quick Actions") && heads.includes("Recent Threads"), `sections: ${heads}`);
    const recent = await all(".x-row-title");
    expect(!recent.some((t) => t.includes("Archived experiment")), "archived thread in recent");
    await shot("01-palette-all-dark");
  });

  await step("Arrow keys move the highlight and wrap", async () => {
    const first = await page.eval(`document.querySelector('.x-row[aria-selected="true"]')?.id`);
    await key("ArrowDown", "ArrowDown", 40);
    const second = await page.eval(`document.querySelector('.x-row[aria-selected="true"]')?.id`);
    expect(first && second && first !== second, "highlight did not move");
    await key("ArrowUp", "ArrowUp", 38);
    await key("ArrowUp", "ArrowUp", 38);
    const last = await page.eval(`(() => { const rows = [...document.querySelectorAll(".x-row")]; return rows.at(-1)?.getAttribute("aria-selected") })()`);
    expect(last === "true", "ArrowUp from the top did not wrap to the last row");
    const desc = await page.eval(`document.querySelector(".x-palette-input").getAttribute("aria-activedescendant") === document.querySelector('.x-row[aria-selected="true"]').id`);
    expect(desc, "aria-activedescendant out of sync");
  });

  await step("Tab and Ctrl+number switch tabs; Commands lists commands only", async () => {
    await key("Tab", "Tab", 9);
    expect((await text('.x-palette-tab[aria-selected="true"]')).startsWith("Threads"), "Tab did not go to Threads");
    await shot("02-palette-threads");
    await key("Tab", "Tab", 9, SHIFT);
    expect((await text('.x-palette-tab[aria-selected="true"]')).startsWith("All"), "Shift+Tab did not return to All");
    await ctrl("3", "Digit3", 51);
    expect((await text('.x-palette-tab[aria-selected="true"]')).startsWith("Commands"), "Ctrl+3 did not open Commands");
    const labels = await all(".x-row-label");
    expect(labels.includes("New chat") && labels.includes("Keyboard shortcuts"), `commands: ${labels}`);
    await shot("03-palette-commands");
    await page.click(".x-palette-tab:nth-child(1)");
    expect((await text('.x-palette-tab[aria-selected="true"]')).startsWith("All"), "clicking All tab failed");
    expect(await page.eval(`document.activeElement?.classList.contains("x-palette-input")`), "focus left input after tab click");
  });

  await step("Search finds a thread by title, highlights the match, Enter opens it", async () => {
    await page.type("telemetry");
    await page.waitFor(`[...document.querySelectorAll(".x-row-title")].some((e) => e.textContent.includes("Telemetry ingestion"))`);
    const hit = await page.eval(`document.querySelector(".x-row-title .x-hit")?.textContent`);
    expect(hit?.toLowerCase() === "telemetry", `highlight: ${hit}`);
    await shot("04-palette-search");
    await key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.view.selectedChatId === "chat-telemetry"`);
    expect(!(await paletteOpen()), "palette still open");
  });

  await step("Archived threads stay searchable with a badge", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("archived experiment");
    await page.waitFor(`!!document.querySelector(".x-badge")`);
    await shot("05-palette-archived");
    await esc();
    await page.waitFor(`!document.querySelector(".x-palette")`);
  });

  await step("Nonsense shows No results", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("zzqxqzz");
    await page.waitFor(`!!document.querySelector(".x-palette .x-empty")`, 5000);
    expect((await text(".x-empty-title")) === "No results", "empty title");
    await shot("06-palette-empty");
    await esc();
  });

  await step("Escape closes and focus returns to where it was", async () => {
    await page.waitFor(`!document.querySelector(".x-palette")`);
    await page.eval(`document.querySelector("textarea, [contenteditable=true]")?.focus(); true`);
    const before = await focused();
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await esc();
    await page.waitFor(`!document.querySelector(".x-palette")`);
    await sleep(50);
    const after = await focused();
    expect(before === after, `focus ${before} -> ${after}`);
  });

  await step("Scrim click closes the palette", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await page.call("Input.dispatchMouseEvent", { type, x: 20, y: 880, button: "left", clickCount: 1 });
    await page.waitFor(`!document.querySelector(".x-palette")`);
  });

  await step("Ctrl+P opens on Files with recent files of the selected chat; again closes", async () => {
    await ctrl("p", "KeyP", 80);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    expect((await text('.x-palette-tab[aria-selected="true"]')).startsWith("Files"), "not on Files");
    await page.waitFor(`document.querySelectorAll(".x-row-name").length > 0 || !!document.querySelector(".x-palette .x-empty")`, 10000);
    await sleep(300);
    console.log("\n    recent files:", await all(".x-row-name"), await all(".x-section-head"));
    await shot("07-palette-files-recent");
    await ctrl("p", "KeyP", 80);
    await page.waitFor(`!document.querySelector(".x-palette")`);
  });

  await step("File and content search in the project; Enter on a file opens it in the files panel", async () => {
    await ctrl("p", "KeyP", 80);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("ignition");
    await page.waitFor(`document.querySelectorAll(".x-row-code").length > 0`, 15000);
    const code = await all(".x-row-path");
    console.log("\n    content hits:", code);
    await shot("08-palette-content");
    await page.eval(`(() => { const i = document.querySelector(".x-palette-input"); i.select(); return true })()`);
    await page.type("launch");
    await page.waitFor(`[...document.querySelectorAll(".x-row-name")].some((e) => e.textContent === "launch.ts")`, 15000);
    await shot("09-palette-files");
    // Highlight the launch.ts row and open it.
    await page.eval(`(() => { const row = [...document.querySelectorAll(".x-row")].find((r) => r.querySelector(".x-row-name")?.textContent === "launch.ts"); row.dispatchEvent(new MouseEvent("mousemove", { bubbles: true })); return true })()`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.view.rightPanel === "files"`, 5000);
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("launchSequence")`, 10000);
    await shot("10-file-opened");
  });

  await step("Theme command flips the theme and keeps the palette open", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("theme");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => /Switch to (light|dark) theme/.test(e.textContent))`);
    const label = await page.eval(`[...document.querySelectorAll(".x-row-label")].find((e) => /Switch to (light|dark) theme/.test(e.textContent)).textContent`);
    await page.eval(`[...document.querySelectorAll(".x-row")].find((r) => /Switch to (light|dark) theme/.test(r.textContent)).click(); true`);
    await sleep(150);
    const next = await page.eval(`[...document.querySelectorAll(".x-row-label")].find((e) => /Switch to (light|dark) theme/.test(e.textContent))?.textContent`);
    expect(next && next !== label, `label stayed ${label}`);
    expect(await paletteOpen(), "palette closed");
    await page.eval(`(() => { const i = document.querySelector(".x-palette-input"); i.select(); return true })()`);
    await page.type(" ");
    await page.eval(`(() => { const i = document.querySelector(".x-palette-input"); i.select(); document.execCommand("delete"); return true })()`);
    await sleep(200);
    await shot("11-palette-all-light");
    await esc();
  });

  await step("Archive thread command archives the selected chat via the engine", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("archive thread");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Archive thread")`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.state.chats["chat-telemetry"]?.archived === true`, 10000);
    await page.waitFor(`[...document.querySelectorAll(".toast")].some((t) => t.textContent.includes("Thread archived"))`, 5000);
    await shot("12-archived-toast");
    await engine.rpc("Mutate", { op: "setChatArchived", chatId: "chat-telemetry", archived: false });
  });

  await step("Ctrl+/ opens shortcuts; Tab is trapped; Escape closes and restores focus", async () => {
    await page.eval(`document.querySelector("textarea, [contenteditable=true]")?.focus(); true`);
    const before = await focused();
    await ctrl("/", "Slash", 191);
    await page.waitFor(`!!document.querySelector(".x-shortcuts")`);
    const rows = await all(".x-shortcut dt");
    console.log("\n    shortcut rows:", rows);
    expect(rows.includes("Open chat switcher") && rows.includes("Show keyboard shortcuts") && rows.includes("Send message"), "missing rows");
    const heads = await all(".x-shortcuts-heading");
    expect(heads.join() === "Composer,Type in the composer,Chat,Application", `sections ${heads}`);
    for (let i = 0; i < 6; i++) await key("Tab", "Tab", 9);
    expect(await page.eval(`!!document.activeElement.closest(".x-shortcuts")`), "focus escaped");
    await sleep(300);
    await shot("13-shortcuts-light");
    await esc();
    await page.waitFor(`!document.querySelector(".x-shortcuts")`);
    await sleep(50);
    expect((await focused()) === before, "focus not restored");
  });

  await step("Shortcuts panel's Open settings goes to settings", async () => {
    await ctrl("/", "Slash", 191);
    await page.waitFor(`!!document.querySelector(".x-shortcuts")`);
    await page.eval(`[...document.querySelectorAll(".x-shortcuts button")].find((b) => b.textContent.includes("Change shortcuts in Settings")).click(); true`);
    await page.waitFor(`window.__zuse.view.route.kind === "settings" && window.__zuse.view.route.section === "shortcuts"`);
    await shot("14-settings-from-shortcuts");
  });

  await step("Ctrl+B toggles the sidebar, Ctrl+O opens New project, Ctrl+, opens settings", async () => {
    const open = await page.eval(`window.__zuse.view.preferences.sidebarOpen`);
    await ctrl("b", "KeyB", 66);
    await page.waitFor(`window.__zuse.view.preferences.sidebarOpen === ${!open}`);
    await ctrl("b", "KeyB", 66);
    await page.waitFor(`window.__zuse.view.preferences.sidebarOpen === ${open}`);
    await page.eval(`document.activeElement?.blur(); true`);
    await ctrl("o", "KeyO", 79);
    await page.waitFor(`window.__zuse.view.route.kind === "newProject"`);
    await ctrl(",", "Comma", 188);
    await page.waitFor(`window.__zuse.view.route.kind === "settings"`);
  });

  await setTheme("dark");
  // Banners post only in the background by default; headless Chromium always reports focus, so test them in the foreground.
  await page.eval(`(() => { const p = JSON.parse(localStorage.getItem("zuse.web.preferences.v1") || "{}"); p.browserNotifications = true; p.notifyInBackgroundOnly = false; localStorage.setItem("zuse.web.preferences.v1", JSON.stringify(p)); return true })()`);
  await page.call("Page.reload", {});
  await sleep(500);
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`, 30000);

  await step("Import chat lists provider history, searches, and imports into a thread", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("import");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Import chat")`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`!!document.querySelector(".x-import")`);
    await page.waitFor(`document.querySelectorAll(".x-import-row").length > 0 || !!document.querySelector(".x-import-caption")`, 15000);
    expect((await page.eval(`document.querySelector(".x-import").dataset.placement`)) === "centred", "palette launch should centre the menu");
    console.log("\n    import rows:", await all(".x-import-row"));
    await shot("15-import-list");
    expect(await page.eval(`document.activeElement?.getAttribute("aria-label") === "Search chats"`), "search not focused");
    await page.type("nothing-like-this");
    await page.waitFor(`document.querySelector(".x-import-caption")?.textContent.includes("No chats match")`);
    await shot("16-import-no-match");
    await page.eval(`(() => { const i = document.activeElement; i.select(); document.execCommand("delete"); return true })()`);
    await page.type("telemetry");
    await page.waitFor(`document.querySelectorAll(".x-import-row").length === 1`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`!document.querySelector(".x-import") && (window.__zuse.view.selectedChatId ?? "").startsWith("imported_")`, 20000);
    await sleep(800);
    await shot("17-imported-chat");
  });

  await step("Landing chip opens the import menu anchored above it; Escape returns focus to the chip", async () => {
    await page.click(".sb-new-chat, [aria-label='New chat']").catch(() => page.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "New chat").click(); true`));
    await page.waitFor(`[...document.querySelectorAll("button")].some((b) => b.textContent.trim() === "Import chat" && !b.disabled)`, 10000);
    await page.eval(`(() => { const b = [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Import chat"); b.id ||= "fx-import-chip"; return true })()`);
    const chipId = await page.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Import chat").id`);
    await page.press(`#${chipId}`);
    await page.waitFor(`document.querySelector(".x-import")?.dataset.placement === "anchored"`, 3000);
    await page.waitFor(`document.querySelectorAll(".x-import-row").length > 0`, 15000);
    const geometry = await page.eval(`(() => { const m = document.querySelector(".x-import").getBoundingClientRect(); const c = document.getElementById(${JSON.stringify(chipId)}).getBoundingClientRect(); return { menuBottom: m.bottom, chipTop: c.top, menuLeft: m.left, chipLeft: c.left, width: m.width } })()`);
    console.log("\n    anchored:", JSON.stringify(geometry));
    expect(Math.abs(geometry.menuBottom - (geometry.chipTop - 4)) < 2 && Math.abs(geometry.width - 288) < 1, "menu not above the chip");
    await sleep(300);
    await shot("16b-import-anchored");
    await esc();
    await page.waitFor(`!document.querySelector(".x-import")`);
    await sleep(50);
    expect(await page.eval(`document.activeElement?.id === ${JSON.stringify(chipId)}`), "focus did not return to the chip");
  });

  await step("Sending a message chimes once when the run finishes (no chime on load)", async () => {
    const before = await page.eval(`({ tones: window.__spy.tones, files: window.__spy.files })`);
    await page.eval(`window.__spy.banners.length = 0; true`);
    await engine.rpc("QueueCommand", {
      chatId: "chat-launch",
      command: { kind: "run", messageId: "m-1", request: { prompt: "hello", harness: "claude-code", model: null, reasoning: null, cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null } },
    });
    await page.waitFor(`!!window.__zuse.state.sessions["chat-launch"]?.lastCompletedTurn`, 20000);
    await sleep(600);
    const after = await page.eval(`({ tones: window.__spy.tones, files: window.__spy.files, banners: window.__spy.banners })`);
    console.log("\n    before", before, "after", JSON.stringify(after));
    console.log("    log:", (await page.eval(`window.__spy.log`)).join("\n         "));
    expect(after.tones - before.tones === 2, `expected one two-note chime, got ${after.tones - before.tones} oscillators`);
  });

  await step("Banners post when backgrounded; click-through opens the chat", async () => {
    await page.eval(`window.__spy.banners.length = 0; true`);
    await engine.rpc("QueueCommand", {
      chatId: "chat-docs",
      command: { kind: "run", messageId: "m-2", request: { prompt: "hello", harness: "claude-code", model: null, reasoning: null, cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null } },
    });
    await page.waitFor(`!!window.__zuse.state.sessions["chat-docs"]?.lastCompletedTurn`, 20000);
    await sleep(600);
    const banners = await page.eval(`window.__spy.banners`);
    console.log("\n    banners:", JSON.stringify(banners));
    expect(banners.length === 1 && banners[0].title === "Write onboarding docs", "expected one banner");
    await page.eval(`window.__spy.lastBanner.onclick(); true`);
    await page.waitFor(`window.__zuse.view.selectedChatId === "chat-docs"`);
  });

  await step("Visual: published visual renders and its controls round-trip through the host", async () => {
    const script = `
      local s = ui.state { n = 0, on = false, tab = "a", level = 3 }
      return function()
        return ui.column {
          ui.row { ui.stat { value = s.n, label = "count" }, ui.button { label = "Increment", primary = true, on_click = function() s.n = s.n + 1 end } },
          ui.toggle { label = "Log scale", value = s.on, on_change = function(v) s.on = v end },
          ui.tabs { options = { "a", "b" }, value = s.tab, on_change = function(v) s.tab = v end },
          ui.slider { label = "Level", value = s.level, min = 1, max = 10, on_change = function(v) s.level = v end },
          ui.chart { type = "bar", height = 160, x = { "Mon", "Tue", "Wed" }, series = { { name = "runs", values = { 3, 5, 2 } } } },
          ui.table { columns = { "Region", "p95" }, rows = { { "us-east", 182 }, { "eu-west", 140 } } },
          ui.canvas { height = 120, draw = function(g, w, h, t) g:rect(10, 10, w - 20, h - 20, { fill = "accent" }) end },
        }
      end`;
    // A turn must be running to publish; this agent takes its time.
    writeFileSync(join(engine.root, "bin", "claude"), `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
IFS= read -r prompt
printf '%s\\n' '{"type":"system","subtype":"init","session_id":"slow","model":"fixture","tools":[]}'
sleep 6
printf '%s\\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Here are the metrics."}]}}'
printf '%s\\n' '{"type":"result","subtype":"success","session_id":"slow","is_error":false,"result":"Here are the metrics."}'
`);
    await engine.rpc("QueueCommand", {
      chatId: "chat-launch",
      command: { kind: "run", messageId: "m-3", request: { prompt: "show metrics", harness: "claude-code", model: null, reasoning: null, cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null } },
    });
    await page.waitFor(`window.__zuse.state.sessions["chat-launch"]?.status === "working"`, 15000);
    let published = false;
    for (let i = 0; i < 20 && !published; i++) {
      try { await engine.rpc("PublishVisual", { chatId: "chat-launch", title: "Launch metrics", script }); published = true; } catch (error) { if (i === 19) throw error; await sleep(250); }
    }
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("launch checklist");
    await page.waitFor(`[...document.querySelectorAll(".x-row-title")].some((e) => e.textContent.includes("Launch checklist"))`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.view.selectedChatId === "chat-launch"`);
    await page.waitFor(`!!document.querySelector(".x-visual")`, 15000);
    // Desktop loads a visual as soon as it is shown; no button press.
    await page.waitFor(`!!document.querySelector(".x-visual-body")`, 15000);
    await page.eval(`document.querySelector(".x-visual").scrollIntoView({ block: "start" }); true`);
    await sleep(800);
    await shot("18-visual-dark");
    const count = () => page.eval(`document.querySelector(".x-vstat-value")?.textContent`);
    expect((await count()) === "0", `initial count ${await count()}`);
    await page.eval(`[...document.querySelectorAll(".x-visual button")].find((b) => b.textContent.includes("Increment")).click(); true`);
    await page.waitFor(`document.querySelector(".x-vstat-value")?.textContent === "1"`, 5000);
    // Keyboard: focus the toggle and press Space.
    await page.eval(`document.querySelector(".x-vtoggle input, .x-vtoggle [role=switch], .x-vtoggle button")?.focus(); true`);
    const toggleBefore = await page.eval(`(() => { const t = document.querySelector(".x-vtoggle input, .x-vtoggle [role=switch], .x-vtoggle button"); return t?.checked ?? t?.getAttribute("aria-checked") })()`);
    await key(" ", "Space", 32);
    await page.waitFor(`(() => { const t = document.querySelector(".x-vtoggle input, .x-vtoggle [role=switch], .x-vtoggle button"); return String(t?.checked ?? t?.getAttribute("aria-checked")) !== ${JSON.stringify(String(toggleBefore))} })()`, 5000);
    // Tabs
    await page.eval(`[...document.querySelectorAll(".x-vtabs button, .x-vtabs [role=tab]")].find((b) => b.textContent.trim() === "b")?.click(); true`);
    await page.waitFor(`[...document.querySelectorAll(".x-vtabs [aria-selected=true], .x-vtabs [aria-pressed=true]")].some((b) => b.textContent.trim() === "b")`, 5000);
    // Slider via keyboard
    await page.eval(`document.querySelector(".x-vslider input").focus(); true`);
    await key("ArrowRight", "ArrowRight", 39);
    await page.waitFor(`Number(document.querySelector(".x-vslider input").value) > 3`, 5000);
    await sleep(500);
    const sliderLabel = await page.eval(`document.querySelector(".x-vslider")?.textContent`);
    console.log("\n    slider:", sliderLabel);
    expect(await page.eval(`document.querySelectorAll(".x-vchart rect, .x-vchart path").length > 0`), "chart has no marks");
    expect(await page.eval(`document.querySelectorAll(".x-vtable td").length === 4`), "table cells");
    const pixels = await page.eval(`(() => { const c = document.querySelector(".x-vcanvas canvas"); if (!c) return -1; const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 3; i < d.length; i += 4) if (d[i]) n++; return n })()`);
    expect(pixels > 100, `canvas pixels ${pixels}`);
    await shot("19-visual-after-controls");
  });

  await step("Voice stage while connecting: focus, hide to pill, show, mute, end call", async () => {
    await page.eval(`document.querySelector("textarea, [contenteditable=true]")?.focus(); true`);
    const before = await focused();
    // Freeze the host so the call stays in its connecting step long enough to use the controls.
    process.kill(enginePid, "SIGSTOP");
    try {
      await ctrl("k", "KeyK", 75);
      await page.waitFor(`!!document.querySelector(".x-palette")`);
      await page.type("start voice");
      await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Start voice call")`);
      await key("Enter", "Enter", 13);
      await page.waitFor(`!!document.querySelector(".x-voice-stage")`, 3000);
      await sleep(600);
      expect(await page.eval(`document.activeElement?.classList.contains("x-voice-stage")`), "stage not focused");
      expect((await text(".x-voice-status")) === "Connecting voice…", `status ${await text(".x-voice-status")}`);
      const left = await page.eval(`document.querySelector(".x-voice-stage").getBoundingClientRect().left`);
      expect(left > 100, `stage covers the sidebar (left ${left})`);
      await shot("20a-voice-stage");
      await esc();
      await page.waitFor(`!document.querySelector(".x-voice-stage") && !!document.querySelector(".x-voice-pill")`);
      await shot("20b-voice-pill");
      await page.click('.x-voice-pill button[aria-label="Show voice call"]');
      await page.waitFor(`!!document.querySelector(".x-voice-stage")`);
      await page.click('.x-voice-stage button[aria-label="Mute"]');
      await page.waitFor(`document.querySelector('.x-voice-stage button[aria-label="Unmute"]')?.getAttribute("aria-pressed") === "true"`);
      await page.click('.x-voice-stage button[aria-label="End call"]');
      await page.waitFor(`!document.querySelector(".x-voice-stage") && !document.querySelector(".x-voice-pill")`);
      await sleep(100);
      expect((await focused()) === before, `focus ${before} -> ${await focused()}`);
    } finally {
      process.kill(enginePid, "SIGCONT");
    }
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 60000);
  });

  await step("Voice: fake microphone passes, host refusal ends the call with a notice, Dismiss clears it", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("voice");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Start voice call")`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`!!document.querySelector(".x-voice-stage")`, 3000);
    await shot("20-voice-connecting");
    const steps = [];
    for (let i = 0; i < 60; i++) {
      const s = await page.eval(`document.querySelector(".x-voice-status")?.textContent ?? document.querySelector(".x-voice-notice")?.textContent ?? null`);
      if (steps.at(-1) !== s) steps.push(s);
      if (await page.eval(`!!document.querySelector(".x-voice-notice")`)) break;
      await sleep(250);
    }
    console.log("\n    voice:", steps);
    await page.waitFor(`!!document.querySelector(".x-voice-notice")`, 20000);
    await shot("21-voice-notice");
    await page.eval(`[...document.querySelectorAll(".x-voice-pill button, .x-voice-stage button")].find((b) => b.textContent.includes("Dismiss")).click(); true`);
    await page.waitFor(`!document.querySelector(".x-voice-pill") && !document.querySelector(".x-voice-stage")`);
  });

  await step("Uncaught error shows one danger toast announced as an alert; close removes it", async () => {
    await page.eval(`setTimeout(() => { throw new Error("fixture failure") }); true`);
    await page.waitFor(`[...document.querySelectorAll('.toast[role="alert"]')].some((t) => t.textContent.includes("fixture failure"))`, 3000);
    await page.eval(`setTimeout(() => { throw new Error("fixture failure") }); true`);
    await sleep(200);
    const n = await page.eval(`[...document.querySelectorAll(".toast")].filter((t) => t.textContent.includes("fixture failure")).length`);
    expect(n === 1, `${n} toasts`);
    await shot("22-error-toast");
    await page.eval(`[...document.querySelectorAll(".toast")].find((t) => t.textContent.includes("fixture failure")).querySelector(".toast-close").click(); true`);
    await page.waitFor(`![...document.querySelectorAll(".toast")].some((t) => t.textContent.includes("fixture failure"))`);
  });

  await step("Palette commands and mouse: New chat, Open settings, clicking a thread row; scrim closes shortcuts", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("open settings");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Open settings")`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.view.route.kind === "settings"`);
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("new chat");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "New chat")`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && !window.__zuse.view.selectedChatId`);
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.eval(`(() => { const row = [...document.querySelectorAll(".x-row")].find((r) => r.textContent.includes("Launch checklist")); row.id ||= "fx-row"; return true })()`);
    await page.press(`#${await page.eval(`[...document.querySelectorAll(".x-row")].find((r) => r.textContent.includes("Launch checklist")).id`)}`);
    await page.waitFor(`window.__zuse.view.selectedChatId === "chat-launch" && !document.querySelector(".x-palette")`);
    await ctrl("/", "Slash", 191);
    await page.waitFor(`!!document.querySelector(".x-shortcuts")`);
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await page.call("Input.dispatchMouseEvent", { type, x: 1400, y: 880, button: "left", clickCount: 1 });
    await page.waitFor(`!document.querySelector(".x-shortcuts")`);
  });

  await step("Light theme: visual and palette files", async () => {
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`);
    await page.type("light theme");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Switch to light theme")`);
    await key("Enter", "Enter", 13);
    await page.waitFor(`document.documentElement.dataset.theme === "light"`);
    await esc();
    await page.waitFor(`!!document.querySelector(".x-visual-body")`, 15000);
    await page.eval(`document.querySelector(".x-visual").scrollIntoView({ block: "start" }); true`);
    // Hover a bar for the readout.
    const bar = await page.eval(`(() => { const r = document.querySelector(".x-vchart svg rect[rx='2']").getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
    await page.call("Input.dispatchMouseEvent", { type: "mouseMoved", x: bar.x, y: bar.y });
    await page.waitFor(`!!document.querySelector(".x-vchart-readout")`, 3000);
    expect((await text(".x-vchart-readout")) === "Mon", `readout ${await text(".x-vchart-readout")}`);
    await sleep(300);
    await shot("27-visual-light-hover");
    await ctrl("p", "KeyP", 80);
    await page.waitFor(`document.querySelectorAll(".x-row-name").length > 0`, 10000);
    await sleep(300);
    await shot("28-palette-files-light");
    await esc();
  });

  await step("Engine restart: palette works offline, import fails with Retry, warning after 10 s, silent reconnect, visual reloads", async () => {
    await page.eval(`window.__spy.banners.length = 0; window.__spy.filesBefore = window.__spy.files; true`);
    const port = new URL(engine.url).port;
    process.kill(enginePid, "SIGTERM");
    await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 15000);
    console.log("\n    disconnected kind:", await page.eval(`window.__zuse.state.connection.kind`));
    await sleep(400);
    console.log("    visual offline:", JSON.stringify(await page.eval(`(() => { const v = document.querySelector(".x-visual"); if (!v) return null; const r = v.getBoundingClientRect(); const chain = []; for (let e = v.parentElement; e && chain.length < 5; e = e.parentElement) chain.push(e.className + ":" + getComputedStyle(e).display + "/" + getComputedStyle(e).alignItems); return { html: v.outerHTML.slice(0, 400), x: r.x, w: r.width, chain } })()`)));
    await shot("23-disconnected");
    await ctrl("k", "KeyK", 75);
    await page.waitFor(`!!document.querySelector(".x-palette")`, 3000);
    await page.type("import");
    await page.waitFor(`[...document.querySelectorAll(".x-row-label")].some((e) => e.textContent === "Import chat")`);
    await sleep(200);
    await shot("24-palette-disconnected");
    await key("Enter", "Enter", 13);
    await page.waitFor(`!!document.querySelector(".x-import")`);
    await page.waitFor(`!!document.querySelector(".x-import-error")`, 15000);
    console.log("    import offline:", await text(".x-import-error"));
    await sleep(200);
    await shot("25-import-disconnected");
    await sleep(Math.max(0, 11000));
    const warn = await page.eval(`({ files: window.__spy.files - window.__spy.filesBefore, banners: window.__spy.banners })`);
    console.log("    warning:", JSON.stringify(warn));
    expect(warn.files === 1 && warn.banners.length === 1 && warn.banners[0].title === "Connection unavailable", "expected one connection-loss warning");
    // Same port, same profile: the page reconnects on its own.
    const binary = [join(repo, "target/release/zuse-server"), join(repo, "target/debug/zuse-server")].find(existsSync);
    respawned = spawn(binary, ["--listen", `127.0.0.1:${port}`, "--ipc-port", "0", "--data-dir", join(engine.root, "profile"), "--language", "en", "--assets", assets], {
      env: { ...process.env, HOME: home, CLAUDE_CONFIG_DIR: claudeDir, CODEX_HOME: join(home, ".codex"), PATH: `${join(engine.root, "bin")}:${process.env.PATH.split(":").filter((d) => !d.startsWith(userHome)).join(":")}`, CLAUDE_CODE_EXECUTABLE: join(engine.root, "bin", "claude"), RUST_LOG: "error" },
      stdio: "ignore",
    });
    const tonesBefore = await page.eval(`window.__spy.tones`);
    await page.waitFor(`window.__zuse.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`, 60000);
    await page.eval(`[...document.querySelectorAll(".x-import-error button")].find((b) => b.textContent.includes("Retry"))?.click(); true`);
    await page.waitFor(`document.querySelectorAll(".x-import-row").length > 0`, 15000);
    await esc();
    await page.waitFor(`!document.querySelector(".x-import")`);
    await page.waitFor(`!!document.querySelector(".x-visual-body")`, 15000);
    await sleep(1500);
    expect((await page.eval(`window.__spy.tones`)) === tonesBefore, "reconnect chimed");
    await shot("26-reconnected");
  });
} finally {
  try { process.kill(enginePid, "SIGCONT"); } catch {}
  if (respawned && respawned.exitCode === null) { respawned.kill("SIGTERM"); await sleep(800); if (respawned.exitCode === null) respawned.kill("SIGKILL"); }
  if (page.errors.length) console.log("page errors:\n" + page.errors.join("\n"));
  await page.close();
  await engine.close();
  rmSync(scratch, { recursive: true, force: true });
  const failed = results.filter((r) => !r[1]);
  console.log(`\n${results.length - failed.length}/${results.length} passed`);
  if (failed.length) process.exitCode = 1;
}
