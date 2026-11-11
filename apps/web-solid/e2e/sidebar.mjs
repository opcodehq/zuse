// Sidebar end to end against a disposable engine: grouping, collapse, views, rename, pins, sections,
// drag, archive (restored from Settings), search, delete, footer and keyboard. Projects live under the system temp directory.
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
const projects = ["alpha-service", "beta"].map((name) => {
  const dir = mkdtempSync(join(tmpdir(), `zuse-sb-${name}-`));
  execFileSync("git", ["init", "-q", "-b", "main"], { cwd: dir });
  writeFileSync(join(dir, "README.md"), `# ${name}\n`);
  execFileSync("git", ["add", "."], { cwd: dir });
  execFileSync("git", ["commit", "-qm", "Initial commit"], { cwd: dir, env: gitEnv });
  return { name, dir };
});
// Forks create worktrees; keep them out of the home directory.
const worktrees = mkdtempSync(join(tmpdir(), "zuse-sb-worktrees-"));
process.env.ZUSE_WORKTREES_DIR = worktrees;

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets });
const page = await openBrowser(engine.url, { width: 1280, height: 820 });
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  await fn();
  console.log("ok");
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `${name}.png`)));
const q = (s) => JSON.stringify(s);
const text = (selector) => page.eval(`[...document.querySelectorAll(${q(selector)})].map((e) => e.textContent.trim())`);
const menuItem = async (label) => {
  await page.waitFor(`[...document.querySelectorAll('[role^="menuitem"]')].some((e) => e.textContent.trim() === ${q(label)})`);
  await page.eval(`(() => { document.querySelectorAll("[data-sb-test]").forEach((e) => e.removeAttribute("data-sb-test")); [...document.querySelectorAll('[role^="menuitem"]')].find((e) => e.textContent.trim() === ${q(label)}).setAttribute("data-sb-test", ""); return true })()`);
  await page.press("[data-sb-test]");
};
const contextMenu = (chatId) =>
  page.eval(`(() => { const row = document.querySelector('[data-chat-id=${q(chatId)}]'); const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left + 30, clientY: r.top + 10 })); return true })()`);

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" };
  for (const [index, project] of projects.entries())
    await engine.rpc("Mutate", { op: "createSpace", spaceId: `sb-space-${index}`, deviceId, path: project.dir, name: project.name, gitDetected: true });
  const chats = [
    ["sb-a1", 0, "Refactor the login flow"],
    ["sb-a2", 0, "Write release notes"],
    ["sb-a3", 0, "Investigate flaky test"],
    ["sb-b1", 1, "Design the onboarding"],
    ["sb-b2", 1, "Fix dark mode contrast"],
  ];
  for (const [id, space, title] of chats) {
    await engine.rpc("Mutate", { op: "createChat", chatId: id, spaceId: `sb-space-${space}`, branch: "main", config });
    await engine.rpc("Mutate", { op: "renameChat", chatId: id, title });
  }

  await step("projects view groups rows under avatar headings", async () => {
    await page.waitFor(`document.querySelectorAll(".sb-row").length === 5`);
    const heads = await text(`.sb-head[data-project="true"] .sb-head-label`);
    if (!heads.includes("alpha-service") || !heads.includes("beta")) throw new Error(`headings: ${heads}`);
    const mono = await text(".sb-head .sb-mono");
    if (!mono.includes("AS")) throw new Error(`monograms: ${mono}`);
    await shot("projects-dark");
  });

  await step("collapsing a project hides its rows and shows a count", async () => {
    await page.click(`.sb-head[data-project="true"] .sb-head-toggle`);
    await page.waitFor(`document.querySelectorAll(".sb-row").length < 5 && !!document.querySelector(".sb-head .sb-count")`);
    await page.click(`.sb-head[data-project="true"] .sb-head-toggle`);
    await page.waitFor(`document.querySelectorAll(".sb-row").length === 5`);
  });

  await step("selecting a row opens the chat and marks it current", async () => {
    await page.click(`[data-chat-id="sb-a2"] .sb-row-open`);
    await page.waitFor(`window.__zuse.view.selectedChatId === "sb-a2" && document.querySelector('[data-chat-id="sb-a2"]').dataset.selected === "true"`);
  });

  await step("double click renames in place", async () => {
    await page.eval(`document.querySelector('[data-chat-id="sb-b2"] .sb-row-open').dispatchEvent(new MouseEvent("dblclick", { bubbles: true }))`);
    await page.waitFor(`document.activeElement?.classList.contains("sb-rename")`);
    await page.eval(`(() => { const i = document.activeElement; i.value = "Fix contrast in light and dark"; return true })()`);
    await page.key("Enter");
    await page.waitFor(`window.__zuse.state.chats["sb-b2"].title === "Fix contrast in light and dark"`);
  });

  await step("context menu pins a chat into Pinned", async () => {
    await contextMenu("sb-b1");
    await shot("context-menu");
    await menuItem("Pin");
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Pinned")`);
    await page.waitFor(`document.querySelector(".sb-vitem [data-chat-id='sb-b1']")?.closest(".sb-vitem").dataset.index === "1"`);
  });

  await step("dragging a row onto Pinned pins it ahead", async () => {
    // Drop zones appear a frame after dragstart, as in a real drag.
    await page.eval(`new Promise((resolve) => {
      const row = document.querySelector('[data-chat-id="sb-a3"]');
      const dt = new DataTransfer();
      row.dispatchEvent(new DragEvent("dragstart", { bubbles: true, dataTransfer: dt }));
      requestAnimationFrame(() => requestAnimationFrame(() => {
      const head = [...document.querySelectorAll(".sb-head")].find((h) => h.textContent.includes("Pinned"));
      const r = head.getBoundingClientRect();
      head.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + 10, clientY: r.top + 10 }));
      head.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + 10, clientY: r.top + 10 }));
      row.dispatchEvent(new DragEvent("dragend", { bubbles: true, dataTransfer: dt }));
      resolve(true);
      }));
    })`);
    await page.waitFor(`document.querySelector(".sb-vitem [data-chat-id='sb-a3']")?.closest(".sb-vitem").dataset.index === "1"`);
  });

  await step("view options create a section and the menu moves a chat into it", async () => {
    await page.press(`button[aria-label="View options"], button[title="View options"]`);
    await menuItem("Create section");
    await page.waitFor(`!!document.querySelector(".dialog input")`);
    await page.eval(`document.querySelector(".dialog input").focus()`);
    await page.type("Later");
    await page.click(`.dialog button[type="submit"]`);
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Later")`);
    await contextMenu("sb-a1");
    await page.waitFor(`[...document.querySelectorAll('[role="menuitem"]')].some((e) => e.textContent.includes("Move to section"))`);
    await page.eval(`(() => { const t = [...document.querySelectorAll('[role="menuitem"]')].find((e) => e.textContent.includes("Move to section")); t.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, pointerType: "mouse" })); t.click(); return true })()`);
    await menuItem("Later");
    const inSection = `(() => { const head = [...document.querySelectorAll(".sb-vitem")].find((v) => v.querySelector(".sb-head-label")?.textContent === "Later"); const row = document.querySelector("[data-chat-id='sb-a1']")?.closest(".sb-vitem"); return !!head && !!row && Number(row.dataset.index) === Number(head.dataset.index) + 1 && !document.body.textContent.includes("Drop sessions here") })()`;
    await page.waitFor(inSection);
    await new Promise((r) => setTimeout(r, 800));
    if (!(await page.eval(inSection))) throw new Error("section membership did not survive the engine's reply");
    await shot("sections");
    const later = `[...document.querySelectorAll(".sb-head")].find((h) => h.textContent.includes("Later"))`;
    await page.eval(`${later}.querySelector(".sb-head-toggle").click()`);
    await page.waitFor(`!document.querySelector("[data-chat-id='sb-a1']") && ${later}.querySelector(".sb-count")?.textContent === "1"`);
    await page.eval(`${later}.querySelector(".sb-head-toggle").click()`);
    await page.waitFor(`!!document.querySelector("[data-chat-id='sb-a1']")`);
  });

  await step("threads view lists one flat list with two-line rows", async () => {
    await page.press(`button[title="View options"]`);
    await menuItem("Threads");
    await page.waitFor(`document.querySelector("#sb-heading-label")?.textContent === "Threads" && !!document.querySelector('.sb-row[data-two-line="true"] .sb-row-meta')`);
    const meta = await text(`[data-chat-id="sb-a2"] .sb-row-loc`);
    if (!meta[0]?.includes("alpha-service")) throw new Error(`location: ${meta}`);
    await shot("threads-dark");
  });

  await step("archive from the row, then restore it from Settings", async () => {
    await page.click(`[data-chat-id="sb-a2"] .sb-row-archive`);
    await page.waitFor(`!document.querySelector('[data-chat-id="sb-a2"]')`);
    await page.waitFor(`window.__zuse.state.chats["sb-a2"].archived === true`);
    // Archived chats live in Settings only; the list heading has no archive view.
    if (await page.eval(`!!document.querySelector('.sb-heading [aria-label*="rchive"]')`)) throw new Error("the heading still offers an archive view");
    await page.click(`button[aria-label="Settings"]`);
    await page.waitFor(`!!document.querySelector("#settings-tab-archived")`);
    await page.click("#settings-tab-archived");
    await page.waitFor(`!!document.querySelector('button[aria-label="Restore chat Write release notes"]')`);
    await shot("archive");
    await page.click(`button[aria-label="Restore chat Write release notes"]`);
    await page.waitFor(`window.__zuse.state.chats["sb-a2"].archived === false && window.__zuse.view.route.kind === "chat" && window.__zuse.view.selectedChatId === "sb-a2"`);
    await page.waitFor(`!!document.querySelector('[data-chat-id="sb-a2"]')`);
  });

  await step("search ranks matching titles and projects", async () => {
    await page.click(`button[aria-label="Search threads"]`);
    await page.waitFor(`document.activeElement?.getAttribute("aria-label") === "Search threads"`);
    await page.type("onboard");
    await page.waitFor(`document.querySelectorAll(".sb-row").length === 1 && !!document.querySelector('[data-chat-id="sb-b1"]')`);
    await page.eval(`(() => { const i = document.activeElement; i.value = ""; i.dispatchEvent(new InputEvent("input", { bubbles: true })); return true })()`);
    await page.type("zzzz");
    await page.waitFor(`document.querySelector(".sb-note")?.textContent === "No matching threads"`);
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".sb-search") && document.querySelectorAll(".sb-row").length === 5`);
  });

  await step("arrow keys move focus between rows and Shift+F10 opens the row menu", async () => {
    await page.eval(`document.querySelector('.sb-vitem[data-index="1"] .sb-focus').focus()`);
    await page.key("ArrowDown", "ArrowDown", 40);
    await page.waitFor(`document.activeElement?.closest(".sb-vitem")?.dataset.index === "2"`);
    await page.key("F10", "F10", 121, 8);
    await page.waitFor(`[...document.querySelectorAll('[role="menuitem"]')].some((e) => e.textContent.trim() === "Rename")`);
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector('[role="menuitem"]')`);
  });

  await step("a finished reply marks the row done until it is opened", async () => {
    await page.click(`[data-chat-id="sb-b1"] .sb-row-open`);
    await engine.rpc("QueueMessage", { chatId: "sb-a3", targetDeviceId: deviceId, text: "Say hello", attachments: [], holdForTurnEnd: true });
    await page.waitFor(`document.querySelector('[data-chat-id="sb-a3"]')?.dataset.unread === "true"`, 30000).catch(async (error) => {
      console.log(await page.eval(`JSON.stringify({ chat: window.__zuse.state.chats["sb-a3"], session: window.__zuse.state.sessions["sb-a3"], selected: window.__zuse.view.selectedChatId })`));
      throw error;
    });
    await page.waitFor(`document.querySelector('[data-chat-id="sb-a3"] .sb-status')?.getAttribute("aria-label") !== "Working"`);
    await shot("done-unseen");
    await page.click(`[data-chat-id="sb-a3"] .sb-row-open`);
    await page.waitFor(`!document.querySelector('[data-chat-id="sb-a3"]')?.dataset.unread && !!window.__zuse.state.chats["sb-a3"].lastSeenAt`);
  });

  // The fixture agent records no assistant reply on current engines, so the engine may refuse the
  // fork ("Wait for a completed response"). Either the copy opens or the refusal is shown.
  await step("fork to new thread opens the copy or shows why it can't", async () => {
    const before = await page.eval(`Object.keys(window.__zuse.state.chats).length`);
    await contextMenu("sb-a3");
    await menuItem("Fork to new thread");
    await page.waitFor(
      `Object.keys(window.__zuse.state.chats).length === ${before + 1} || [...document.querySelectorAll(".toast")].some((t) => t.textContent.includes("Couldn't fork"))`,
      30000,
    );
    const forked = await page.eval(`Object.keys(window.__zuse.state.chats).length === ${before + 1}`);
    if (forked) {
      await page.waitFor(`window.__zuse.view.selectedChatId !== "sb-a3" && !window.__zuse.state.chats[window.__zuse.view.selectedChatId]?.parentChatId`);
      process.stdout.write("(forked) ");
    } else process.stdout.write(`(refused: ${await page.eval(`[...document.querySelectorAll(".toast")].map((t) => t.textContent.trim()).join(" | ")`)}) `);
  });

  await step("delete asks first, then removes the chat", async () => {
    await contextMenu("sb-b2");
    await menuItem("Delete…");
    await page.waitFor(`document.querySelector(".dialog-title")?.textContent === "Delete session?"`);
    await shot("delete-dialog");
    await page.eval(`[...document.querySelectorAll(".dialog button")].find((b) => b.textContent === "Delete").click()`);
    await page.waitFor(`!window.__zuse.state.chats["sb-b2"] && !document.querySelector('[data-chat-id="sb-b2"]')`);
  });

  await step("project filter narrows the Threads list", async () => {
    const all = await page.eval(`document.querySelectorAll(".sb-row").length`);
    await page.press(`button[aria-label="Filter by project"]`);
    await page.waitFor(`!!document.querySelector(".sb-filter-menu")`);
    await page.eval(`[...document.querySelectorAll(".sb-filter-menu [role=option]")].find((e) => e.textContent.includes("beta")).click()`);
    await page.waitFor(`!!document.querySelector(".sb-chip") && document.querySelectorAll(".sb-row").length === 1`);
    await page.click(`button[aria-label="Clear project filter"]`);
    await page.waitFor(`document.querySelectorAll(".sb-row").length === ${all}`);
  });

  await step("footer shows agents, opens usage and toggles settings", async () => {
    const agents = await page.eval(`document.querySelector(".sb-agents")?.textContent.trim()`);
    if (agents !== "0") throw new Error(`agents: ${agents}`);
    await page.press(`button[title="Usage limits"]`);
    await page.waitFor(`[...document.querySelectorAll(".menu .menu-label")].some((e) => e.textContent === "Usage limits")`);
    await shot("usage-menu");
    await page.key("Escape", "Escape", 27);
    await page.click(`.sb-foot button[aria-label="Settings"]`);
    await page.waitFor(`window.__zuse.view.route.kind === "settings"`);
    await page.click(`.sb-foot button[aria-label="Close settings"]`);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`);
  });

  await step("light theme", async () => {
    await page.eval(`document.documentElement.dataset.theme = "light"; true`);
    await page.press(`button[title="View options"]`);
    await menuItem("Projects");
    await page.waitFor(`document.querySelector("#sb-heading-label")?.textContent === "Projects"`);
    await shot("projects-light");
    await page.eval(`document.documentElement.dataset.theme = "dark"; true`);
  });

  const head = (name) => `[...document.querySelectorAll('.sb-head[data-project="true"]')].find((h) => h.querySelector(".sb-head-label")?.textContent === ${q(name)})`;
  const pressIn = async (expr) => {
    // A menu still closing would take this press as an outside click.
    await page.waitFor(`!document.querySelector('[role="menu"]')`);
    await new Promise((r) => setTimeout(r, 150));
    await page.eval(`(() => { document.querySelectorAll("[data-sb-test]").forEach((e) => e.removeAttribute("data-sb-test")); ${expr}.setAttribute("data-sb-test", ""); return true })()`);
    await page.press("[data-sb-test]");
  };

  await step("project settings rename the project", async () => {
    await pressIn(`${head("alpha-service")}.querySelector('button[title="Project settings"]')`);
    await shot("project-menu");
    await menuItem("Rename…");
    await page.waitFor(`document.querySelector(".dialog-title")?.textContent === "Rename project"`);
    await page.eval(`(() => { const i = document.querySelector(".dialog input"); i.focus(); i.select(); return true })()`);
    await page.type("alpha");
    await page.click(`.dialog button[type="submit"]`);
    await page.waitFor(`window.__zuse.state.spaces["sb-space-0"].name === "alpha" && !!(${head("alpha")})`);
  });

  await step("new chat in project starts a draft there", async () => {
    await page.eval(`${head("alpha")}.querySelector('button[title="New chat in project"]').click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === null && window.__zuse.view.draftProjectPath === ${q(projects[0].dir)}`);
  });

  await step("archived chats opens the project archive", async () => {
    await pressIn(`${head("alpha")}.querySelector('button[title="Project settings"]')`);
    await menuItem("Archived chats");
    await page.waitFor(`window.__zuse.view.route.kind === "archive" && !!window.__zuse.view.route.projectKey`);
    await page.click(`[data-chat-id="sb-a1"] .sb-row-open`);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`);
  });

  await step("section options rename and delete a section", async () => {
    const section = `[...document.querySelectorAll(".sb-head")].find((h) => h.querySelector(".sb-head-label")?.textContent === "Later")`;
    await pressIn(`${section}.querySelector('button[title="Section options"]')`);
    await menuItem("Edit section");
    await page.waitFor(`document.querySelector(".dialog-title")?.textContent === "Edit section"`);
    await page.eval(`(() => { const i = document.querySelector(".dialog input"); i.focus(); i.select(); return true })()`);
    await page.type("Someday");
    await page.click(`.dialog button[type="submit"]`);
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Someday")`);
    const someday = `[...document.querySelectorAll(".sb-head")].find((h) => h.querySelector(".sb-head-label")?.textContent === "Someday")`;
    await pressIn(`${someday}.querySelector('button[title="Section options"]')`);
    await menuItem("Delete");
    await page.waitFor(`![...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Someday") && !!document.querySelector('[data-chat-id="sb-a1"]')`);
  });

  await step("remove project asks first", async () => {
    await contextMenu("sb-b1");
    await menuItem("Unpin");
    await page.waitFor(`!!(${head("beta")})`);
    await pressIn(`${head("beta")}.querySelector('button[title="Project settings"]')`);
    await menuItem("Remove…");
    await page.waitFor(`document.querySelector(".dialog-title")?.textContent === "Remove project?"`);
    const body = await page.eval(`document.querySelector(".dialog-description").textContent`);
    if (!/permanently deletes its 1 session/.test(body)) throw new Error(body);
    await page.eval(`[...document.querySelectorAll(".dialog button")].find((b) => b.textContent === "Remove").click()`);
    await page.waitFor(`!window.__zuse.state.spaces["sb-space-1"] && !(${head("beta")})`);
  });

  await step("hide sidebar", async () => {
    await page.click(`button[aria-label="Hide projects panel"]`);
    await page.waitFor(`!document.querySelector(".sb")`);
    // No on-screen control reopens it yet (that belongs to the top bar); the shortcut does.
    await page.eval(`document.activeElement?.blur(); true`);
    await page.key("b", "KeyB", 66, 2);
    await page.waitFor(`!!document.querySelector(".sb")`);
  });

  const errors = page.errors.filter((e) => !/github\.com/.test(e));
  if (errors.length) throw new Error(`page errors:\n${errors.join("\n")}`);
  console.log("sidebar ok");
} finally {
  await page.close();
  await engine.close();
  for (const project of projects) rmSync(project.dir, { recursive: true, force: true });
  rmSync(worktrees, { recursive: true, force: true });
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
}
