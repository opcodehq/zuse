// Workspace area end to end: top bar, files and editor save, changes, Git, history, terminal and browser
// against a disposable engine and a throwaway Git repository under the system temp directory.
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
const git = (cwd, ...args) => execFileSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" } });

const project = mkdtempSync(join(tmpdir(), "zuse-ws-project-"));
git(project, "init", "-q", "-b", "main");
writeFileSync(join(project, "README.md"), "# Fixture\n\nFirst line.\n");
writeFileSync(join(project, "app.ts"), "export const answer = 41;\n");
git(project, "add", ".");
git(project, "commit", "-qm", "Initial commit");
git(project, "checkout", "-qb", "feature/web");
writeFileSync(join(project, "app.ts"), "export const answer = 42;\n");
git(project, "commit", "-qam", "Fix the answer");
git(project, "checkout", "-q", "main");
git(project, "merge", "-q", "--no-ff", "feature/web", "-m", "Merge feature/web");
git(project, "checkout", "-q", "feature/web");
writeFileSync(join(project, "README.md"), "# Fixture\n\nFirst line.\nSecond line.\n");
writeFileSync(join(project, "notes.txt"), "untracked\n");
writeFileSync(join(project, "pixel.png"), Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR4nGP8z8DAwMDAxMDAwMDAAAANHQEDasKb6QAAAABJRU5ErkJggg==", "base64"));

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets });
const page = await openBrowser(engine.url, { width: 1500, height: 900 });
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  await fn();
  console.log("ok");
};
const addSurface = async (label) => {
  await page.press('button[aria-label="Add to panel"]');
  const find = `[...document.querySelectorAll(".menu-item")].find((m) => m.textContent.trim() === ${JSON.stringify(label)})`;
  await page.waitFor(`!!${find}`);
  await page.eval(`${find}.setAttribute("data-e2e-pick", "")`);
  await page.press("[data-e2e-pick]");
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `${name}.png`)));

// Test-only socket control: track sockets and refuse new ones while "offline".
await page.call("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => { const Native = WebSocket; window.__sockets = []; window.__offline = false;
    window.WebSocket = class extends Native { constructor(url, protocols) { super(window.__offline ? "ws://127.0.0.1:9/" : url, protocols); window.__sockets.push(this); } }; })();`,
});
await page.call("Page.reload");
await new Promise((r) => setTimeout(r, 500));

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "ws-space", deviceId, path: project, name: "fixture", gitDetected: true });
  await engine.rpc("Mutate", { op: "createChat", chatId: "ws-chat", spaceId: "ws-space", branch: "feature/web", config: { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" } });
  await page.waitFor(`!!document.querySelector('[data-chat-id="ws-chat"] .sb-row-open')`);
  await page.click(`[data-chat-id="ws-chat"] .sb-row-open`);
  await page.waitFor(`window.__zuse.view.selectedChatId === "ws-chat"`);

  await step("top bar shows the branch and changed-file count", async () => {
    await page.waitFor(`document.querySelector(".ws-branch-label")?.textContent === "feature/web"`);
    await page.waitFor(`/· 3/.test(document.querySelector(".ws-branch-trigger")?.textContent ?? "")`, 20000);
  });

  await step("branch menu lists local branches", async () => {
    await page.press(".ws-branch-trigger");
    await page.waitFor(`[...document.querySelectorAll(".ws-branch-row")].map((r) => r.textContent).join(",").includes("main")`);
    await shot("branch-menu");
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".ws-branch-menu")`);
  });

  await step("files tree lists the checkout with Git decorations", async () => {
    await page.click(`button[aria-label="Show panel"]`);
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="README.md"]')`);
    await page.waitFor(`document.querySelector('.ws-tree-row[data-path="README.md"]')?.dataset.deco === "modified"`);
    await page.waitFor(`document.querySelector('.ws-tree-row[data-path="notes.txt"]')?.dataset.deco === "added"`);
  });

  await step("editor opens, edits and saves", async () => {
    await page.click('.ws-tree-row[data-path="app.ts"]');
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("answer = 42")`);
    await page.press(".cm-content .cm-line");
    await page.key("End", "End", 35);
    await page.type(" // edited");
    await page.waitFor(`!!document.querySelector(".ws-dirty")`);
    await shot("editor-dirty");
    await page.key("s", "KeyS", 83, 2);
    await page.waitFor(`!document.querySelector(".ws-dirty")`);
    const disk = readFileSync(join(project, "app.ts"), "utf8");
    if (!disk.includes("// edited")) throw new Error(`save did not reach disk: ${disk}`);
  });

  await step("external change while editing raises a conflict instead of overwriting", async () => {
    await page.type(" more");
    await page.waitFor(`!!document.querySelector(".ws-dirty")`);
    writeFileSync(join(project, "app.ts"), "export const answer = 43;\n");
    await page.waitFor(`/changed on disk/.test(document.querySelector(".ws-editor-pane .ws-banner")?.textContent ?? "")`, 20000);
    await shot("editor-conflict");
    await page.eval(`[...document.querySelectorAll(".ws-editor-pane .ws-banner button")].find((b) => b.textContent === "Reload").click()`);
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("answer = 43")`);
  });

  await step("changes review shows the working tree diff", async () => {
    await addSurface("Changes");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-file-name")].map((e) => e.textContent).includes("README.md")`, 20000);
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-changes .ws-diff-line[data-kind=add] .ws-diff-text")].some((e) => e.textContent === "Second line.")`);
    await shot("changes");
  });

  await step("git panel stages and commits", async () => {
    await addSurface("Git");
    await page.waitFor(`!!document.querySelector('#ws-surface-git button[aria-label="Stage all changes"]')`, 20000);
    await page.click('#ws-surface-git button[aria-label="Stage all changes"]');
    await page.waitFor(`document.querySelector('#ws-surface-git button[aria-label="Stage all changes"]')?.getAttribute("aria-checked") === "true"`, 20000);
    await page.eval(`(() => { const input = document.querySelector('#ws-surface-git input[aria-label="Commit message"]'); input.focus(); return true })()`);
    await page.type("Commit from the web client");
    await page.click('#ws-surface-git .ws-commit-form button[type="submit"]');
    await page.waitFor(`/No local changes/.test(document.querySelector("#ws-surface-git")?.textContent ?? "")`, 20000);
    const log = git(project, "log", "-1", "--format=%s");
    if (log.trim() !== "Commit from the web client") throw new Error(`commit missing: ${log}`);
    await shot("git");
  });

  await step("history shows commits with the graph and opens a commit diff", async () => {
    await addSurface("History");
    await page.waitFor(`document.querySelectorAll("#ws-surface-history .ws-commit").length >= 4`, 20000);
    const subjects = await page.eval(`[...document.querySelectorAll("#ws-surface-history .ws-commit-subject")].map((e) => e.textContent)`);
    if (subjects[0] !== "Commit from the web client") throw new Error(`unexpected history: ${subjects}`);
    const lanes = await page.eval(`document.querySelectorAll("#ws-surface-history .ws-graph path").length`);
    if (!lanes) throw new Error("graph has no lanes");
    await shot("history");
    await page.eval(`[...document.querySelectorAll("#ws-surface-history .ws-commit")].find((r) => r.querySelector(".ws-commit-subject").textContent === "Fix the answer").click()`);
    await page.waitFor(`window.__zuse.view.rightPanel === "changes"`);
    await page.waitFor(`/Commit/.test(document.querySelector("#ws-surface-changes .ws-select")?.textContent ?? "") && [...document.querySelectorAll("#ws-surface-changes .ws-diff-file-name")].map((e) => e.textContent).join() === "app.ts"`, 20000);
  });

  await step("terminal runs a command over the engine PTY", async () => {
    await page.click('button[aria-label="Toggle terminal"]');
    await page.waitFor(`!!document.querySelector("#ws-surface-terminal .xterm-helper-textarea")`, 20000);
    await page.waitFor(`document.querySelectorAll("#ws-surface-terminal .ws-term-tab").length === 1`);
    await new Promise((r) => setTimeout(r, 800));
    await page.eval(`document.querySelector("#ws-surface-terminal .xterm-helper-textarea").focus()`);
    await page.type("echo zuse-$((40+2))");
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`document.querySelector("#ws-surface-terminal .xterm-rows")?.textContent.includes("zuse-42")`, 15000);
    await shot("terminal");
  });

  await step("terminal reattaches and replays after leaving the chat", async () => {
    await page.click('button[aria-label="Hide panel"]');
    await page.click('button[aria-label="Toggle terminal"]');
    await page.waitFor(`document.querySelector("#ws-surface-terminal .xterm-rows")?.textContent.includes("zuse-42")`, 15000);
  });

  await step("browser surface explains previews and refuses the app origin", async () => {
    await addSurface("Browser");
    await page.waitFor(`/Web preview/.test(document.querySelector("#ws-surface-browser")?.textContent ?? "")`);
    await page.eval(`(() => { const input = document.querySelector('#ws-surface-browser input[aria-label="Web address"]'); input.focus(); return true })()`);
    await page.type(location0());
    await page.click('#ws-surface-browser button[aria-label="Open address"]');
    await page.waitFor(`/rather than the Zuse application/.test(document.querySelector("#ws-surface-browser")?.textContent ?? "")`);
    await shot("browser");
  });

  await step("pull requests explain a project without a GitHub remote", async () => {
    await addSurface("PR");
    await page.waitFor(`/No GitHub repository|Couldn't/.test(document.querySelector("#ws-surface-pullRequest")?.textContent ?? "")`, 20000);
    await shot("pull-requests");
  });

  await step("file search finds files on the host", async () => {
    await page.click("#ws-tab-files");
    await page.eval(`document.querySelector('#ws-surface-files button[aria-label="Back to files"]')?.click() ?? true`);
    // Search opens from the explorer header's magnifier, like desktop.
    await page.eval(`document.querySelector('#ws-surface-files input[aria-label="Search files"]') || document.querySelector('#ws-surface-files button[aria-label="Search files"]').click(); true`);
    await page.waitFor(`!!document.querySelector('#ws-surface-files input[aria-label="Search files"]')`);
    await page.eval(`document.querySelector('#ws-surface-files input[aria-label="Search files"]').focus()`);
    await page.type("notes");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-files .ws-search-results .ws-tree-name")].some((e) => e.textContent === "notes.txt")`, 15000);
  });

  await step("reconnect keeps unsaved edits and flags a file changed while offline", async () => {
    await page.click('.ws-search-results .ws-tree-row');
    await page.waitFor(`document.querySelector(".cm-content")?.textContent.includes("untracked")`);
    await page.press(".cm-content .cm-line");
    await page.key("End", "End", 35);
    await page.type(" draft");
    await page.waitFor(`!!document.querySelector(".ws-dirty")`);
    await page.eval(`(() => { window.__offline = true; for (const s of window.__sockets) s.close(); return true })()`);
    await page.waitFor(`window.__zuse.state.connection.kind !== "connected"`, 15000);
    writeFileSync(join(project, "notes.txt"), "changed while offline\n");
    await new Promise((r) => setTimeout(r, 1500));
    await page.eval(`(() => { window.__offline = false; return true })()`);
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await page.waitFor(`/changed on disk/.test(document.querySelector(".ws-editor-pane .ws-banner")?.textContent ?? "")`, 20000);
    const text = await page.eval(`document.querySelector(".cm-content").textContent`);
    if (!text.includes("draft")) throw new Error("unsaved edits were lost on reconnect");
    await shot("reconnect-conflict");
  });

  await step("terminal resumes after the reconnect without replaying twice", async () => {
    await page.click("#ws-tab-terminal");
    await page.eval(`document.querySelector("#ws-surface-terminal .xterm-helper-textarea").focus()`);
    await page.type("echo again-$((1+1))");
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`document.querySelector("#ws-surface-terminal .xterm-rows")?.textContent.includes("again-2")`, 15000);
    const count = await page.eval(`(document.querySelector("#ws-surface-terminal .xterm-rows").textContent.match(/zuse-42/g) ?? []).length`);
    if (count !== 1) throw new Error(`terminal output replayed ${count} times`);
  });

  await step("leaving unsaved edits asks first, then images preview from the host", async () => {
    await page.click("#ws-tab-files");
    await page.eval(`document.querySelector('#ws-surface-files button[aria-label="Back to files"]')?.click() ?? true`);
    await page.waitFor(`/Discard unsaved changes/.test(document.querySelector(".dialog")?.textContent ?? "")`);
    await page.eval(`[...document.querySelectorAll(".dialog button")].find((b) => b.textContent === "Discard").click()`);
    await page.waitFor(`!!document.querySelector('.ws-tree-row[data-path="pixel.png"]') || !!document.querySelector('#ws-surface-files .ws-search-results')`);
    await page.eval(`document.querySelector('#ws-surface-files input[aria-label="Search files"]') || document.querySelector('#ws-surface-files button[aria-label="Search files"]').click(); true`);
    await page.waitFor(`!!document.querySelector('#ws-surface-files input[aria-label="Search files"]')`);
    await page.eval(`(() => { const i = document.querySelector('#ws-surface-files input[aria-label="Search files"]'); i.focus(); i.select(); return true })()`);
    await page.type("pixel");
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-files .ws-search-results .ws-tree-name")].some((e) => e.textContent === "pixel.png")`, 15000);
    await page.eval(`[...document.querySelectorAll("#ws-surface-files .ws-search-results .ws-tree-row")].find((r) => r.textContent.includes("pixel.png")).click()`);
    await page.waitFor(`document.querySelector(".ws-image-preview img")?.naturalWidth === 2`, 15000);
  });

  await step("session card summarizes the environment", async () => {
    // The app-wide uncertain-action notice from the reconnect step sits over the top bar.
    await page.eval(`[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Dismiss")?.click() ?? true`);
    await page.press('button[aria-label="Toggle environment summary"]');
    await page.waitFor(`/Branch changes/.test(document.querySelector(".ws-session-card")?.textContent ?? "")`);
    await shot("session");
    await page.key("Escape", "Escape", 27);
  });

  await step("light theme renders", async () => {
    await page.eval(`(() => { document.documentElement.dataset.theme = "light"; return true })()`);
    await shot("light");
  });

  const errors = page.errors.filter((e) => !/favicon/.test(e));
  if (errors.length) throw new Error(`page errors:\n${errors.join("\n")}`);
  console.log("workspace e2e ok");
} catch (error) {
  await shot("failure").catch(() => {});
  console.error(error);
  console.error(engine.log().slice(-3000));
  process.exitCode = 1;
} finally {
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(project, { recursive: true, force: true });
}

function location0() {
  return new URL(engine.url).origin;
}
