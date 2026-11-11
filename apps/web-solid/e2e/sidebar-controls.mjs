// Sidebar controls the main walk-through leaves out: sort, Show toggles, device grouping, copy,
// section moves from the submenu, Archive all, Pinned collapse, keyboard menus, search to list,
// the project heading's own context menu, and the footer's Git and usage entries.
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const q = (s) => JSON.stringify(s);
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
const dir = mkdtempSync(join(tmpdir(), "zuse-sb-delta-"));
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: dir });
writeFileSync(join(dir, "README.md"), "# delta\n");
execFileSync("git", ["add", "."], { cwd: dir });
execFileSync("git", ["commit", "-qm", "Initial commit"], { cwd: dir, env: gitEnv });

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets });
const page = await openBrowser(engine.url, { width: 1280, height: 820 });
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  try {
    await fn();
  } catch (error) {
    await shot(`fail-${name.slice(0, 24).replace(/\W+/g, "-")}`);
    throw error;
  }
  console.log("ok");
};
const shot = async (name) => shots && (await page.screenshot(join(shots, `ctl-${name}.png`)));
const mark = (expr) => page.eval(`(() => { document.querySelectorAll("[data-sb-test]").forEach((e) => e.removeAttribute("data-sb-test")); const e = ${expr}; if (!e) return false; e.setAttribute("data-sb-test", ""); return true })()`);
/** Press an element found by `find`, retrying while menus are still mounting or settling. */
const pressFound = async (find) => {
  for (let attempt = 0; ; attempt++) {
    await page.waitFor(`!!${find}`);
    await sleep(80);
    try {
      if (!(await mark(find))) throw new Error("gone");
      await page.press("[data-sb-test]");
      return;
    } catch (error) {
      if (attempt >= 3) throw error;
    }
  }
};
const menuItem = (label) => pressFound(`[...document.querySelectorAll('[role^="menuitem"]')].find((e) => e.textContent.trim() === ${q(label)})`);
const openSub = async (label) => {
  const find = `[...document.querySelectorAll('[role="menuitem"][aria-haspopup]')].find((e) => e.textContent.trim() === ${q(label)})`;
  await pressFound(find);
  await page.waitFor(`${find}?.getAttribute("aria-expanded") === "true"`);
};
const viewOptions = async () => {
  await page.waitFor(`!document.querySelector('[role="menu"]')`);
  await page.press(`button[title="View options"]`);
  await page.waitFor(`!!document.querySelector('[role="menu"]')`);
};
const closeMenus = async () => {
  for (let i = 0; i < 3 && (await page.eval(`!!document.querySelector('[role="menu"]')`)); i++) {
    await page.key("Escape", "Escape", 27);
    await sleep(150);
  }
  await page.waitFor(`!document.querySelector('[role="menu"]')`);
};
const contextMenu = async (selector) => {
  await page.waitFor(`!document.querySelector('[role="menu"]')`);
  await page.eval(`(() => { const row = document.querySelector(${q(selector)}); const r = row.getBoundingClientRect(); row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: r.left + 30, clientY: r.top + 10 })); return true })()`);
  // Let the menu settle where it opens before aiming at an item.
  await page.waitFor(`!!document.querySelector('[role="menu"]')`);
  await sleep(150);
};
const rowOrder = (prefix) => page.eval(`[...document.querySelectorAll(".sb-vitem")].sort((a, b) => a.dataset.index - b.dataset.index).map((v) => v.querySelector(".sb-row")?.dataset.chatId).filter((id) => id && id.startsWith(${q(prefix)})).join()`);
/** Enter with its text, so buttons and forms get their default action. */
const enter = async () => {
  for (const type of ["keyDown", "keyUp"]) await page.call("Input.dispatchKeyEvent", { type, key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" });
};
const toast = (text) => page.waitFor(`[...document.querySelectorAll(".toast")].some((t) => t.textContent.includes(${q(text)}))`);

try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" };
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "d-space", deviceId, path: dir, name: "delta", gitDetected: true });
  const hour = 3_600_000;
  // Created order c1 < c2 < c3 (oldest first); activity order c3, c1, c2.
  const seeds = [
    ["c1", "Created first", 30 * hour, 1 * hour, "d-space"],
    ["c2", "Created second", 20 * hour, 10 * hour, "d-space"],
    ["c3", "Created third", 5 * hour, 0.5 * hour, null],
  ];
  for (const [id, title, created, active, space] of seeds) {
    await engine.rpc("Mutate", { op: "createChat", chatId: id, ...(space ? { spaceId: space } : { deviceId }), branch: "main", config });
    await engine.rpc("Mutate", { op: "renameChat", chatId: id, title });
    await engine.rpc("Mutate", { op: "setChatActivity", chatId: id, createdAt: Date.now() - created, lastMessageAt: Date.now() - active });
    await engine.rpc("Mutate", { op: "markChatSeen", chatId: id });
  }
  await page.waitFor(`document.querySelectorAll(".sb-row").length === 3`);

  await step("view options open from the keyboard and move with arrows", async () => {
    await page.eval(`document.querySelector('button[title="View options"]').focus()`);
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`!!document.querySelector('[role="menu"]')`);
    await page.key("ArrowDown", "ArrowDown", 40);
    await page.waitFor(`!!document.activeElement?.closest('[role="menu"]')`);
    const focused = await page.eval(`document.activeElement.textContent.trim()`);
    await shot("view-menu");
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector('[role="menu"]') && document.activeElement?.getAttribute("title") === "View options"`);
    console.log(`(first item ${focused})`);
  });

  await step("Threads view and sort by created or updated", async () => {
    await viewOptions();
    await menuItem("Threads");
    await page.waitFor(`document.querySelector("#sb-heading-label")?.textContent === "Threads"`);
    const updated = await rowOrder("c");
    if (updated !== "c3,c1,c2") throw new Error(`updated order ${updated}`);
    await viewOptions();
    await openSub("Sort");
    await menuItem("Created");
    await page.waitFor(`${q("c3,c2,c1")} === (${`[...document.querySelectorAll(".sb-vitem")].sort((a, b) => a.dataset.index - b.dataset.index).map((v) => v.querySelector(".sb-row")?.dataset.chatId).filter(Boolean).join()`})`);
    await shot("sort-created");
    await viewOptions();
    await openSub("Sort");
    await menuItem("Last updated");
    await page.waitFor(`document.querySelector(".sb-vitem .sb-row")?.dataset.chatId === "c3"`);
  });

  await step("Show toggles hide branch, project icon, harness and the filter button", async () => {
    const loc = () => page.eval(`document.querySelector('[data-chat-id="c1"] .sb-row-loc')?.textContent`);
    if ((await loc()) !== "delta · main") throw new Error(`location ${await loc()}`);
    await viewOptions();
    await openSub("Show");
    await sleep(250);
    await menuItem("Branch");
    await page.waitFor(`document.querySelector('[data-chat-id="c1"] .sb-row-loc')?.textContent === "delta"`);
    // The menu stays open for toggles.
    await sleep(250);
    await menuItem("Project icon");
    await page.waitFor(`!document.querySelector('[data-chat-id="c1"] .sb-mono') && !!document.querySelector('[data-chat-id="c1"] .sb-avatar svg')`);
    // Clicks closer together than a person manages can close Kobalte's submenu under CDP.
    await sleep(250);
    await menuItem("Harness");
    await page.waitFor(`!document.querySelector('[data-chat-id="c1"] .sb-avatar')`).catch(async (e) => {
      console.log(await page.eval(`JSON.stringify({ prefs: Object.fromEntries(Object.entries(window.__zuse.view.preferences).filter(([k]) => k.startsWith("sidebar"))), sub: [...document.querySelectorAll('[role="menu"]')].length, avatar: document.querySelector('[data-chat-id="c1"] .sb-avatar')?.outerHTML.slice(0, 200) })`));
      throw e;
    });
    await sleep(250);
    await menuItem("Project filter");
    await page.waitFor(`!document.querySelector('button[aria-label="Filter by project"]')`);
    await shot("show-off");
    for (const label of ["Branch", "Project icon", "Harness", "Project filter"]) {
      await menuItem(label);
      await sleep(250);
    }
    await page.waitFor(`!!document.querySelector('button[aria-label="Filter by project"]') && !!document.querySelector('[data-chat-id="c1"] .sb-mono')`);
    await closeMenus();
  });

  await step("Group by device shows one heading per device", async () => {
    await viewOptions();
    await menuItem("Group by device");
    await page.waitFor(`document.querySelector("#sb-heading-label")?.textContent === "Devices"`);
    const heads = await page.eval(`[...document.querySelectorAll(".sb-head .sb-head-label")].map((e) => e.textContent)`);
    await shot("devices");
    await menuItem("Group by device");
    await closeMenus();
    await page.waitFor(`document.querySelector("#sb-heading-label")?.textContent === "Threads"`);
    console.log(`(${heads.join(", ")})`);
    if (heads.length !== 1) throw new Error(`device heads ${heads}`);
  });

  await step("Projects view without Compact shows two-line pinned rows", async () => {
    await viewOptions();
    await menuItem("Projects");
    await contextMenu(`[data-chat-id="c2"]`);
    await menuItem("Pin");
    await page.waitFor(`[...document.querySelectorAll(".sb-head-label")].some((e) => e.textContent === "Pinned")`);
    await page.waitFor(`document.querySelector('[data-chat-id="c2"]')?.dataset.twoLine !== "true"`);
    await viewOptions();
    await menuItem("Compact");
    await page.waitFor(`document.querySelector('[data-chat-id="c2"]')?.dataset.twoLine === "true"`);
    await closeMenus();
    await shot("not-compact");
    await viewOptions();
    await menuItem("Compact");
    await closeMenus();
    await page.waitFor(`document.querySelector('[data-chat-id="c2"]')?.dataset.twoLine !== "true"`);
  });

  await step("Pinned collapses and expands", async () => {
    const toggle = `[...document.querySelectorAll(".sb-head-toggle")].find((b) => b.textContent.includes("Pinned"))`;
    await page.eval(`${toggle}.click()`);
    await page.waitFor(`!document.querySelector('[data-chat-id="c2"]') && ${toggle}.getAttribute("aria-expanded") === "false"`);
    await page.eval(`${toggle}.click()`);
    await page.waitFor(`!!document.querySelector('[data-chat-id="c2"]')`);
  });

  await step("Move to section › New section… creates it with the chat inside", async () => {
    await contextMenu(`[data-chat-id="c1"]`);
    await openSub("Move to section");
    await menuItem("New section…");
    await page.waitFor(`document.querySelector(".dialog-title")?.textContent === "New section"`);
    await page.eval(`document.querySelector(".dialog input").focus()`);
    await page.type("Focus");
    // A real Enter (with its text) so the form submits implicitly.
    for (const type of ["keyDown", "keyUp"]) await page.call("Input.dispatchKeyEvent", { type, key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" });
    const inFocus = `(() => { const items = [...document.querySelectorAll(".sb-vitem")].sort((a, b) => a.dataset.index - b.dataset.index); const at = items.findIndex((v) => v.querySelector(".sb-head-label")?.textContent === "Focus"); return at >= 0 && items[at + 1]?.querySelector(".sb-row")?.dataset.chatId === "c1" })()`;
    await page.waitFor(inFocus);
    await sleep(600);
    if (!(await page.eval(inFocus))) throw new Error("new section lost its chat");
    await shot("new-section");
  });

  await step("Remove from section returns the chat to its project", async () => {
    await contextMenu(`[data-chat-id="c1"]`);
    await openSub("Move to section");
    await menuItem("Remove from section");
    await page.waitFor(`!!document.querySelector(".sb-note") && document.querySelector(".sb-note").textContent === "Drop sessions here"`);
  });

  await step("Archive all archives a section's chats", async () => {
    await contextMenu(`[data-chat-id="c3"]`);
    await openSub("Move to section");
    await menuItem("Focus");
    await page.waitFor(`!document.querySelector(".sb-note")`);
    await page.waitFor(`!document.querySelector('[role="menu"]')`);
    await mark(`[...document.querySelectorAll(".sb-head")].find((h) => h.textContent.includes("Focus")).querySelector('button[title="Section options"]')`);
    await page.press("[data-sb-test]");
    await menuItem("Archive all");
    await page.waitFor(`window.__zuse.state.chats.c3.archived === true`);
    await engine.rpc("Mutate", { op: "setChatArchived", chatId: "c3", archived: false });
    await page.waitFor(`!!document.querySelector('[data-chat-id="c3"]')`);
  });

  await step("Copy › Chat ID copies and confirms", async () => {
    await contextMenu(`[data-chat-id="c1"]`);
    await openSub("Copy");
    await menuItem("Chat ID");
    await toast("Chat ID copied");
    await contextMenu(`[data-chat-id="c1"]`);
    await openSub("Copy");
    await menuItem("Path");
    await toast("Path copied");
  });

  await step("right-clicking a project heading opens its menu", async () => {
    await contextMenu(`.sb-head[data-project="true"]`);
    await page.waitFor(`[...document.querySelectorAll('[role="menuitem"]')].some((e) => e.textContent.trim() === "Remove…")`);
    await shot("project-context");
    await menuItem("Usage");
    await page.waitFor(`window.__zuse.view.route.kind === "usage"`);
    await page.waitFor(`!document.querySelector('[role="menu"]')`, 3000).catch(async (e) => {
      console.log(await page.eval(`[...document.querySelectorAll('[role="menu"]')].map((m) => m.getAttribute("aria-label") + " " + m.textContent.slice(0, 60)).join(" | ")`));
      throw e;
    });
  });

  await step("search: Down moves into results and Enter opens one", async () => {
    // The Usage page takes focus once its data loads, so leave it first.
    await page.click(`[data-chat-id="c1"] .sb-row-open`);
    await page.waitFor(`window.__zuse.view.route.kind === "chat"`);
    await sleep(500);
    await page.click(`button[aria-label="Search threads"]`);
    await page.waitFor(`document.activeElement?.getAttribute("aria-label") === "Search threads"`);
    await page.type("second");
    await page.waitFor(`document.querySelectorAll(".sb-row").length === 1`);
    await page.key("ArrowDown", "ArrowDown", 40);
    await page.waitFor(`document.activeElement?.closest(".sb-row")?.dataset.chatId === "c2"`);
    await enter();
    await page.waitFor(`window.__zuse.view.selectedChatId === "c2" && window.__zuse.view.route.kind === "chat"`);
    await shot("search-open");
    await page.click(`button[aria-label="Close search"]`);
  });

  await step("project filter works from the keyboard", async () => {
    await viewOptions();
    await menuItem("Threads");
    // The closing menu hands focus back to its trigger; wait for that before moving it.
    await page.waitFor(`!document.querySelector('[role="menu"]') && document.activeElement?.getAttribute("title") === "View options"`);
    await page.eval(`document.querySelector('button[aria-label="Filter by project"]').focus()`);
    await enter();
    await page.waitFor(`document.activeElement?.getAttribute("aria-label") === "Search projects"`).catch(async (e) => {
      console.log(await page.eval(`JSON.stringify({ menu: !!document.querySelector(".sb-filter-menu"), active: document.activeElement.tagName + "." + document.activeElement.className + "/" + document.activeElement.getAttribute("aria-label") })`));
      throw e;
    });
    await page.type("del");
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`!!document.querySelector(".sb-chip") && document.querySelectorAll(".sb-row").length === 2`);
    await shot("filter-chip");
    await page.click(`button[aria-label="Clear project filter"]`);
    await page.waitFor(`document.querySelectorAll(".sb-row").length === 3`);
  });

  await step("footer: Git opens the right panel, usage reaches the full page", async () => {
    await page.click(`.sb-foot button[aria-label="Git"]`);
    await page.waitFor(`window.__zuse.view.rightPanel === "git"`);
    await shot("git-open");
    await page.click(`.sb-foot button[aria-label="Git"]`);
    await page.waitFor(`!window.__zuse.view.rightPanel`);
    await page.waitFor(`!document.querySelector('[role="menu"]')`);
    await page.press(`button[title="Usage limits"]`);
    await menuItem("Refresh usage");
    await menuItem("Full usage");
    await page.waitFor(`window.__zuse.view.route.kind === "usage"`);
  });

  await step("nav: New chat clears the selection, New project opens its page", async () => {
    await page.click(`[data-chat-id="c1"] .sb-row-open`);
    await page.waitFor(`window.__zuse.view.selectedChatId === "c1"`);
    await page.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("New chat")).click()`);
    await page.waitFor(`window.__zuse.view.selectedChatId === null`);
    await page.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("New project")).click()`);
    await page.waitFor(`window.__zuse.view.route.kind === "newProject"`);
    const current = await page.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("New project")).getAttribute("aria-current")`);
    if (current !== "page") throw new Error("New project not marked current");
  });

  const errors = page.errors.filter((e) => !/github\.com/.test(e));
  if (errors.length) throw new Error(`page errors:\n${errors.join("\n")}`);
  console.log("sidebar controls ok");
} finally {
  await page.close();
  await engine.close();
  rmSync(dir, { recursive: true, force: true });
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
}
