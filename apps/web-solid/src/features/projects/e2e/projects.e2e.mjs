// Projects area e2e: a disposable engine with a temp HOME, fixture git remotes and a fixture GitHub CLI,
// served through a vite dev server that mounts only this area (frame.tsx), driven by headless Chromium.
// Run from apps/web-solid: node src/features/projects/e2e/projects.e2e.mjs   (SHOTS=dir keeps screenshots)
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const app = join(here, "../../../..");
const { startEngine, openBrowser } = await import(join(app, "e2e/harness.mjs"));
const { createServer } = await import("vite");
const { default: solid } = await import("vite-plugin-solid");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const shots = process.env.SHOTS ?? mkdtempSync(join(tmpdir(), "pj-shots-"));
mkdirSync(shots, { recursive: true });

// A fake home so the engine's folder browser and home index stay inside /tmp.
const home = mkdtempSync(join(tmpdir(), "pj-home-"));
const git = (cwd, ...args) => execFileSync("git", args, { cwd, stdio: "ignore", env: { ...process.env, HOME: home, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t" } });
for (const dir of ["code/alpha", "code/beta-notes", "code/gamma", "Documents/letters", "Developer"]) mkdirSync(join(home, dir), { recursive: true });
git(join(home, "code/alpha"), "init", "-q");
writeFileSync(join(home, "code/alpha/README.md"), "alpha\n");
git(join(home, "code/alpha"), "add", ".");
git(join(home, "code/alpha"), "commit", "-qm", "init");
const remote = join(home, "remote-src");
mkdirSync(remote);
git(remote, "init", "-q");
writeFileSync(join(remote, "zuse.json"), JSON.stringify({ actions: [{ name: "Hello", command: "echo hello", icon: "play" }] }));
writeFileSync(join(remote, "README.md"), "remote\n");
git(remote, "add", ".");
git(remote, "commit", "-qm", "init");
execFileSync("git", ["clone", "-q", "--bare", remote, join(home, "sample.git")]);

// Git rewrites these https URLs (which the engine accepts) to local fixtures, so nothing touches the network.
writeFileSync(join(home, ".gitconfig"), `[url "file://${home}/"]\n\tinsteadOf = https://fixture.invalid/\n[url "ext::sh -c sleep% 30% #"]\n\tinsteadOf = https://slow.invalid/\n[protocol "ext"]\n\tallow = always\n`);
// A fixture GitHub CLI: signed in, with 50 repositories on page 1 and 3 on page 2. No real account is read.
const ghDir = join(home, ".fixture-bin");
mkdirSync(ghDir);
const repo = (name, priv) => ({ full_name: `fixture-org/${name}`, description: null, ssh_url: `git@fixture.invalid:fixture-org/${name}.git`, clone_url: name === "sample" ? "https://fixture.invalid/sample.git" : `https://fixture.invalid/${name}.git`, private: priv });
const page1 = [repo("sample", false), ...Array.from({ length: 49 }, (_, i) => repo(`repo-${String(i).padStart(2, "0")}`, i % 3 === 0))];
const page2 = [repo("late-one", false), repo("late-two", true), repo("late-three", false)];
writeFileSync(join(ghDir, "page1.json"), JSON.stringify(page1));
writeFileSync(join(ghDir, "page2.json"), JSON.stringify(page2));
// The mode file switches it to signed out or failing repository listings.
writeFileSync(join(ghDir, "mode"), "ok");
const ghMode = (mode) => writeFileSync(join(ghDir, "mode"), mode);
writeFileSync(join(ghDir, "gh"), `#!/bin/sh
mode=$(cat "${ghDir}/mode")
case "$*" in
  "auth status"*) [ "$mode" = signedout ] && exit 1; exit 0;;
esac
[ "$mode" = reposfail ] && { echo "HTTP 502" >&2; exit 1; }
case "$*" in
  *page=1) cat "${ghDir}/page1.json";;
  *page=2) cat "${ghDir}/page2.json";;
  *) echo '[]';;
esac
`, { mode: 0o755 });
const realPath = process.env.PATH;
process.env.PATH = `${ghDir}:${realPath}`;
const realHome = process.env.HOME;
process.env.HOME = home;
process.env.ZUSE_CHROME = `${realHome}/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome`;
const engine = await startEngine({});
process.env.HOME = realHome;
process.env.PATH = realPath;
const target = engine.origin;
const repoRoot = join(app, "../..");
const vite = await createServer({
  root: here,
  configFile: false,
  logLevel: "error",
  plugins: [solid()],
  resolve: {
    alias: [
      { find: "@repo", replacement: repoRoot },
      { find: /^@\//, replacement: `${app}/src/` },
    ],
  },

  server: { port: 0, host: "127.0.0.1", strictPort: false, hmr: false, fs: { allow: [repoRoot] }, proxy: { "/api": { target, ws: true } } },
});
await vite.listen();
const viteUrl = `http://127.0.0.1:${vite.httpServer.address().port}/`;
const token = new URL(engine.url).hash;
const page = await openBrowser(viteUrl + token, { width: 1280, height: 820 });
// Record sockets so steps can drop the connection; reload once the session cookie is set.
await page.call("Page.addScriptToEvaluateOnNewDocument", { source: `(() => { const W = window.WebSocket; window.__sockets = []; window.WebSocket = class extends W { constructor(...a) { super(...a); window.__sockets.push(this); } }; })()` });
await page.call("Page.reload", {});
await sleep(500);
const dropNow = `window.__sockets.filter((s) => s.readyState === 1 && s.url.includes("/api/rpc")).forEach((s) => s.close(4000, "test drop"))`;

const results = [];
const step = async (name, fn) => {
  try {
    await fn();
    results.push(["ok", name]);
    console.log("ok  ", name);
  } catch (error) {
    results.push(["FAIL", name, String(error?.message ?? error)]);
    console.log("FAIL", name, String(error?.message ?? error));
    await page.screenshot(join(shots, `fail-${results.length}.png`)).catch(() => {});
  }
};
const q = (sel) => `document.querySelector(${JSON.stringify(sel)})`;
const text = (sel) => page.eval(`${q(sel)}?.textContent ?? ""`);
const typeInto = async (sel, value) => {
  await page.eval(`(() => { const e = ${q(sel)}; e.focus(); e.select?.(); return true })()`);
  await page.type(value);
};
const clearInput = async (sel) => page.eval(`(() => { const e = ${q(sel)}; e.focus(); e.value = ""; e.dispatchEvent(new InputEvent("input", { bubbles: true })); return true })()`);
const nav = async (route) => {
  await page.eval(`(window.__zuse.viewModule.navigate({ kind: "chat" }), true)`);
  await page.eval(`(window.__zuse.viewModule.navigate(${JSON.stringify(route)}), true)`);
};
const rowWith = (needle) => `[...document.querySelectorAll(".pj-row")].find((r) => r.textContent.includes(${JSON.stringify(needle)}))`;
const clickRow = (needle) => page.eval(`(() => { const r = ${rowWith(needle)}; if (!r) throw new Error("no row ${needle}"); r.click(); return true })()`);

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`, 30000);

  await step("New project opens with Start actions", async () => {
    await nav({ kind: "newProject" });
    await page.waitFor(`!!${q(".pj-card")}`);
    const rows = await page.eval(`[...document.querySelectorAll(".pj-row")].map((r) => r.textContent)`);
    if (!rows.some((r) => r.includes("Open folder")) || !rows.some((r) => r.includes("Clone repository")) || !rows.some((r) => r.includes("Create project")))
      throw new Error(`rows: ${rows}`);
    if (await page.eval(`document.activeElement !== ${q(".pj-input")}`)) throw new Error("input not focused");
    await page.screenshot(join(shots, "01-choose.png"));
  });

  await step("Filtering narrows actions and arrows move the highlight", async () => {
    await page.type("clone");
    await page.waitFor(`document.querySelectorAll(".pj-row").length === 1`);
    await clearInput(".pj-input");
    await page.waitFor(`document.querySelectorAll(".pj-row").length >= 3`);
    await page.key("ArrowDown", "ArrowDown", 40);
    const hl = await page.eval(`document.querySelector('.pj-row[data-highlighted="true"]')?.textContent`);
    if (!hl?.includes("Clone")) throw new Error(`highlight ${hl}`);
    await page.key("ArrowUp", "ArrowUp", 38);
  });

  await step("Open folder browses the host Home, including non-Git folders", async () => {
    await clickRow("Open folder");
    await page.waitFor(`!!${rowWith("code")}`);
    await clickRow("code");
    await page.waitFor(`!!${rowWith("beta-notes")} && !!${rowWith("alpha")}`);
    const repoMark = await page.eval(`!!${rowWith("alpha")}.querySelector(".pj-glyph + span + span, [class*=pj-glyph]:last-child")`);
    await page.screenshot(join(shots, "02-browser.png"));
    if (!repoMark) throw new Error("no repo marker");
  });

  await step("Typing filters folders, Tab completes and Escape-free back works", async () => {
    await page.type("ga");
    await page.waitFor(`document.querySelectorAll(".pj-row").length === 1 && !!${rowWith("gamma")}`);
    const ghost = await text(".pj-ghost");
    if (!ghost.endsWith("mma")) throw new Error(`ghost ${ghost}`);
    await page.key("Tab", "Tab", 9);
    await page.waitFor(`${q(".pj-input")}.value === "gamma"`);
    await clearInput(".pj-input");
  });

  await step("Home search finds folders anywhere under Home", async () => {
    // Back to Home root, then search for a nested folder.
    await page.eval(`[...document.querySelectorAll(".pj-crumb")].find((c) => c.textContent.includes("Home")).click()`);
    await page.waitFor(`!!${rowWith("Documents")}`);
    await page.type("letters");
    await page.waitFor(`!!${rowWith("letters")}`, 20000);
    await page.screenshot(join(shots, "03-home-search.png"));
    await clearInput(".pj-input");
  });

  await step("New folder is created on the host and opened", async () => {
    await page.click('button[aria-label="New folder"]');
    await page.waitFor(`!!${q(".pj-new-folder-input")}`);
    await page.type("fresh-folder");
    await page.eval(`${q(".pj-new-folder")}.requestSubmit()`);
    await page.waitFor(`[...document.querySelectorAll(".pj-crumb")].some((c) => c.textContent.includes("fresh-folder"))`);
    if (!existsSync(join(home, "fresh-folder"))) throw new Error("folder not on disk");
  });

  await step("Add project creates a space for a non-Git folder and lands in it", async () => {
    await page.click(".pj-submit-add");
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && window.__zuse.view.draftProjectPath?.endsWith("fresh-folder")`);
    await page.waitFor(`Object.values(window.__zuse.state.spaces).some((s) => s.path.endsWith("fresh-folder") && !s.gitDetected)`);
  });

  await step("Opening an existing project from the list lands in it", async () => {
    await nav({ kind: "newProject" });
    await page.waitFor(`!!${rowWith("fresh-folder")}`);
    await clickRow("fresh-folder");
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`);
  });

  await step("Create: Empty template with a default location", async () => {
    await nav({ kind: "newProject" });
    await page.waitFor(`!!${rowWith("Create project")}`);
    await clickRow("Create project");
    await page.waitFor(`${q(".pj-input")}.placeholder === "Project name"`);
    await page.waitFor(`[...document.querySelectorAll(".pj-row")].some((r) => r.textContent.includes("Choose a folder") === false && r.textContent.includes("Change"))`);
    await page.type("demo-app");
    await page.waitFor(`${q(".pj-summary-detail")}.textContent.startsWith("Creates ")`);
    // The fixture CLI is signed in, so publishing is offered; toggle it on and off again (no publish run).
    await page.waitFor(`${rowWith("GitHub repository")}.getAttribute("aria-disabled") === "false"`, 10000);
    await clickRow("GitHub repository");
    await page.waitFor(`${rowWith("GitHub repository")}.getAttribute("aria-checked") === "true"`);
    await page.screenshot(join(shots, "04-create.png"));
    await clickRow("GitHub repository");
    await page.waitFor(`${rowWith("GitHub repository")}.getAttribute("aria-checked") === "false"`);
    await page.click(".pj-submit");
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && window.__zuse.view.draftProjectPath?.endsWith("demo-app")`, 20000);
  });

  await step("Create finishes: files ready on disk and space registered", async () => {
    await page.waitFor(`Object.values(window.__zuse.state.spaces).some((s) => s.path.endsWith("demo-app"))`, 30000);
    const parent = await page.eval(`window.__zuse.view.draftProjectPath`);
    for (let i = 0; i < 100 && !existsSync(join(parent, "README.md")); i++) await sleep(200);
    if (!existsSync(join(parent, "README.md"))) throw new Error(`no README in ${parent}`);
  });

  await step("Clone: repository search, Load more, pick and clone", async () => {
    await nav({ kind: "newProject" });
    await page.waitFor(`!!${rowWith("Clone repository")}`);
    await clickRow("Clone repository");
    await page.waitFor(`${q(".pj-input")}.placeholder.startsWith("Search your repositories")`);
    await page.waitFor(`!!${rowWith("repo-00")}`, 15000);
    await page.eval(`(document.querySelector("#pj-setup-list").scrollTop = 1e6, true)`);
    await page.waitFor(`!!${rowWith("repo-48")} && !!${rowWith("Load more repositories")}`, 5000);
    await page.screenshot(join(shots, "05a-repos.png"));
    await clickRow("Load more repositories");
    await page.eval(`(document.querySelector("#pj-setup-list").scrollTop = 1e6, true)`);
    await page.waitFor(`!!${rowWith("late-three")} && !${rowWith("Load more repositories")}`, 15000);
    await page.type("sampl");
    await page.waitFor(`document.querySelectorAll(".pj-row").length === 1 && !!${rowWith("sample")}`);
    await page.key("Enter");
    await page.waitFor(`${q(".pj-input")}.placeholder === "Folder name" && ${q(".pj-input")}.value === "sample"`);
    await page.screenshot(join(shots, "05-clone-confirm.png"));
    await page.key("Enter", "Enter", 13, 2);
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && window.__zuse.view.draftProjectPath?.endsWith("/sample")`, 20000);
    const dest = await page.eval(`window.__zuse.view.draftProjectPath`);
    for (let i = 0; i < 100 && !existsSync(join(dest, "README.md")); i++) await sleep(200);
    if (!existsSync(join(dest, "README.md"))) throw new Error("clone missing files " + JSON.stringify((await engine.rpc("ListProjectCreations")).map((o) => [o.request, o.status, o.error])));
    await page.waitFor(`Object.values(window.__zuse.state.spaces).some((s) => s.path.endsWith("/sample"))`, 20000);
  });

  await step("GitHub signed out and failing listings explain themselves; Retry recovers", async () => {
    ghMode("signedout");
    await nav({ kind: "newProject" });
    await clickRow("Clone repository");
    await page.waitFor(`document.querySelector(".pj-note")?.textContent.startsWith("Run gh auth login")`, 10000);
    await nav({ kind: "newProject" });
    await clickRow("Create project");
    await page.waitFor(`${rowWith("GitHub repository")}?.textContent.includes("Run gh auth login to enable") && ${rowWith("GitHub repository")}.getAttribute("aria-disabled") === "true"`, 10000);
    ghMode("reposfail");
    await nav({ kind: "newProject" });
    await clickRow("Clone repository");
    await page.waitFor(`document.querySelector(".pj-note")?.textContent.startsWith("Could not load repositories")`, 10000);
    await page.screenshot(join(shots, "05b-repos-failed.png"));
    ghMode("ok");
    await page.eval(`[...document.querySelectorAll(".pj-footer button")].find((b) => b.textContent === "Retry").click()`);
    await page.waitFor(`!!${rowWith("repo-00")}`, 15000);
  });

  await step("A request cut off by a dropped connection says it may have completed", async () => {
    await nav({ kind: "newProject" });
    await clickRow("Create project");
    await page.waitFor(`${q(".pj-input")}.placeholder === "Project name"`);
    await page.type("dropped-app");
    await page.waitFor(`${q(".pj-summary-detail")}.textContent.startsWith("Creates ")`);
    await page.eval(`(document.querySelector(".pj-submit").click(), ${dropNow}, true)`);
    await page.waitFor(`document.querySelector(".pj-error")?.textContent.includes("may already be setting up")`, 5000);
    await page.screenshot(join(shots, "04b-dropped.png"));
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 15000);
    // Resubmitting reuses the operation id, so it cannot start a second project.
    await page.click(".pj-submit");
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && window.__zuse.view.draftProjectPath?.endsWith("dropped-app")`, 20000);
  });

  await step("Clone: a pasted URL continues to the destination; Backspace returns", async () => {
    await nav({ kind: "newProject" });
    await clickRow("Clone repository");
    await page.waitFor(`${q(".pj-input")}.placeholder.startsWith("Search your repositories")`);
    await page.type("https://fixture.invalid/sample.git");
    await page.waitFor(`!!${rowWith("Clone from URL")}`);
    await page.key("Enter");
    await page.waitFor(`${q(".pj-input")}.value === "sample"`);
    await clearInput(".pj-input");
    await page.key("Backspace", "Backspace", 8);
    await page.waitFor(`${q(".pj-input")}.placeholder.startsWith("Search your repositories") && ${q(".pj-input")}.value === "https://fixture.invalid/sample.git"`);
    await page.key("Enter");
    await page.waitFor(`${q(".pj-input")}.value === "sample"`);
    await clearInput(".pj-input");
    await page.type("sample-copy");
    await page.waitFor(`${q(".pj-summary-detail")}.textContent.includes("sample-copy")`);
    await page.key("Enter", "Enter", 13, 2);
    await page.waitFor(`window.__zuse.view.route.kind === "chat" && window.__zuse.view.draftProjectPath?.endsWith("sample-copy")`, 20000);
  });

  await step("Failed creation shows Retry in the project list", async () => {
    await nav({ kind: "newProject" });
    await clickRow("Clone repository");
    await page.waitFor(`${q(".pj-input")}.placeholder.startsWith("Search")`);
    await page.type("https://fixture.invalid/missing.git");
    await page.key("Enter");
    await page.waitFor(`${q(".pj-input")}.value === "missing"`);
    await page.key("Enter", "Enter", 13, 2);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`, 20000);
    await sleep(1500);
    await nav({ kind: "newProject" });
    await page.waitFor(`!!${rowWith("missing")}`, 10000);
    await page.waitFor(`[...document.querySelectorAll(".pj-row-action")].some((b) => b.textContent === "Retry")`, 20000);
    await page.screenshot(join(shots, "06-failed.png"));
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`);
  });

  await step("Retry restarts a failed creation", async () => {
    await nav({ kind: "newProject" });
    await page.waitFor(`[...document.querySelectorAll(".pj-row-action")].some((b) => b.textContent === "Retry")`, 10000);
    const before = await page.eval(`window.__zuse.state.localDeviceId`);
    await page.eval(`[...document.querySelectorAll(".pj-row-action")].find((b) => b.textContent === "Retry").click()`);
    // It fails again quickly; the badge returns to Failed with Retry offered.
    await page.waitFor(`[...document.querySelectorAll(".pj-badge")].some((b) => b.textContent.includes("Failed"))`, 20000);
    if (!before) throw new Error("no device");
    await page.key("Escape", "Escape", 27);
  });

  await step("A running creation shows progress and can be cancelled", async () => {
    await nav({ kind: "newProject" });
    await clickRow("Clone repository");
    await page.waitFor(`${q(".pj-input")}.placeholder.startsWith("Search")`);
    await page.type("https://slow.invalid/slowpoke.git");
    await page.key("Enter");
    await page.waitFor(`${q(".pj-input")}.value === "slowpoke" && ${q(".pj-summary-detail")}.textContent.startsWith("Clones into")`);
    await page.key("Enter", "Enter", 13, 2);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`, 20000).catch(async (e) => { await page.screenshot(join(shots, "slow-fail.png")); throw e; });
    await page.waitFor(`!!document.querySelector("#notice-host .pj-notice")`, 10000);
    await page.screenshot(join(shots, "08-notice-running.png"));
    await page.eval(`document.querySelector('#notice-host .pj-notice button').click()`);
    await page.waitFor(`document.querySelector("#notice-host .pj-notice")?.textContent.includes("cancelled")`, 20000);
    await page.screenshot(join(shots, "09-notice-cancelled.png"));
    await nav({ kind: "newProject" });
    await page.waitFor(`${rowWith("slowpoke")}?.textContent.includes("Cancelled")`, 5000);
    await page.key("Escape", "Escape", 27);
  });

  const sampleSpace = await page.eval(`Object.values(window.__zuse.state.spaces).find((s) => s.path.endsWith("/sample"))?.id`);
  const chatId = "22222222-2222-4222-8222-222222222222";
  if (sampleSpace) {
    await engine.rpc("Mutate", { op: "createChat", chatId, spaceId: sampleSpace });
    await page.waitFor(`!!window.__zuse.state.chats[${JSON.stringify(chatId)}]`);
  }

  await step("Project actions: zuse.json import, add, run, edit, delete", async () => {
    await page.eval(`(window.__zuse.viewModule.openChat(${JSON.stringify(chatId)}), true)`);
    await page.waitFor(`!!document.querySelector("#actions-host .pj-actions")`);
    await page.waitFor(`[...document.querySelectorAll("#actions-host .pj-action-row")].some((r) => r.textContent.includes("Hello") && r.textContent.includes("Import"))`, 10000);
    await page.screenshot(join(shots, "10-actions.png"));
    await page.eval(`[...document.querySelectorAll("#actions-host .pj-action-run")].find((b) => b.textContent.includes("Hello")).click()`);
    await page.waitFor(`document.querySelector("#pj-action-name")?.value === "Hello"`);
    if ((await page.eval(`document.querySelector("#pj-action-command").value`)) !== "echo hello") throw new Error("import draft command");
    await page.screenshot(join(shots, "11-editor.png"));
    await page.eval(`document.querySelector(".pj-editor").requestSubmit()`);
    await page.waitFor(`!document.querySelector(".pj-editor") && [...document.querySelectorAll("#actions-host .pj-action-run")].some((b) => b.getAttribute("aria-label") === "Run Hello")`, 10000);
    // Validation.
    await page.eval(`[...document.querySelectorAll("#actions-host .pj-action-link")].find((b) => b.textContent === "Add action").click()`);
    await page.waitFor(`!!document.querySelector("#pj-action-name")`);
    await page.eval(`document.querySelector(".pj-editor").requestSubmit()`);
    await page.waitFor(`document.querySelector(".pj-editor-error")?.textContent === "Action name is required"`);
    await typeInto("#pj-action-name", "Build it");
    await typeInto("#pj-action-command", "echo built");
    await page.eval(`document.querySelector('.pj-icon-choice[aria-label="Build"]').click()`);
    await page.eval(`document.querySelector(".pj-check").click()`);
    await page.eval(`document.querySelector(".pj-editor").requestSubmit()`);
    await page.waitFor(`[...document.querySelectorAll("#actions-host .pj-action-name")].some((n) => n.textContent === "Build it (setup)")`, 10000);
    // Run.
    await page.eval(`document.querySelector('[aria-label="Run Hello"]').click()`);
    await page.waitFor(`window.__zuse.actions.actionTerminals().length === 1 && window.__zuse.view.rightPanel === "terminal"`, 10000);
    const terminalId = await page.eval(`window.__zuse.actions.actionTerminals()[0].terminalId`);
    const first = await engine.rpc("SubscribeTerminal", { terminalId });
    if (!first) throw new Error("action terminal has no output stream");
    // Edit then delete.
    await page.eval(`document.querySelector('[aria-label="Edit action Build it"]').click()`);
    await page.waitFor(`document.querySelector("#pj-action-name")?.value === "Build it"`);
    await page.eval(`[...document.querySelectorAll(".pj-editor button")].find((b) => b.textContent === "Delete action").click()`);
    await page.waitFor(`[...document.querySelectorAll(".dialog-title")].some((t) => t.textContent === "Delete action?")`);
    await page.screenshot(join(shots, "12-delete.png"));
    await page.eval(`[...document.querySelectorAll(".dialog button")].find((b) => b.textContent === "Delete").click()`);
    await page.waitFor(`!document.querySelector(".dialog") && ![...document.querySelectorAll("#actions-host .pj-action-name")].some((n) => n.textContent.startsWith("Build it"))`, 10000);
    const saved = await engine.rpc("ListProjectActions", { spaceId: sampleSpace });
    if (saved.actions.length !== 1 || saved.actions[0].name !== "Hello") throw new Error(JSON.stringify(saved));
  });

  // Worktree location is a Settings → General control, covered by e2e/settings.mjs.

  await step("Archive: list, read-only preview, restore", async () => {
    await engine.rpc("Mutate", { op: "setChatArchived", chatId, archived: true });
    await page.waitFor(`window.__zuse.state.chats[${JSON.stringify(chatId)}]?.archived === true`);
    await nav({ kind: "archive", projectKey: sampleSpace });
    await page.waitFor(`!!document.querySelector(".pj-archive-row")`, 10000);
    await page.screenshot(join(shots, "14-archive-list.png"));
    await page.eval(`document.querySelector(".pj-archive-open").click()`);
    await page.waitFor(`!!document.querySelector(".pj-preview")`);
    await page.waitFor(`!document.querySelector(".pj-preview-body")?.textContent.includes("Loading transcript")`, 10000);
    await page.screenshot(join(shots, "15-archive-preview.png"));
    await page.eval(`document.querySelector(".pj-preview-foot .pj-restore").click()`);
    await page.waitFor(`window.__zuse.state.chats[${JSON.stringify(chatId)}]?.archived === false && window.__zuse.view.route.kind === "chat" && window.__zuse.view.selectedChatId === ${JSON.stringify(chatId)}`, 10000);
  });

  await step("Project archive renders for a project", async () => {
    const space = await page.eval(`Object.values(window.__zuse.state.spaces).find((s) => s.path.endsWith("sample"))?.id`);
    await nav({ kind: "archive", projectKey: space });
    await page.waitFor(`!!document.querySelector(".pj-archive h1")`);
    const empty = await text(".pj-archive-empty strong");
    if (empty !== "No archived chats yet") throw new Error(empty);
    await page.screenshot(join(shots, "07-archive.png"));
  });

  await step("Archive preview renders the full transcript and resumes after a dropped connection", async () => {
    // A fixture agent that streams markdown and runs one tool, so the archive has real content.
    const line = (event) => `printf '%s\\n' '${JSON.stringify(event).replace(/'/g, "'\\''")}'`;
    const body = "## Hello\n\nSome **markdown**:\n\n- one\n- two\n";
    writeFileSync(join(engine.root, "bin/claude"), [
      "#!/bin/sh",
      `case "$*" in *--version*) echo '2.1.0'; exit 0;; esac`,
      "IFS= read -r prompt",
      line({ type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] }),
      line({ type: "assistant", message: { id: "m1", role: "assistant", model: "fixture", content: [{ type: "tool_use", id: "tu1", name: "Bash", input: { command: "ls" } }], usage: { input_tokens: 1, output_tokens: 1 } } }),
      line({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "tu1", content: "README.md" }] } }),
      ...body.match(/.{1,8}/gs).map((piece) => line({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: piece } } })),
      line({ type: "assistant", message: { id: "m2", role: "assistant", model: "fixture", content: [{ type: "text", text: body }], usage: { input_tokens: 1, output_tokens: 1 } } }),
      line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: body, usage: { input_tokens: 1, output_tokens: 1 } }),
      "",
    ].join("\n"), { mode: 0o700 });
    const rich = "99999999-9999-4999-8999-999999999999";
    const space = Object.values(await page.eval(`JSON.parse(JSON.stringify(window.__zuse.state.spaces))`)).find((s) => s.path.endsWith("/sample"));
    const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write", plan: false };
    await engine.rpc("Mutate", { op: "createChat", chatId: rich, spaceId: space.id, config });
    await engine.rpc("QueueCommand", { chatId: rich, command: { kind: "run", messageId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", request: { prompt: "Greet me", ...config, cwd: space.path, autoApprove: false, resume: null } } });
    await page.waitFor(`window.__zuse.state.chats[${JSON.stringify(rich)}]?.lastMessageAt`, 15000);
    await sleep(1000);
    await engine.rpc("Mutate", { op: "setChatArchived", chatId: rich, archived: true });
    await nav({ kind: "archive", projectKey: space.id });
    await page.waitFor(`[...document.querySelectorAll(".pj-archive-row")].some((r) => r.textContent.includes("Greet me"))`, 10000);
    await page.eval(`[...document.querySelectorAll(".pj-archive-row")].find((r) => r.textContent.includes("Greet me")).querySelector(".pj-archive-open").click()`);
    await page.waitFor(`document.querySelector(".pj-transcript")?.textContent.includes("1 tool call") && document.querySelector(".pj-transcript").textContent.includes("Some markdown")`, 10000);
    await page.screenshot(join(shots, "16-archive-rich.png"));
    await page.eval(`(${dropNow}, true)`);
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 15000);
    await page.waitFor(`document.querySelector(".pj-transcript")?.textContent.includes("1 tool call")`, 10000);
    await nav({ kind: "chat" });
  });

  await step("Light theme renders the palette, browser and archive", async () => {
    await page.eval(`(window.__zuse.viewModule.setView("preferences", "theme", "light"), true)`);
    await page.waitFor(`document.documentElement.dataset.theme === "light"`);
    await nav({ kind: "newProject" });
    await page.waitFor(`!!${rowWith("sample")}`);
    await page.screenshot(join(shots, "20-light-choose.png"));
    await clickRow("Create project");
    await page.waitFor(`${q(".pj-input")}.placeholder === "Project name"`);
    await page.screenshot(join(shots, "21-light-create.png"));
    await nav({ kind: "newProject" });
    await clickRow("Open folder");
    await page.waitFor(`!!${rowWith("code")}`);
    await page.screenshot(join(shots, "22-light-browser.png"));
    // Keyboard: Right opens the highlighted folder, Left goes up.
    await page.eval(`(() => { const i = ${q(".pj-input")}; i.focus(); return true })()`);
    const first = await page.eval(`document.querySelector('.pj-row[data-highlighted="true"] .pj-title').textContent`);
    await page.key("ArrowRight", "ArrowRight", 39);
    await page.waitFor(`[...document.querySelectorAll(".pj-crumb")].some((c) => c.textContent === ${JSON.stringify(first)})`);
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await page.waitFor(`![...document.querySelectorAll(".pj-crumb")].some((c) => c.textContent === ${JSON.stringify(first)})`);
    await page.eval(`(window.__zuse.viewModule.setView("preferences", "theme", "dark"), true)`);
  });

  if (page.errors.length) console.log("page errors:\n" + page.errors.join("\n"));
} finally {
  await page.close();
  await vite.close();
  await engine.close();
  rmSync(home, { recursive: true, force: true });
  if (!process.env.SHOTS) rmSync(shots, { recursive: true, force: true });
}
const failed = results.filter((r) => r[0] === "FAIL");
console.log(`${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
