// Pull request overview against a fixture pull request: one-line status, sorted checks, the Feedback list with
// prompts added to the linked chat's composer, and resolved threads grouped apart, light and dark.
// Usage: node e2e/pr-tab.mjs (ZUSE_SHOTS=dir keeps screenshots).
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import { createServer } from "vite";
import { openBrowser } from "./harness.mjs";

const root = new URL("..", import.meta.url).pathname;
const shots = process.env.ZUSE_SHOTS;
if (shots) mkdirSync(shots, { recursive: true });
const server = await createServer({ root, configFile: `${root}/vite.config.ts`, server: { host: "127.0.0.1", port: 0, hmr: { overlay: false } } });
await server.listen();
const url = (theme, state = "waiting") => `http://127.0.0.1:${server.httpServer.address().port}/e2e/pr-tab/fixture.html?theme=${theme}&state=${state}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const S = "#ws-surface-pullRequest";
const text = (sel) => `(document.querySelector(${JSON.stringify(sel)})?.textContent ?? "").trim()`;
const all = (sel) => `[...document.querySelectorAll(${JSON.stringify(sel)})].map((e) => e.textContent.trim())`;
const clickText = (sel, label) => `[...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.textContent.trim().startsWith(${JSON.stringify(label)})).click()`;
const ready = async (page) => page.waitFor(`/Quieter chat errors/.test(${text(`${S} .ws-pr-title`)})`, 30000);
let page;
try {
  page = await openBrowser(url("dark"), { width: 1000, height: 1500 });
  await ready(page);

  assert.equal(await page.eval(text(`${S} .ws-pr-status`)), "Open·Waiting on checks");
  assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-state").dataset.tone`), "success");
  assert.equal(await page.eval(`[...document.querySelectorAll("${S} .ws-pr-status [data-tone]:not(.ws-pr-icon)")].length`), 1, "only the state word carries a tone");
  assert.deepEqual(await page.eval(all(`${S} .ws-pr-prop dt`)), ["Branch", "Status", "Reviews", "Checks", "Thread"]);
  assert.match(await page.eval(text(`${S} .ws-pr-props`)), /zuse\/quiet-errors-resume-after-sign-in\s*main6 files\+176−131/);
  assert.match(await page.eval(text(`${S} .ws-pr-props`)), /2 running, 7 passed/);
  assert.equal(await page.eval(`[...document.querySelectorAll("${S} .ws-pr-prop-icon")].every((i) => i.style.width === "14px")`), true);
  assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-ref .md-ref")?.textContent`), "Quieter chat errors", "thread row uses the reference chip");
  assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-merge-main").dataset.variant`), "default", "merge stays quiet while checks run");
  assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-desc").hasAttribute("data-clamped")`), true, "description starts collapsed");
  await page.eval(clickText(`${S} .ws-pr-link`, "Show more"));
  assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-desc").hasAttribute("data-clamped")`), false);
  await page.eval(clickText(`${S} .ws-pr-link`, "Show less"));

  assert.deepEqual(await page.eval(all(`${S} .ws-pr-section-toggle`)), ["Checks9", "Feedback3"]);
  const rows = await page.eval(`[...document.querySelectorAll("${S} .ws-check-row")].map((r) => [r.querySelector(".ws-grow").textContent, r.dataset.tone])`);
  assert.deepEqual(rows.slice(0, 3), [["rust", "running"], ["native (ubuntu-24.04)", "running"], ["platforms", "passed"]]);
  assert.equal(rows.length, 9);
  await page.eval(clickText(`${S} .ws-pr-section-toggle`, "Checks"));
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-check-row").length`), 0, "checks collapse");
  await page.eval(clickText(`${S} .ws-pr-section-toggle`, "Checks"));

  const feedback = await page.eval(`[...document.querySelectorAll("${S} .ws-fb:not([data-closed])")].map((f) => [f.querySelector(".ws-fb-author").textContent, f.querySelector(".ws-fb-summary").textContent])`);
  assert.deepEqual(feedback, [
    ["coderabbitai[bot]", "Minor · Open the Git panel after switching to the branch's chat."],
    ["maya", "Keep the banner change in its own pull request; it touches the shell layout and needs its own review."],
    ["maya", "Keep the pending line until the editor shows that file's text. revealWantedLine checks the store snapshot, not the editor content."],
  ]);
  assert.equal(/cloudflare|Deploying|Walkthrough|Actionable comments/i.test(await page.eval(text(`${S} [aria-label=Feedback]`))), false, "bot status comments and review walkthroughs are not feedback");
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-fb[data-closed]").length`), 0, "resolved group starts closed");
  assert.equal(await page.eval(text(`${S} .ws-fb-closed-toggle`)), "2 resolved");

  await page.eval(`document.querySelectorAll("${S} .ws-fb-toggle")[0].click()`);
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-fb[data-expanded]").length`), 1);
  const prompt = "In @apps/web-solid/src/features/transcript/ReferenceMarkdown.tsx around lines 267 - 270, when the branch belongs to another chat, open that chat and then call openPanel(\"git\") so the Git view shows; keep the current-chat path unchanged.";
  assert.equal(await page.eval(text(`${S} .ws-fb[data-expanded] .ws-fb-prompt-text`)), prompt);
  assert.equal(await page.eval(text(`${S} .ws-fb[data-expanded] .ws-fb-path`)), "apps/web-solid/src/features/transcript/ReferenceMarkdown.tsx:270");
  assert.match(await page.eval(text(`${S} .ws-fb[data-expanded] .ws-comment-hunk`)), /openPanel/);
  assert.equal(/Prompt for AI|fingerprinting/.test(await page.eval(text(`${S} .ws-fb[data-expanded] .ws-markdown`))), false, "the prompt block and markup comments stay out of the body");

  await page.eval(`document.querySelector("${S} .ws-fb[data-expanded] [aria-label='Add to chat']").click()`);
  assert.equal(await page.eval("__pr.draft()"), prompt, "add puts the bot's prompt in the composer without sending");
  assert.equal(await page.eval("document.querySelector('.cmp-input').value"), prompt);
  assert.equal(await page.eval("__pr.calls.filter((c) => c.method !== 'GetChangeRequest').length"), 0, "nothing is sent");

  await page.eval(`document.querySelectorAll("${S} .ws-fb-toggle")[2].click()`);
  assert.equal(await page.eval(text(`${S} .ws-fb[data-expanded]:last-child .ws-fb-prompt-text`)), "Address this review comment from maya on apps/web-solid/src/features/workspace/files/Files.tsx:188:\n\nKeep the pending line until the editor shows that file's text. `revealWantedLine` checks the store snapshot, not the editor content.");

  await page.eval(clickText(`${S} .ws-fb-add-all`, "Add all"));
  const draft = await page.eval("__pr.draft()");
  assert.ok(draft.startsWith("Fix these 3 review comments on #425 (Quieter chat errors, and resume the chat once sign-in lands):\n\n1. In @apps/web-solid/src/features/transcript/ReferenceMarkdown.tsx"), draft);
  assert.equal(draft.split(prompt).length - 1, 1, "Add all replaces the item added before instead of repeating it");
  assert.match(draft, /\n\n2\. Address this review from maya:/);
  assert.match(draft, /\n\n3\. Address this review comment from maya on apps\/web-solid\/src\/features\/workspace\/files\/Files\.tsx:188:/);
  assert.equal(/cloudflare|Deploying/i.test(draft), false);
  await page.eval(clickText(`${S} .ws-fb-add-all`, "Add all"));
  assert.equal(await page.eval("__pr.draft()"), draft, "a second Add all leaves one copy");

  await page.eval(clickText(`${S} .ws-fb-closed-toggle`, "2 resolved"));
  const closed = await page.eval(`[...document.querySelectorAll("${S} .ws-fb[data-closed]")].map((f) => [f.querySelector(".ws-fb-title").textContent, getComputedStyle(f.querySelector(".ws-fb-title")).textDecorationLine, !!f.querySelector(".ws-fb-check")])`);
  assert.deepEqual(closed, [["Nit: this regex also matches inside code spans.", "line-through", true], ["Count distinct file paths when checking basename ambiguity.", "line-through", true]]);
  await page.eval(`document.querySelector("${S} .ws-fb[data-closed] .ws-fb-toggle").click()`);
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-fb[data-closed][data-expanded]").length`), 1, "a resolved item expands on click");

  // Merge control: one split button that never looks disabled; comment, method, auto-merge and force merge.
  const calls = () => page.eval("__pr.calls.filter((c) => c.method !== 'GetChangeRequest')");
  const mergeLabel = () => page.eval(text(`${S} .ws-pr-merge-main`));
  assert.equal(await mergeLabel(), "Auto-merge");
  assert.deepEqual(await page.eval(`(() => { const b = document.querySelector("${S} .ws-pr-merge-main"); return [b.dataset.variant, b.disabled] })()`), ["default", false], "pending checks keep the button quiet but enabled");
  await page.press(`${S} .ws-pr-merge-more`);
  await page.waitFor("!!document.querySelector('.ws-merge-menu')", 5000);
  assert.deepEqual(await page.eval(all(".ws-merge-menu [role=radio] .ws-merge-text > span:first-child")), ["Squash and merge", "Rebase and merge"], "only the methods the repository allows");
  assert.equal(await page.eval(text(".ws-merge-menu [role=radio][aria-checked=true]")), "Squash and mergeCombine all commits");
  await page.eval(clickText(".ws-merge-menu [role=radio]", "Rebase and merge"));
  assert.equal(await page.eval("localStorage.getItem('zuse.pullRequest.mergeMethod')"), "rebase", "the pick is remembered");
  await page.eval("(() => { const t = document.querySelector('.ws-merge-comment'); t.value = 'Shipping once CI is green.'; t.dispatchEvent(new InputEvent('input', { bubbles: true })); })()");
  assert.deepEqual(await page.eval(all(".ws-merge-menu .ws-merge-row:not([role=radio]) .ws-merge-text > span:first-child")), ["Enable auto-merge", "Force merge"]);
  await page.eval(clickText(".ws-merge-menu .ws-merge-row", "Enable auto-merge"));
  await page.waitFor(`/Auto-merge on/.test(${text(`${S} .ws-pr-merge-main`)})`, 5000);
  assert.deepEqual((await calls()).slice(-2), [
    { method: "UpdateChangeRequestAtHead", params: { url: "https://github.com/opcodehq/zuse/pull/425", update: { kind: "enableAutoMerge", method: "rebase" }, headRefOid: "a".repeat(40), targetDeviceId: "fixture-device" } },
    { method: "PostChangeRequestComment", params: { url: "https://github.com/opcodehq/zuse/pull/425", body: "Shipping once CI is green.", targetDeviceId: "fixture-device" } },
  ], "auto-merge uses the picked method and posts the comment after GitHub accepts it");
  assert.equal(await page.eval("!!document.querySelector('.ws-merge-menu')"), false, "the menu closes once done");
  await page.press(`${S} .ws-pr-merge-main`);
  await page.waitFor("!!document.querySelector('.ws-merge-menu')", 5000);
  assert.equal(await page.eval("!!document.querySelector('.ws-merge-comment')"), false, "no comment box while auto-merge is on");
  await page.eval(clickText(".ws-merge-menu .ws-merge-row", "Cancel auto-merge"));
  await page.waitFor(`${text(`${S} .ws-pr-merge-main`)} === "Auto-merge"`, 5000);
  assert.deepEqual((await calls()).at(-1).params.update, { kind: "disableAutoMerge" });
  await page.press(`${S} .ws-pr-merge-more`);
  await page.waitFor("!!document.querySelector('.ws-merge-menu')", 5000);
  await page.eval(clickText(".ws-merge-menu .ws-merge-row", "Force merge"));
  assert.match(await page.eval(text(".ws-merge-confirm")), /Merge before checks pass\?/, "force merge asks first");
  const before = (await calls()).length;
  await page.eval(clickText(".ws-merge-confirm button", "Cancel"));
  assert.equal((await calls()).length, before, "cancelling sends nothing");
  await page.eval(clickText(".ws-merge-menu .ws-merge-row", "Force merge"));
  await page.eval(clickText(".ws-merge-confirm button", "Force merge"));
  await page.waitFor(`/Required status checks are still running/.test(${text(`${S} .ws-banner`)})`, 5000);
  assert.deepEqual((await calls()).at(-1).params.update, { kind: "merge", method: "rebase" });
  await page.key("Escape", "Escape", 27);
  await sleep(200);

  // Overflow menu: every existing action, with Copy and Threads submenus and a confirmed close.
  await page.press(`${S} [aria-label="Pull request actions"]`);
  await page.waitFor("document.querySelectorAll('.menu .menu-item').length > 3", 5000);
  assert.deepEqual(await page.eval(all(".menu .menu-item")), ["Open in GitHub", "Copy", "Threads", "Refresh", "Convert to draft", "Merge pull request", "Close pull request"]);
  assert.equal(await page.eval("[...document.querySelectorAll('.menu .menu-item')].at(-1).dataset.danger"), "true");
  await page.eval("[...document.querySelectorAll('.menu .menu-item')].find((m) => m.textContent.trim() === 'Copy').setAttribute('data-e2e', 'copy')");
  await page.press("[data-e2e=copy]");
  await page.waitFor("document.querySelectorAll('.menu').length > 1", 5000);
  assert.deepEqual(await page.eval("[...document.querySelectorAll('.menu')].at(-1) && [...[...document.querySelectorAll('.menu')].at(-1).querySelectorAll('.menu-item')].map((m) => m.textContent.trim())"), ["Link", "Branch namezuse/quiet-errors-resume-af…", "Number#425"]);
  await page.key("Escape", "Escape", 27);
  await page.key("Escape", "Escape", 27);
  await sleep(200);
  await page.press(`${S} [aria-label="Pull request actions"]`);
  await page.waitFor("document.querySelectorAll('.menu .menu-item').length > 3", 5000);
  await page.eval("[...document.querySelectorAll('.menu .menu-item')].find((m) => m.textContent.trim() === 'Threads').setAttribute('data-e2e', 'threads')");
  await page.press("[data-e2e=threads]");
  await page.waitFor("document.querySelectorAll('.menu').length > 1", 5000);
  assert.deepEqual(await page.eval("[...[...document.querySelectorAll('.menu')].at(-1).querySelectorAll('.menu-item')].map((m) => m.textContent.trim())"), ["Quieter chat errors"]);
  await page.key("Escape", "Escape", 27);
  await page.key("Escape", "Escape", 27);
  await sleep(200);
  await page.press(`${S} [aria-label="Pull request actions"]`);
  await page.waitFor("document.querySelectorAll('.menu .menu-item').length > 3", 5000);
  await page.eval("[...document.querySelectorAll('.menu .menu-item')].find((m) => m.textContent.trim() === 'Close pull request').setAttribute('data-e2e', 'close')");
  await page.press("[data-e2e=close]");
  await page.waitFor("!!document.querySelector('.dialog')", 5000);
  assert.equal(await page.eval(text(".dialog-title")), "Close this pull request?");
  const beforeClose = (await calls()).length;
  await page.eval(clickText(".dialog-footer button", "Close pull request"));
  await page.waitFor(`${text(`${S} .ws-pr-state`)} === "Closed"`, 5000);
  assert.equal((await calls()).length, beforeClose + 1);
  assert.deepEqual((await calls()).at(-1).params.update, { kind: "close" });
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-pr-merge").length`), 0, "closed pull requests show no merge button");
  await page.press(`${S} [aria-label="Pull request actions"]`);
  await page.waitFor("document.querySelectorAll('.menu .menu-item').length > 3", 5000);
  assert.equal(await page.eval("[...document.querySelectorAll('.menu .menu-item')].at(-1).textContent.trim()"), "Reopen pull request");
  await page.eval("[...document.querySelectorAll('.menu .menu-item')].at(-1).setAttribute('data-e2e', 'reopen')");
  await page.press("[data-e2e=reopen]");
  await page.waitFor(`${text(`${S} .ws-pr-state`)} === "Open"`, 5000);

  // Replies: an inline reply from an expanded review thread, and a top-level comment from Activity.
  const thread = `[...document.querySelectorAll("${S} .ws-fb:not([data-closed])")].find((f) => /pending line/.test(f.textContent))`;
  await page.eval(`${thread}.hasAttribute("data-expanded") || ${thread}.querySelector(".ws-fb-toggle").click()`);
  await page.waitFor(`!!${thread}.querySelector(".ws-fb-reply textarea")`, 5000);
  await page.eval(`(() => { const t = ${thread}.querySelector(".ws-fb-reply textarea"); t.value = "Fixed in the next push."; t.dispatchEvent(new InputEvent("input", { bubbles: true })); })()`);
  await sleep(50);
  await page.eval(`${thread}.querySelector(".ws-fb-reply").requestSubmit()`);
  await page.waitFor("__pr.calls.some((c) => c.method === 'ReplyChangeRequestThread')", 5000);
  assert.deepEqual((await calls()).find((c) => c.method === "ReplyChangeRequestThread").params, { url: "https://github.com/opcodehq/zuse/pull/425", threadId: "t2", body: "Fixed in the next push.", targetDeviceId: "fixture-device" });
  await page.waitFor(`/2 replies on GitHub/.test(${thread}.textContent) && ${thread}.querySelector(".ws-fb-reply textarea").value === ""`, 5000);
  await page.eval(clickText(`${S} .tab`, "Activity"));
  await page.waitFor(`!!document.querySelector("${S} .ws-pr-compose textarea")`, 5000);
  await page.eval(`(() => { const t = document.querySelector("${S} .ws-pr-compose textarea"); t.value = "Thanks for the reviews."; t.dispatchEvent(new InputEvent("input", { bubbles: true })); })()`);
  await page.eval(clickText(`${S} .ws-pr-compose button`, "Comment"));
  await page.waitFor("__pr.calls.some((c) => c.method === 'PostChangeRequestComment' && c.params.body === 'Thanks for the reviews.')", 5000);
  await page.eval(clickText(`${S} .tab`, "Overview"));

  for (const theme of ["dark", "light"]) {
    for (const state of ["waiting", "ready"]) {
      await page.eval("localStorage.clear()");
      await page.goto(url(theme, state));
      await ready(page);
      assert.equal(await page.eval("document.documentElement.dataset.theme"), theme);
      if (state === "ready") {
        assert.equal(await page.eval(text(`${S} .ws-pr-status`)), "Open·Ready to merge");
        assert.deepEqual(await page.eval(`[document.querySelector("${S} .ws-pr-merge-main").textContent.trim(), document.querySelector("${S} .ws-pr-merge-main").dataset.variant]`), ["Merge", "primary"], "merge turns primary once ready");
      } else {
        await page.eval(`document.querySelectorAll("${S} .ws-fb-toggle")[0].click()`);
        await page.eval(clickText(`${S} .ws-fb-closed-toggle`, "2 resolved"));
      }
      await sleep(300);
      if (shots) await page.screenshot(`${shots}/after-${state}-${theme}.png`);
      if (state === "waiting") {
        await page.eval(`document.querySelector("${S} .ws-pr-scroll").scrollTop = 0`);
        await page.press(`${S} .ws-pr-merge-more`);
        await page.waitFor("!!document.querySelector('.ws-merge-menu')", 5000);
        await sleep(200);
        if (shots) await page.screenshot(`${shots}/after-merge-menu-${theme}.png`);
        await page.key("Escape", "Escape", 27);
        await sleep(200);
        await page.press(`${S} [aria-label="Pull request actions"]`);
        await page.waitFor("document.querySelectorAll('.menu .menu-item').length > 3", 5000);
        await page.eval("[...document.querySelectorAll('.menu .menu-item')].find((m) => m.textContent.trim() === 'Copy').setAttribute('data-e2e', 'copy')");
        await page.press("[data-e2e=copy]");
        await page.waitFor("document.querySelectorAll('.menu').length > 1", 5000);
        await sleep(200);
        if (shots) await page.screenshot(`${shots}/after-overflow-${theme}.png`);
        await page.key("Escape", "Escape", 27);
        await page.key("Escape", "Escape", 27);
      }
    }
  }
  await page.goto(url("dark", "merged"));
  await ready(page);
  assert.equal(await page.eval(text(`${S} .ws-pr-status`)), "Merged");
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-pr-merge").length`), 0, "merged pull requests offer no merge");
  assert.deepEqual(page.errors.filter((e) => !/github\.com|ERR_|Failed to load resource/.test(e)), []);
  console.log("pr-tab: ok");
} finally {
  page?.close?.();
  await server.close();
}
