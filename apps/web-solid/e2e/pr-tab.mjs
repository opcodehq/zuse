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
  assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-merge .btn").dataset.variant`), "default", "merge stays quiet while checks run");
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
    ["cloudflare-workers-and-pages[bot]", "Deploying with Cloudflare Workers"],
    ["coderabbitai[bot]", "Minor · Open the Git panel after switching to the branch's chat."],
    ["maya", "Keep the pending line until the editor shows that file's text. revealWantedLine checks the store snapshot, not the editor content."],
  ]);
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-fb[data-closed]").length`), 0, "resolved group starts closed");
  assert.equal(await page.eval(text(`${S} .ws-fb-closed-toggle`)), "2 resolved");

  await page.eval(`document.querySelectorAll("${S} .ws-fb-toggle")[1].click()`);
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
  assert.ok(draft.startsWith(prompt + "\n\nFix these 3 review comments on #425 (Quieter chat errors, and resume the chat once sign-in lands):\n\n1. Address this pull request comment from cloudflare-workers-and-pages[bot]:"), draft);
  assert.match(draft, /\n\n2\. In @apps\/web-solid\/src\/features\/transcript\/ReferenceMarkdown\.tsx/);
  assert.match(draft, /\n\n3\. Address this review comment from maya on apps\/web-solid\/src\/features\/workspace\/files\/Files\.tsx:188:/);

  await page.eval(clickText(`${S} .ws-fb-closed-toggle`, "2 resolved"));
  const closed = await page.eval(`[...document.querySelectorAll("${S} .ws-fb[data-closed]")].map((f) => [f.querySelector(".ws-fb-title").textContent, getComputedStyle(f.querySelector(".ws-fb-title")).textDecorationLine, !!f.querySelector(".ws-fb-check")])`);
  assert.deepEqual(closed, [["Nit: this regex also matches inside code spans.", "line-through", true], ["Count distinct file paths when checking basename ambiguity.", "line-through", true]]);
  await page.eval(`document.querySelector("${S} .ws-fb[data-closed] .ws-fb-toggle").click()`);
  assert.equal(await page.eval(`document.querySelectorAll("${S} .ws-fb[data-closed][data-expanded]").length`), 1, "a resolved item expands on click");

  for (const theme of ["dark", "light"]) {
    for (const state of ["waiting", "ready"]) {
      await page.eval("localStorage.clear()");
      await page.goto(url(theme, state));
      assert.equal(await page.eval("document.documentElement.dataset.theme"), theme);
      await ready(page);
      if (state === "ready") {
        assert.equal(await page.eval(text(`${S} .ws-pr-status`)), "Open·Ready to merge");
        assert.equal(await page.eval(`document.querySelector("${S} .ws-pr-merge .btn").dataset.variant`), "primary", "merge turns primary once ready");
      } else {
        await page.eval(`document.querySelectorAll("${S} .ws-fb-toggle")[1].click()`);
        await page.eval(clickText(`${S} .ws-fb-closed-toggle`, "2 resolved"));
      }
      await sleep(300);
      if (shots) await page.screenshot(`${shots}/after-${state}-${theme}.png`);
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
