// Zapper Files in the browser: tree, keyboard, dialogs, conflicts and errors, from the real
// components over an in-memory store. ZAPPER_FILES_SHOTS=<dir> also saves screenshots.
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { openBrowser } from "./harness.mjs";
const app = fileURLToPath(new URL("..", import.meta.url));
const shots = process.env.ZAPPER_FILES_SHOTS;
if (shots) mkdirSync(shots, { recursive: true });
const server = await createServer({ root: app, configFile: app + "/vite.config.ts", server: { host: "127.0.0.1", port: 0, open: false, hmr: { overlay: false } } });
await server.listen();
const url = "http://127.0.0.1:" + server.httpServer.address().port + "/e2e/zapper-files/fixture.html";
const q = JSON.stringify;
const row = (path) => '.zp-tree-row[data-path="' + path + '"]';
const rows = (page) => page.eval("[...document.querySelectorAll('.zp-tree-row')].map(r=>'  '.repeat(Number(r.getAttribute('aria-level'))-1)+r.querySelector('.zp-tree-name').textContent)");
const shot = async (page, name) => { if (shots) await page.screenshot(join(shots, name + ".png")); };
const fill = (page, selector, value) => page.eval("(()=>{const e=document.querySelector(" + q(selector) + ");e.value=" + q(value) + ";e.dispatchEvent(new Event('input',{bubbles:true}));})()");
const menu = async (page, trigger, item) => {
  await page.press(trigger);
  await page.waitFor("[...document.querySelectorAll('[role=menuitem]')].some(e=>e.textContent.trim()===" + q(item) + ")");
  await page.eval("[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.trim()===" + q(item) + ").setAttribute('data-files-item','')");
  await page.press("[data-files-item]");
};
const submit = (page) => page.eval("document.querySelector('[role=dialog] form').requestSubmit()");
let page;
try {
  for (const [theme, width] of [["dark", 1440], ["light", 1440], ["dark", 760]]) {
    page = await openBrowser(url, { width, height: 900 });
    await page.waitFor("!!window.files");
    await page.eval("files.open()");
    await page.eval("files.theme(" + q(theme) + ")");
    await page.waitFor(`!!document.querySelector('.zp-tabs [aria-label="Drive"]')`);
    await page.click('.zp-tabs [aria-label="Drive"]');
    await page.waitFor("!!document.querySelector(" + q(row("handoffs/conversational-pr-research.md")) + ")");
    await page.click(row("handoffs/design-references"));
    await page.click(row("NOTES.md"));
    await page.waitFor("document.querySelector('.zp-page-surface:not([hidden])')?.textContent.includes('Design review moved')");
    await page.click('.zp-tabs [aria-label="Drive"]');
    await page.waitFor("document.querySelector(" + q(row("NOTES.md")) + ")?.getAttribute('aria-selected')==='true'");
    await shot(page, "tree-nested-" + theme + (width < 1000 ? "-narrow" : ""));
    if (theme !== "dark" || width < 1000) { await page.close(); page = null; continue; }

    assert.deepEqual(await rows(page), [
      "handoffs", "  design-references", "    composer.md", "    sidebar.md", "  conversational-pr-research.md",
      "reports", "  2026-11-26.md", "AGENTS.md", "DIGEST.md", "INSTRUCTIONS.md", "NOTES.md", "WORKERS.md",
    ], "folders first, nested, deleted files hidden");

    // Keyboard: up from NOTES.md selects INSTRUCTIONS.md, left on a nested file selects its folder.
    await page.eval("document.querySelector(" + q(row("NOTES.md")) + ").focus()");
    await page.key("ArrowUp", "ArrowUp", 38);
    await page.waitFor("document.activeElement?.dataset.path==='INSTRUCTIONS.md'");
    await page.eval("document.querySelector(" + q(row("handoffs/design-references/sidebar.md")) + ").click()");
    await page.click('.zp-tabs [aria-label="Drive"]');
    await page.eval("document.querySelector(" + q(row("handoffs/design-references/sidebar.md")) + ").focus()");
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await page.waitFor("document.activeElement?.dataset.path==='handoffs/design-references'");
    await page.key("ArrowLeft", "ArrowLeft", 37);
    await page.waitFor("!document.querySelector(" + q(row("handoffs/design-references/sidebar.md")) + ")");
    await page.key("ArrowRight", "ArrowRight", 39);
    await page.waitFor("!!document.querySelector(" + q(row("handoffs/design-references/sidebar.md")) + ")");

    // Reserved files can't be moved or deleted.
    await page.press(row("AGENTS.md") + " .zp-tree-more-button");
    await page.waitFor("[...document.querySelectorAll('[role=menuitem]')].some(e=>e.textContent.includes('fixed name'))");
    assert.equal(await page.eval("[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.includes('always kept')).hasAttribute('data-disabled')"), true);
    await page.key("Escape", "Escape", 27);

    // Folder delete names its whole scope, and the request carries it.
    await menu(page, row("handoffs") + " .zp-tree-more-button", "Delete folder…");
    await page.waitFor("document.querySelector('[role=dialog]')?.textContent.includes('3 files and 1 folder inside handoffs')");
    await shot(page, "delete-folder-confirm");
    await page.eval("[...document.querySelectorAll('[role=dialog] button')].find(b=>b.textContent.trim()==='Delete 3 files').click()");
    await page.waitFor("!document.querySelector('[role=dialog]') && !document.querySelector(" + q(row("handoffs")) + ")");
    const sent = (await page.eval("files.requests()")).filter((r) => r.method === "ZotCommand").at(-1).params.action;
    assert.deepEqual(sent, { type: "deleteFolder", path: "handoffs", files: [
      { path: "handoffs/conversational-pr-research.md", version: 1 },
      { path: "handoffs/design-references/composer.md", version: 2 },
      { path: "handoffs/design-references/sidebar.md", version: 1 },
    ] });

    // A bad path is caught in the dialog and the draft stays.
    await page.eval("document.querySelector('[aria-label=\"New file\"]').click()");
    await page.waitFor("!!document.querySelector('[role=dialog] input[required]')");
    await fill(page, "[role=dialog] input[required]", "reports/../escape.md");
    await fill(page, "[role=dialog] textarea", "Keep this draft");
    await submit(page);
    await page.waitFor("document.querySelector('[role=dialog] [role=alert]')?.textContent.includes('. or ..')");
    // An engine refusal also keeps it.
    await page.eval("files.fail('a folder named reports already exists')");
    await fill(page, "[role=dialog] input[required]", "reports");
    await submit(page);
    await page.waitFor("document.querySelector('[role=dialog] [role=alert]')?.textContent.includes('a folder named reports already exists')");
    assert.equal(await page.eval("document.querySelector('[role=dialog] textarea').value"), "Keep this draft");
    await shot(page, "error-new-file");
    await page.eval("files.fail(null)");
    await fill(page, "[role=dialog] input[required]", "reports/2026-11-27.md");
    await submit(page);
    await page.waitFor("!document.querySelector('[role=dialog]') && document.querySelector(" + q(row("reports/2026-11-27.md")) + ")?.getAttribute('aria-selected')==='true'");

    // A conflicting save keeps the draft and shows what changed.
    await page.click(row("WORKERS.md"));
    await page.waitFor("[...document.querySelectorAll('.zp-page-surface:not([hidden]) button')].some(b=>b.textContent.trim()==='Edit')");
    await page.eval("[...document.querySelectorAll('.zp-page-surface:not([hidden]) button')].find(b=>b.textContent.trim()==='Edit').click()");
    await page.waitFor("!!document.querySelector('[role=dialog] textarea')");
    await fill(page, "[role=dialog] textarea", "My new worker rules");
    await page.eval("files.change('WORKERS.md','Rules someone else saved'); files.fail('Zapper file version conflict on WORKERS.md: expected 2, current version is 3.')");
    await submit(page);
    await page.waitFor("document.querySelector('[role=dialog]')?.textContent.includes('Rules someone else saved')");
    assert.equal(await page.eval("document.querySelector('[role=dialog] textarea').value"), "My new worker rules");
    await shot(page, "edit-conflict");
    await page.eval("files.fail(null)");
    await submit(page);
    await page.waitFor("!document.querySelector('[role=dialog]') && document.querySelector('.zp-page-surface:not([hidden])')?.textContent.includes('My new worker rules')");
    const saved = (await page.eval("files.requests()")).filter((r) => r.method === "ZotCommand").at(-1).params.action;
    assert.deepEqual(saved, { type: "writeFile", path: "WORKERS.md", content: "My new worker rules", expectedVersion: 3 });

    // Moving a folder keeps its contents together.
    await page.click('.zp-tabs [aria-label="Drive"]');
    await menu(page, row("reports") + " .zp-tree-more-button", "Rename or move…");
    await page.waitFor("!!document.querySelector('[role=dialog] input[required]')");
    await fill(page, "[role=dialog] input[required]", "archive/reports");
    await submit(page);
    await page.waitFor("!document.querySelector('[role=dialog]')");
    assert.deepEqual(await page.eval("files.files().filter(p=>p.includes('/'))"), ["archive/reports/2026-11-26.md", "archive/reports/2026-11-27.md"]);

    // Errors listing files, then the empty state.
    await page.eval("files.listError('Zapper storage unavailable')");
    await page.waitFor("document.querySelector('.zp-files-error')?.textContent.includes('Zapper storage unavailable')");
    await shot(page, "error-loading");
    await page.eval("files.listError(null); files.empty()");
    await page.waitFor("document.querySelectorAll('.zp-tree-row').length===5 && !!document.querySelector('.zp-files-empty')");
    await shot(page, "tree-empty-dark");
    assert.deepEqual(page.errors, []);
    await page.close();
    page = null;
  }
  console.log("Zapper files checks passed");
} finally {
  await page?.close();
  await server.close();
}
