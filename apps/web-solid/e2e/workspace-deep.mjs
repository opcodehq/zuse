// Workspace area in depth: branch refusals, tree keyboard and live renames, read-only and failed saves, panel
// resizing, every diff scope, Git publish/pull/push against a local bare remote, history paging, terminal tabs,
// an embedded local page and reconnects. Disposable engine, throwaway repositories under the system temp directory.
// Usage: node e2e/workspace-deep.mjs  (ZUSE_ASSETS=<build> skips the build, ZUSE_SHOTS=<dir> keeps screenshots)
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { execFileSync, spawn } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync, renameSync, unlinkSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
if (shots) mkdirSync(shots, { recursive: true });
const only = process.env.ONLY ? new Set(process.env.ONLY.split(",")) : null;
const env = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
const git = (cwd, ...args) => execFileSync("git", args, { cwd, encoding: "utf8", env });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const root = mkdtempSync(join(tmpdir(), "zuse-wsq-"));
const remote = join(root, "remote.git");
const project = join(root, "project");
const other = join(root, "other");
git(root, "init", "-q", "--bare", "-b", "main", remote);
git(root, "init", "-q", "-b", "main", project);
git(project, "remote", "add", "origin", remote);
writeFileSync(join(project, "README.md"), "# Fixture\n\nFirst line.\n");
writeFileSync(join(project, "app.ts"), "export const answer = 41;\n");
mkdirSync(join(project, "src/lib"), { recursive: true });
writeFileSync(join(project, "src/index.ts"), "import { util } from './lib/util';\nconsole.log(util());\n");
writeFileSync(join(project, "src/lib/util.ts"), "export const util = () => 1;\n");
writeFileSync(join(project, "readonly.txt"), "locked\n");
writeFileSync(join(project, "latin1.txt"), Buffer.from([0x63, 0x61, 0x66, 0xe9, 0x0a]));
writeFileSync(join(project, "big.txt"), "x".repeat(120) + "\n".repeat(1) + Array.from({ length: 60000 }, (_, i) => `line ${i} ${"lorem ipsum ".repeat(4)}`).join("\n"));
git(project, "add", ".");
git(project, "commit", "-qm", "Initial commit");
for (let i = 1; i <= 130; i++) git(project, "commit", "-q", "--allow-empty", "-m", `Commit ${i}`);
git(project, "tag", "v1.0");
git(project, "push", "-q", "origin", "main");
git(project, "checkout", "-qb", "feature/web");
writeFileSync(join(project, "app.ts"), "export const answer = 42;\n");
git(project, "commit", "-qam", "Fix the answer");
symlinkSync("README.md", join(project, "link.md"));
chmodSync(join(project, "readonly.txt"), 0o444);
writeFileSync(join(project, "README.md"), "# Fixture\n\nFirst line.\nSecond line.\n");
writeFileSync(join(project, "notes.txt"), "untracked\n");

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets });
const page = await openBrowser(engine.url, { width: 1500, height: 900 });
await page.call("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => { const Native = WebSocket; window.__sockets = []; window.__offline = false;
    window.WebSocket = class extends Native { constructor(url, protocols) { super(window.__offline ? "ws://127.0.0.1:9/" : url, protocols); window.__sockets.push(this); } }; })();`,
});
await page.call("Page.reload");
await sleep(500);

const results = [];
const step = async (name, fn) => {
  if (only && !only.has(name)) return;
  process.stdout.write(`- ${name} … `);
  try {
    await fn();
    results.push([name, "ok"]);
    console.log("ok");
  } catch (e) {
    results.push([name, `FAIL ${e.message}`]);
    console.log(`FAIL ${e.message}`);
    if (shots) await page.screenshot(join(shots, `fail-${name}.png`)).catch(() => {});
  }
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `${name}.png`)));
const text = (sel) => page.eval(`document.querySelector(${JSON.stringify(sel)})?.textContent ?? ""`);
const pickMenu = async (label) => {
  const find = `[...document.querySelectorAll(".menu-item")].find((m) => m.textContent.trim() === ${JSON.stringify(label)})`;
  await page.waitFor(`!!${find}`);
  await page.eval(`(() => { document.querySelectorAll("[data-e2e-pick]").forEach((e) => e.removeAttribute("data-e2e-pick")); ${find}.setAttribute("data-e2e-pick", ""); return true })()`);
  await page.press("[data-e2e-pick]");
};
const addSurface = async (label) => {
  if (await page.eval(`!!document.querySelector('button[aria-label="Show panel"]')`)) { await page.click('button[aria-label="Show panel"]'); await sleep(200); }
  if (await page.eval(`!!document.querySelector('.ws-tab-main[aria-label=${JSON.stringify(label)}]')`)) {
    await page.click(`.ws-tab-main[aria-label=${JSON.stringify(label)}]`);
    return;
  }
  await sleep(400);
  await page.press('button[aria-label="Add to panel"]');
  await pickMenu(label);
};
const focusType = async (sel, value) => {
  await page.eval(`(() => { const i = document.querySelector(${JSON.stringify(sel)}); i.focus(); i.select?.(); return true })()`);
  await page.type(value);
};
const clickText = (scope, tag, label) => page.eval(`(() => { const b = [...document.querySelectorAll(${JSON.stringify(`${scope} ${tag}`)})].find((b) => b.textContent.trim() === ${JSON.stringify(label)}); if (!b) throw new Error("missing ${label}"); b.click(); return true })()`);
const drop = async () => {
  await page.eval(`(() => { window.__offline = true; for (const s of window.__sockets) s.close(); return true })()`);
  await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 15000);
};
const restore = async () => {
  await page.eval(`(() => { window.__offline = false; return true })()`);
  await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
};
const enter = async () => { for (const type of ["keyDown", "keyUp"]) await page.call("Input.dispatchKeyEvent", { type, key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, ...(type === "keyDown" ? { text: "\r" } : {}) }); };
const toTree = async () => {
  if (await page.eval(`!!document.querySelector('#ws-surface-files button[aria-label="Back to files"]')`)) {
    await page.click('#ws-surface-files button[aria-label="Back to files"]');
    await sleep(200);
    await page.eval(`[...document.querySelectorAll(".dialog button")].find((b) => b.textContent === "Discard")?.click(); true`);
    await sleep(200);
  }
};
const dismissNotice = () => page.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Dismiss")?.click() ?? true`);
const toastText = () => page.eval(`[...document.querySelectorAll(".toast, [role=status]")].map((t) => t.textContent).join(" | ")`);

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "ws-space", deviceId, path: project, name: "fixture", gitDetected: true });
  await engine.rpc("Mutate", { op: "createChat", chatId: "ws-chat", spaceId: "ws-space", branch: "feature/web", config: { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" } });
  await page.waitFor(`!!document.querySelector('[data-chat-id="ws-chat"] .sb-row-open')`);
  await page.click(`[data-chat-id="ws-chat"] .sb-row-open`);
  await page.waitFor(`window.__zuse.view.selectedChatId === "ws-chat"`);
  await page.waitFor(`document.querySelector(".ws-branch-label")?.textContent === "feature/web"`);

  await step("branch-refused", async () => {
    writeFileSync(join(project, "app.ts"), "export const answer = 99;\n");
    await page.waitFor(`/· 4/.test(document.querySelector(".ws-branch-trigger")?.textContent ?? "")`, 20000);
    await page.press(".ws-branch-trigger");
    await page.waitFor(`[...document.querySelectorAll(".ws-branch-row")].some((r) => r.textContent.includes("main"))`);
    await page.eval(`[...document.querySelectorAll(".ws-branch-row")].find((r) => r.textContent.trim() === "main").click()`);
    await page.waitFor(`!!document.querySelector(".ws-branch-menu .ws-danger")`, 15000);
    await shot("branch-refused");
    const branch = git(project, "branch", "--show-current").trim();
    if (branch !== "feature/web") throw new Error(`switched anyway to ${branch}`);
  });

  await step("branch-copy", async () => {
    if (!(await page.eval(`!!document.querySelector(".ws-branch-menu")`))) await page.press(".ws-branch-trigger");
    await page.waitFor(`!!document.querySelector(".ws-branch-menu")`);
    await page.click('.ws-branch-menu button[aria-label="Copy branch name"]');
    await sleep(300);
    const t = await toastText();
    await shot("branch-copy");
    if (!/copied/i.test(t)) throw new Error(`no toast: ${t}`);
    await page.key("Escape", "Escape", 27);
  });

  await step("branch-switch-clean", async () => {
    git(project, "checkout", "--", "app.ts");
    await page.waitFor(`/· 3/.test(document.querySelector(".ws-branch-trigger")?.textContent ?? "")`, 20000);
    if (!(await page.eval(`!!document.querySelector(".ws-branch-menu")`))) await page.press(".ws-branch-trigger");
    await page.waitFor(`[...document.querySelectorAll(".ws-branch-row")].some((r) => r.textContent.trim() === "main")`);
    await page.eval(`[...document.querySelectorAll(".ws-branch-row")].find((r) => r.textContent.trim() === "main").click()`);
    await page.waitFor(`!document.querySelector(".ws-branch-menu")`, 15000);
    const branch = git(project, "branch", "--show-current").trim();
    if (branch !== "main") throw new Error(`branch is ${branch}`);
    await sleep(1500);
    const label = await text(".ws-branch-label");
    await shot("branch-switched");
    // switch back for the rest, outside the app
    git(project, "checkout", "-q", "feature/web");
    if (label !== "main") throw new Error(`top bar label after switch: ${label}`);
    await page.waitFor(`document.querySelector(".ws-branch-label")?.textContent === "feature/web"`, 15000);
  });

  await step("shortcuts", async () => {
    await page.eval(`document.activeElement?.blur(); true`);
    await page.key("e", "KeyE", 69, 2);
    await page.waitFor(`window.__zuse.view.rightPanel === "files"`, 3000);
    await page.key("j", "KeyJ", 74, 2);
    await page.waitFor(`window.__zuse.view.rightPanel === "terminal"`, 3000);
    await page.key("b", "KeyB", 66, 2 | 1);
    await page.waitFor(`window.__zuse.view.rightPanel === null`, 3000);
    await page.key("b", "KeyB", 66, 2 | 1);
    await page.waitFor(`window.__zuse.view.rightPanel === "terminal"`, 3000);
    await page.click("#ws-tab-files");
  });

  await step("tree-expand-keyboard", async () => {
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src"]')`, 15000);
    await page.click('.ws-tree-row[data-path="src"]');
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src/lib"]')`);
    await page.eval(`document.querySelector('.ws-tree-row[data-path="src"]').focus(); true`);
    await page.key("ArrowDown", "ArrowDown", 40);
    const sel = await page.eval(`document.activeElement?.dataset.path`);
    if (sel !== "src/lib") throw new Error(`down went to ${sel}`);
    await page.key("ArrowRight", "ArrowRight", 39);
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src/lib/util.ts"]')`);
    await page.key("ArrowRight", "ArrowRight", 39);
    await page.waitFor(`document.activeElement?.dataset.path === "src/lib/util.ts"`);
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await page.waitFor(`document.activeElement?.dataset.path === "src/lib"`);
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await page.waitFor(`!document.querySelector('.ws-tree-row[data-path="src/lib/util.ts"]')`);
    await shot("tree-keyboard");
  });

  await step("tree-external-rename", async () => {
    renameSync(join(project, "notes.txt"), join(project, "notez.txt"));
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="notez.txt"]')`, 15000);
    const names = await page.eval(`[...document.querySelectorAll(".ws-tree-row[data-path]")].map((r) => r.dataset.path + "=" + r.querySelector(".ws-tree-name").textContent)`);
    const bad = names.filter((n) => { const [p, name] = n.split("="); return p.split("/").pop() !== name; });
    renameSync(join(project, "notez.txt"), join(project, "notes.txt"));
    if (bad.length) throw new Error(`stale names: ${bad}`);
  });

  await step("readonly-files", async () => {
    const reasons = {};
    for (const name of ["readonly.txt", "latin1.txt", "link.md", "big.txt"]) {
      await toTree();
      await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="${name}"]')`, 10000).catch(async () => {
        await page.eval(`document.querySelector('.ws-tree')?.scrollTo(0, 99999); true`);
        await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="${name}"]')`, 5000);
      });
      await page.click(`.ws-tree-row[data-path="${name}"]`);
      await page.waitFor(`document.querySelector(".ws-editor-name")?.textContent.startsWith(${JSON.stringify(name)})`, 10000);
      await sleep(800);
      reasons[name] = { banner: await text(".ws-editor-pane .ws-banner"), editable: await page.eval(`document.querySelector(".cm-content")?.getAttribute("contenteditable")`) };
      await shot(`readonly-${name}`);
    }
    for (const [name, want] of [["latin1.txt", /UTF-8/], ["link.md", /Symbolic links/], ["big.txt", /too large/]]) if (!want.test(reasons[name].banner)) throw new Error(`${name}: ${reasons[name].banner}`);
  });

  await step("delete-open-file", async () => {
    await toTree();
    await page.click('.ws-tree-row[data-path="src"]').catch(() => {});
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src/index.ts"]')`).catch(async () => { await page.click('.ws-tree-row[data-path="src"]'); });
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src/index.ts"]')`);
    await page.click('.ws-tree-row[data-path="src/index.ts"]');
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("console.log")`);
    await page.press(".cm-content .cm-line");
    await page.key("End", "End", 35);
    await page.type(" // mine");
    await page.waitFor(`!!document.querySelector(".ws-dirty")`);
    unlinkSync(join(project, "src/index.ts"));
    await page.waitFor(`!!document.querySelector(".ws-editor-pane .ws-banner")`, 15000);
    await shot("deleted-open");
    const banner = await text(".ws-editor-pane .ws-banner");
    git(project, "checkout", "--", "src/index.ts");
    if (!/delet|removed|no longer/i.test(banner)) throw new Error(`banner: ${banner}`);
    await page.eval(`[...document.querySelectorAll(".ws-editor-pane .ws-banner button")].find((b) => b.textContent === "Reload")?.click(); true`);
  });

  await step("readonly-typing", async () => {
    await toTree();
    await page.click('.ws-tree-row[data-path="latin1.txt"]');
    await page.waitFor(`document.querySelector(".ws-editor-name")?.textContent.startsWith("latin1.txt")`, 10000);
    await sleep(500);
    await page.press(".cm-content .cm-line");
    await page.type("zzz");
    await sleep(300);
    const t = await page.eval(`document.querySelector(".cm-content").textContent`);
    const ro = await page.eval(`document.querySelector(".cm-content").getAttribute("aria-readonly")`);
    if (t.includes("zzz") || (await page.eval(`!!document.querySelector(".ws-dirty")`))) throw new Error("read-only file accepted typing");
    if (ro !== "true") throw new Error(`aria-readonly ${ro}`);
  });

  await step("save-failure", async () => {
    await toTree();
    if (!(await page.eval(`!!document.querySelector('.ws-tree-row[data-path="src/lib/util.ts"]')`))) {
      if (!(await page.eval(`!!document.querySelector('.ws-tree-row[data-path="src/lib"]')`))) await page.click('.ws-tree-row[data-path="src"]');
      await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src/lib"]')`);
      await page.click('.ws-tree-row[data-path="src/lib"]');
    }
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="src/lib/util.ts"]')`);
    await page.click('.ws-tree-row[data-path="src/lib/util.ts"]');
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("util")`);
    chmodSync(join(project, "src/lib"), 0o555);
    try {
      await page.press(".cm-content .cm-line");
      await page.key("End", "End", 35);
      await page.type(" // x");
      await page.waitFor(`!!document.querySelector(".ws-dirty")`);
      await page.key("s", "KeyS", 83, 2);
      await sleep(2500);
      const banner = await page.eval(`[...document.querySelectorAll(".ws-editor-pane .ws-banner")].map((b) => b.textContent).join(" | ")`);
      await shot("save-failed");
      const disk = readFileSync(join(project, "src/lib/util.ts"), "utf8");
      console.log(" banner:", banner, "| dirty:", await page.eval(`!!document.querySelector(".ws-dirty")`));
      if (disk.includes("// x")) console.log(" (engine wrote anyway)");
      else if (!banner) throw new Error("no error shown for failed save");
    } finally {
      chmodSync(join(project, "src/lib"), 0o755);
    }
    await page.key("s", "KeyS", 83, 2);
    await page.waitFor(`!document.querySelector(".ws-dirty")`, 10000);
    if (!readFileSync(join(project, "src/lib/util.ts"), "utf8").includes("// x")) throw new Error("retry save did not write");
    git(project, "checkout", "--", "src/lib/util.ts");
  });

  await step("panel-controls", async () => {
    await addSurface("Changes");
    await addSurface("History");
    const width0 = await page.eval(`document.querySelector(".ws-right").getBoundingClientRect().width`);
    await page.eval(`document.querySelector('.app-seam[aria-label="Resize panel"]').focus(); true`);
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await sleep(100);
    const width1 = await page.eval(`document.querySelector(".ws-right").getBoundingClientRect().width`);
    if (!(width1 > width0)) throw new Error(`keyboard resize ${width0} -> ${width1}`);
    // pointer drag
    const box = await page.eval(`(() => { const r = document.querySelector('.app-seam[aria-label="Resize panel"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + 200 } })()`);
    await page.call("Input.dispatchMouseEvent", { type: "mouseMoved", x: box.x, y: box.y });
    await page.call("Input.dispatchMouseEvent", { type: "mousePressed", x: box.x, y: box.y, button: "left", clickCount: 1 });
    await page.call("Input.dispatchMouseEvent", { type: "mouseMoved", x: box.x - 100, y: box.y, button: "left", buttons: 1 });
    await page.call("Input.dispatchMouseEvent", { type: "mouseReleased", x: box.x - 100, y: box.y, button: "left", clickCount: 1 });
    await sleep(100);
    const width2 = await page.eval(`document.querySelector(".ws-right").getBoundingClientRect().width`);
    if (Math.abs(width2 - width1 - 100) > 4) throw new Error(`drag resize ${width1} -> ${width2}`);
    await page.click('button[aria-label="Expand panel"]');
    await sleep(400); // the panel eases to its new width (up to 240ms)
    const width3 = await page.eval(`document.querySelector(".ws-right").getBoundingClientRect().width`);
    await shot("panel-expanded");
    await page.click('button[aria-label="Collapse panel"]');
    await sleep(400);
    const width4 = await page.eval(`document.querySelector(".ws-right").getBoundingClientRect().width`);
    if (!(width3 > width2) || Math.abs(width4 - width2) > 2) throw new Error(`expand ${width2} -> ${width3} -> ${width4}`);
    // middle click closes History
    const hb = await page.eval(`(() => { const r = document.querySelector("#ws-tab-history").getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 } })()`);
    for (const type of ["mousePressed", "mouseReleased"]) await page.call("Input.dispatchMouseEvent", { type, x: hb.x, y: hb.y, button: "middle", clickCount: 1 });
    await page.waitFor(`!document.querySelector("#ws-tab-history")`, 3000);
    await page.click('button[aria-label="Close Changes"]');
    await page.waitFor(`!document.querySelector("#ws-tab-changes")`, 3000);
    await page.click('button[aria-label="Close panel"]');
    await page.waitFor(`window.__zuse.view.rightPanel === null`);
    await page.click('button[aria-label="Show panel"]');
    await page.waitFor(`window.__zuse.view.rightPanel !== null`);
  });

  await step("changes-scopes", async () => {
    await addSurface("Changes");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-file-name")].some((e) => e.textContent === "README.md")`, 20000);
    await shot("changes-wt");
    await page.press('#ws-surface-changes button[aria-label^="Diff scope"]');
    await pickMenu("Branch changes");
    await page.waitFor(`/vs main/.test(document.querySelector("#ws-surface-changes .ws-diff-tools")?.textContent ?? "")`, 20000);
    const names = await page.eval(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-file-name")].map((e) => e.textContent).join(",")`);
    await shot("changes-branch");
    if (!names.includes("app.ts")) throw new Error(`branch diff: ${names}`);
    await page.press('#ws-surface-changes button[aria-label^="Diff scope"]');
    await pickMenu("Latest turn");
    await page.waitFor(`/turn/i.test(document.querySelector("#ws-surface-changes")?.textContent ?? "")`, 15000);
    await sleep(1000);
    await shot("changes-turn");
    await page.press('#ws-surface-changes button[aria-label^="Diff scope"]');
    await pickMenu("Working tree");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-file-name")].some((e) => e.textContent === "README.md")`, 20000);
  });

  await step("changes-wrap-collapse-open", async () => {
    writeFileSync(join(project, "README.md"), "# Fixture\n\nFirst line.\nSecond line. " + "very long words ".repeat(40) + "\n");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-text")].some((e) => e.textContent.includes("very long"))`, 20000);
    await page.click('#ws-surface-changes button[aria-label="Wrap long lines"]');
    await sleep(300);
    await shot("changes-wrapped");
    const h = await page.eval(`Math.max(...[...document.querySelectorAll("#ws-surface-changes .ws-diff-line")].map((e) => e.getBoundingClientRect().height))`);
    if (h < 30) throw new Error(`wrap did not wrap: max line height ${h}`);
    await page.click('#ws-surface-changes button[aria-label="Collapse all files"]');
    await sleep(200);
    const lines = await page.eval(`document.querySelectorAll("#ws-surface-changes .ws-diff-line").length`);
    if (lines) throw new Error(`collapsed still shows ${lines} lines`);
    await shot("changes-collapsed");
    await page.click('#ws-surface-changes button[aria-label="Expand all files"]');
    await page.click('#ws-surface-changes .ws-changed-toggle');
    await shot("changes-filelist");
    await page.click('#ws-surface-changes button[aria-label="Open README.md"]');
    await page.waitFor(`window.__zuse.view.rightPanel === "files" && document.querySelector(".ws-editor-name")?.textContent.startsWith("README.md")`, 10000);
  });

  await step("changes-discard", async () => {
    await page.click("#ws-tab-changes");
    await page.click('#ws-surface-changes button[aria-label="Discard all changes"]');
    await page.waitFor(`/Discard all changes/.test(document.querySelector(".dialog")?.textContent ?? "")`);
    await shot("discard-dialog");
    await clickText(".dialog", "button", "Cancel");
    await page.waitFor(`!document.querySelector(".dialog")`);
    if (!existsSync(join(project, "notes.txt"))) throw new Error("cancel discarded");
    await page.click('#ws-surface-changes button[aria-label="Discard all changes"]');
    await page.waitFor(`!!document.querySelector(".dialog")`);
    await clickText(".dialog", "button", "Discard");
    await page.waitFor(`/No uncommitted changes/.test(document.querySelector("#ws-surface-changes")?.textContent ?? "")`, 20000);
    await shot("discarded");
    const status = git(project, "status", "--porcelain");
    if (status.trim()) throw new Error(`still dirty: ${status}`);
  });

  await step("git-stage-unstage-commit", async () => {
    writeFileSync(join(project, "README.md"), "# Fixture\n\nChanged.\n");
    writeFileSync(join(project, "new.txt"), "new\n");
    await addSurface("Git");
    await page.waitFor(`document.querySelectorAll("#ws-surface-git .ws-git-files .ws-git-row").length === 2`, 20000);
    await shot("git-two");
    await page.click('#ws-surface-git .ws-git-files button[aria-label^="new.txt"]');
    await page.waitFor(`document.querySelector('#ws-surface-git .ws-git-files button[aria-label^="new.txt"]')?.getAttribute("aria-checked") === "true"`, 15000);
    const all = await page.eval(`document.querySelector('#ws-surface-git button[aria-label="Stage all changes"]').getAttribute("aria-checked")`);
    if (all !== "mixed") throw new Error(`all=${all}`);
    await page.click('#ws-surface-git .ws-git-files button[aria-label^="new.txt"]');
    await page.waitFor(`document.querySelector('#ws-surface-git .ws-git-files button[aria-label^="new.txt"]')?.getAttribute("aria-checked") === "false"`, 15000);
    await page.click('#ws-surface-git .ws-git-files button[aria-label^="README.md"]');
    await page.waitFor(`document.querySelector('#ws-surface-git .ws-git-files button[aria-label^="README.md"]')?.getAttribute("aria-checked") === "true"`, 15000);
    const disabled = await page.eval(`document.querySelector('#ws-surface-git .ws-commit-form button[type="submit"]').disabled`);
    if (!disabled) throw new Error("commit enabled without a message");
    await focusType('#ws-surface-git input[aria-label="Commit message"]', "Only the readme");
    await enter();
    await page.waitFor(`document.querySelectorAll("#ws-surface-git .ws-git-files .ws-git-row").length === 1`, 20000);
    const log = git(project, "log", "-1", "--format=%s").trim();
    const shown = git(project, "show", "--stat", "--format=", "HEAD");
    await shot("git-committed");
    if (log !== "Only the readme" || shown.includes("new.txt")) throw new Error(`commit: ${log} ${shown}`);
  });

  await step("git-publish", async () => {
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-git .ws-panel-header button")].some((b) => b.textContent.trim() === "Publish")`, 10000);
    await clickText("#ws-surface-git .ws-panel-header", "button", "Publish");
    await page.waitFor(`![...document.querySelectorAll("#ws-surface-git .ws-panel-header button")].some((b) => b.textContent.trim() === "Publish") && !/…/.test(document.querySelector("#ws-surface-git .ws-panel-header").textContent)`, 30000);
    await shot("git-published");
    const remoteRef = git(remote, "rev-parse", "feature/web").trim();
    const local = git(project, "rev-parse", "HEAD").trim();
    const fail = await text("#ws-surface-git .ws-banner");
    if (remoteRef !== local) throw new Error(`remote ${remoteRef} local ${local} ${fail}`);
  });

  await step("git-pull", async () => {
    git(root, "clone", "-q", "-b", "feature/web", remote, other);
    writeFileSync(join(other, "remote.txt"), "from elsewhere\n");
    git(other, "add", ".");
    git(other, "commit", "-qm", "Remote change");
    git(other, "push", "-q", "origin", "feature/web");
    await page.press('#ws-surface-git button[aria-label="More Git actions"]');
    await pickMenu("Fetch");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-git .ws-panel-header button")].some((b) => b.textContent.trim() === "Pull 1")`, 30000);
    await shot("git-behind");
    await clickText("#ws-surface-git .ws-panel-header", "button", "Pull 1");
    await page.waitFor(`![...document.querySelectorAll("#ws-surface-git .ws-panel-header button")].some((b) => /^Pull/.test(b.textContent.trim())) && !/…/.test(document.querySelector("#ws-surface-git .ws-panel-header").textContent)`, 30000);
    const fail = await text("#ws-surface-git .ws-banner");
    if (!existsSync(join(project, "remote.txt"))) throw new Error(`pull did not land ${fail}`);
  });

  await step("git-push", async () => {
    await page.click('#ws-surface-git .ws-git-files button[aria-label^="new.txt"]');
    await page.waitFor(`document.querySelector('#ws-surface-git .ws-git-files button[aria-label^="new.txt"]')?.getAttribute("aria-checked") === "true"`, 15000);
    await focusType('#ws-surface-git input[aria-label="Commit message"]', "Add new file");
    await page.click('#ws-surface-git .ws-commit-form button[type="submit"]');
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-git .ws-panel-header button")].some((b) => b.textContent.trim() === "Push 1")`, 30000);
    await shot("git-ahead");
    await clickText("#ws-surface-git .ws-panel-header", "button", "Push 1");
    await page.waitFor(`![...document.querySelectorAll("#ws-surface-git .ws-panel-header button")].some((b) => /^Push/.test(b.textContent.trim())) && !/…/.test(document.querySelector("#ws-surface-git .ws-panel-header").textContent)`, 30000);
    if (git(remote, "rev-parse", "feature/web").trim() !== git(project, "rev-parse", "HEAD").trim()) throw new Error("push missing");
    await shot("git-pushed");
  });

  await step("git-new-branch-and-switch", async () => {
    await addSurface("Git");
    await page.waitFor(`!!document.querySelector('#ws-surface-git button[aria-label="More Git actions"]')`, 15000);
    await page.press('#ws-surface-git button[aria-label="More Git actions"]');
    await pickMenu("New branch…");
    await page.waitFor(`!!document.querySelector('#ws-surface-git input[aria-label="New branch name"]')`);
    await sleep(700);
    const active = await page.eval(`document.activeElement?.getAttribute("aria-label")`);
    if (active !== "New branch name") throw new Error(`focus on ${active}`);
    await page.type("qa/new");
    await shot("git-new-branch-form");
    await enter();
    await page.waitFor(`document.querySelector("#ws-surface-git .ws-panel-title")?.textContent === "qa/new"`, 20000);
    const cur = git(project, "branch", "--show-current").trim();
    if (cur !== "qa/new") throw new Error(`branch ${cur}`);
    await sleep(1500);
    const top = await text(".ws-branch-label");
    await page.press('#ws-surface-git button[aria-label="More Git actions"]');
    await pickMenu("Switch branch…");
    await sleep(700);
    await page.type("feature/web");
    await enter();
    await page.waitFor(`document.querySelector("#ws-surface-git .ws-panel-title")?.textContent === "feature/web"`, 20000);
    console.log(` top bar while on qa/new: ${top}`);
  });

  await step("git-switch-missing", async () => {
    await page.press('#ws-surface-git button[aria-label="More Git actions"]');
    await pickMenu("Switch branch…");
    await sleep(700);
    await page.type("does-not-exist");
    await enter();
    await page.waitFor(`!!document.querySelector("#ws-surface-git .ws-banner")`, 20000);
    await shot("git-switch-missing");
    console.log(" banner:", await text("#ws-surface-git .ws-banner"));
  });

  await step("git-create-pr-no-github", async () => {
    await page.press('#ws-surface-git button[aria-label="More Git actions"]');
    await pickMenu("Create pull request…");
    await sleep(700);
    await page.type("Title");
    if ((await page.eval(`document.querySelector('#ws-surface-git input[aria-label="Pull request title"]').value`)) !== "Title") throw new Error("PR title did not take focus");
    await shot("git-pr-form");
    await clickText("#ws-surface-git .ws-form", "button", "Create");
    await sleep(4000);
    await shot("git-pr-result");
    console.log(" banner:", await text("#ws-surface-git .ws-banner"));
  });

  await step("git-interrupted-commit", async () => {
    await dismissNotice();
    writeFileSync(join(project, "interrupt.txt"), "x\n");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-git .ws-git-files .ws-git-row")].some((r) => r.getAttribute("aria-label").startsWith("interrupt.txt"))`, 20000);
    await page.click('#ws-surface-git .ws-git-files button[aria-label^="interrupt.txt"]');
    await page.waitFor(`document.querySelector('#ws-surface-git .ws-git-files button[aria-label^="interrupt.txt"]')?.getAttribute("aria-checked") === "true"`, 15000);
    await focusType('#ws-surface-git input[aria-label="Commit message"]', "Interrupted");
    await page.eval(`(() => { document.querySelector('#ws-surface-git .ws-commit-form button[type="submit"]').click(); window.__offline = true; for (const s of window.__sockets) s.close(); return true })()`);
    await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 15000);
    await sleep(500);
    await shot("git-interrupted-offline");
    const offlineBanner = await text("#ws-surface-git .ws-banner");
    await restore();
    await sleep(2500);
    await shot("git-interrupted-back");
    console.log(" offline banner:", offlineBanner, "| after:", await text("#ws-surface-git .ws-banner"), "| msg:", await page.eval(`document.querySelector('#ws-surface-git input[aria-label="Commit message"]')?.value`));
    console.log(" log:", git(project, "log", "-1", "--format=%s").trim());
  });

  await step("history-paging-search", async () => {
    await dismissNotice();
    await addSurface("History");
    await page.waitFor(`document.querySelectorAll("#ws-surface-history .ws-commit").length > 5`, 20000);
    const total = await text("#ws-surface-history .ws-panel-header");
    await shot("history-top");
    for (let i = 0; i < 12; i++) {
      await page.eval(`(() => { const l = document.querySelector("#ws-surface-history .ws-history-list"); l.scrollTop = l.scrollHeight; return true })()`);
      await sleep(300);
    }
    const last = await page.eval(`[...document.querySelectorAll("#ws-surface-history .ws-commit-subject")].map((e) => e.textContent).pop()`);
    await shot("history-bottom");
    if (last !== "Initial commit") throw new Error(`bottom is ${last}; header ${total}`);
    await focusType('#ws-surface-history input[aria-label="Search commits"]', "Commit 12");
    await sleep(2000);
    const n = await page.eval(`[...document.querySelectorAll("#ws-surface-history .ws-commit-subject")].slice(0, 6).map((e) => e.textContent).join(",")`);
    const top = await page.eval(`document.querySelector("#ws-surface-history .ws-history-list")?.scrollTop`);
    await shot("history-search");
    console.log(" search hits:", n, "header:", await text("#ws-surface-history .ws-panel-header"), "scrollTop:", top);
    await focusType('#ws-surface-history input[aria-label="Search commits"]', "zzzznothing");
    await sleep(1500);
    await shot("history-search-empty");
    console.log(" empty search:", (await text("#ws-surface-history")).slice(0, 200));
    await page.eval(`(() => { const i = document.querySelector('#ws-surface-history input[aria-label="Search commits"]'); i.focus(); i.select(); return true })()`);
    await page.key("Backspace", "Backspace", 8);
    await page.waitFor(`document.querySelectorAll("#ws-surface-history .ws-commit").length > 5`, 15000);
  });

  await step("history-menu-keyboard", async () => {
    await page.eval(`document.querySelector("#ws-surface-history .ws-commit").focus(); true`);
    await page.key("ArrowDown", "ArrowDown", 40);
    await page.key("ArrowDown", "ArrowDown", 40);
    const focused = await page.eval(`document.activeElement?.querySelector?.(".ws-commit-subject")?.textContent`);
    console.log(" focused after 2 downs:", focused);
    const sha = await page.eval(`document.querySelector("#ws-surface-history .ws-commit").dataset.sha`);
    await page.eval(`(() => { const b = document.querySelector('#ws-surface-history .ws-commit button[aria-label^="Actions for"]'); b.setAttribute("data-e2e-act", ""); return true })()`);
    await page.press("[data-e2e-act]");
    await pickMenu("Copy commit SHA");
    await sleep(300);
    const t = await toastText();
    if (!/SHA copied/.test(t)) throw new Error(`toast ${t}`);
    const tagged = await page.eval(`[...document.querySelectorAll("#ws-surface-history .ws-ref")].map((r) => r.textContent).join(",")`);
    console.log(" refs:", tagged, sha.slice(0, 7));
  });

  await step("terminal-tabs-exit", async () => {
    await addSurface("Terminal");
    await page.waitFor(`document.querySelectorAll("#ws-surface-terminal .ws-term-tab").length >= 1`, 20000);
    await page.click('#ws-surface-terminal button[aria-label="New terminal"]');
    await page.waitFor(`document.querySelectorAll("#ws-surface-terminal .ws-term-tab").length === 2`, 15000);
    await sleep(800);
    await page.eval(`document.querySelector("#ws-surface-terminal .ws-term-view:not([hidden]) .xterm-helper-textarea, #ws-surface-terminal .ws-term-view[data-visible=true] .xterm-helper-textarea")?.focus() ?? [...document.querySelectorAll("#ws-surface-terminal .xterm-helper-textarea")].pop().focus(); true`);
    await page.type("echo second-$((2*3))");
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-terminal .xterm-rows")].some((r) => r.textContent.includes("second-6"))`, 10000);
    await shot("terminal-two");
    await page.type("exit");
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`/exited/.test(document.querySelector("#ws-surface-terminal .ws-term-tabs").textContent)`, 10000);
    await shot("terminal-exited");
    const closes = await page.eval(`document.querySelectorAll('#ws-surface-terminal .ws-term-tab .ws-tab-close').length`);
    await page.eval(`[...document.querySelectorAll('#ws-surface-terminal .ws-term-tab')].find((t) => /exited/.test(t.textContent)).querySelector(".ws-tab-close").click(); true`);
    await page.waitFor(`document.querySelectorAll("#ws-surface-terminal .ws-term-tab").length === ${closes - 1}`, 5000);
    const list = await engine.rpc("ListTerminals", { chatId: "ws-chat" }).catch((e) => e.message);
    console.log(" engine terminals:", JSON.stringify(list).slice(0, 300));
  });

  await step("terminal-resize", async () => {
    const before = await page.eval(`document.querySelector("#ws-surface-terminal .xterm-rows").children.length`);
    await page.click('button[aria-label="Expand panel"]');
    await sleep(600);
    await page.eval(`[...document.querySelectorAll("#ws-surface-terminal .xterm-helper-textarea")].pop().focus(); true`);
    await page.type("tput cols");
    await page.key("Enter", "Enter", 13);
    await sleep(800);
    const out = await page.eval(`document.querySelector("#ws-surface-terminal .xterm-rows").textContent`);
    const cols = await page.eval(`(() => { const t = document.querySelector("#ws-surface-terminal .xterm-rows").textContent; const m = t.match(/tput cols(\\d+)/); return m && +m[1] })()`);
    await page.click('button[aria-label="Collapse panel"]');
    console.log(" cols after expand:", cols, "rows", before);
    if (!cols || cols < 100) throw new Error(`cols ${cols} ${out.slice(-200)}`);
  });

  await step("browser-local-page", async () => {
    const port = 40000 + Math.floor(Math.random() * 2000);
    mkdirSync(join(root, "site"), { recursive: true });
    writeFileSync(join(root, "site/index.html"), "<!doctype html><title>QA page</title><h1 id=hello>Hello from QA</h1>");
    const server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1", "-d", join(root, "site")], { stdio: "ignore" });
    try {
      await sleep(800);
      await addSurface("Browser");
      // Surfaces load on first open.
      await page.waitFor(`!!document.querySelector('#ws-surface-browser input[aria-label="Web address"]')`, 10000);
      await focusType('#ws-surface-browser input[aria-label="Web address"]', `127.0.0.1:${port}`);
      await enter();
      await page.waitFor(`!!document.querySelector("#ws-surface-browser iframe")`, 10000);
      await sleep(1500);
      await shot("browser-page");
      const src = await page.eval(`document.querySelector("#ws-surface-browser iframe").src`);
      const loading = await page.eval(`!!document.querySelector("#ws-surface-browser .loader")`);
      console.log(" iframe:", src, "loading:", loading, "banner:", await text("#ws-surface-browser .ws-banner"));
      await focusType('#ws-surface-browser input[aria-label="Web address"]', "javascript:alert(1)");
      await enter();
      await sleep(300);
      await shot("browser-js");
      const refused = await page.eval(`[...document.querySelectorAll("#ws-surface-browser .ws-banner")].map((b) => b.textContent).join(" | ")`);
      if (!/Use an HTTP or HTTPS address/.test(refused) || /Open in new tab/.test(refused)) throw new Error(`refusal banner: ${refused}`);
      if (!(await page.eval(`!!document.querySelector("#ws-surface-browser iframe")`))) throw new Error("a refused address hid the current page");
    } finally {
      server.kill();
    }
  });

  await step("session-card", async () => {
    await dismissNotice();
    await page.press('button[aria-label="Toggle environment summary"]');
    await page.waitFor(`!!document.querySelector(".ws-session-card")`);
    await shot("session-card");
    const rows = await page.eval(`[...document.querySelectorAll(".ws-session-card button, .ws-session-card [role=button]")].map((b) => b.textContent.trim())`);
    console.log(" session buttons:", rows);
    // Desktop's card has no panel shortcuts; Branch changes opens the review.
    const changesBtn = await page.eval(`[...document.querySelectorAll(".ws-session-card button")].find((b) => b.textContent.includes("Branch changes"))?.click() ?? "none"`);
    await sleep(300);
    const panel = await page.eval(`window.__zuse.view.rightPanel`);
    if (panel !== "changes") throw new Error(`Branch changes row -> ${panel} ${changesBtn}`);
  });

  await step("light-theme", async () => {
    await page.eval(`(() => { document.documentElement.dataset.theme = "light"; return true })()`);
    for (const s of ["Git", "History", "Changes", "Terminal", "Files"]) {
      await addSurface(s);
      await sleep(600);
      await shot(`light-${s}`);
    }
    await page.eval(`(() => { document.documentElement.dataset.theme = "dark"; return true })()`);
  });

  await step("reconnect-surfaces", async () => {
    await addSurface("History");
    await drop();
    await sleep(800);
    await shot("offline-history");
    for (const s of ["Git", "Changes", "Files", "Terminal"]) { await addSurface(s); await sleep(300); await shot(`offline-${s}`); }
    writeFileSync(join(project, "offline.txt"), "x\n");
    await restore();
    await sleep(3000);
    await addSurface("Changes");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-file-name")].some((e) => e.textContent === "offline.txt")`, 20000);
    await addSurface("Git");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-git .ws-git-row")].some((r) => (r.getAttribute("aria-label") ?? "").startsWith("offline.txt"))`, 20000);
  });

  const errors = page.errors.filter((e) => !/favicon/.test(e));
  if (errors.length) throw new Error(`page errors:\n${errors.join("\n")}`);
} catch (error) {
  console.error(error);
  await shot("fatal").catch(() => {});
  process.exitCode = 1;
} finally {
  const failed = results.filter(([, r]) => r !== "ok");
  if (failed.length) {
    console.error(`${failed.length} step(s) failed: ${failed.map(([n]) => n).join(", ")}`);
    process.exitCode = 1;
  } else if (!process.exitCode) console.log("workspace deep e2e ok");
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(root, { recursive: true, force: true });
}
