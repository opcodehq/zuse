// Real engine/browser checks. All mutations use a disposable profile and fixture agent.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
const assets = process.env.ZUSE_TEST_ASSETS || buildApp();
const engine = await startEngine({ assets });
const project = mkdtempSync(join(tmpdir(), "zapper-project-"));
writeFileSync(join(project, "README.md"), "# Fixture\n");
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: project });
execFileSync("git", ["add", "."], { cwd: project });
execFileSync(
  "git",
  [
    "-c",
    "user.name=Fixture",
    "-c",
    "user.email=test@example.invalid",
    "commit",
    "-qm",
    "Initial",
  ],
  { cwd: project },
);
const page = await openBrowser(engine.url);
const q = JSON.stringify;
const clickText = async (selector, text) => {
  await page.waitFor(
    `[...document.querySelectorAll(${q(selector)})].some(e=>(e.getAttribute("aria-label") ?? e.textContent.trim())===${q(text)})`,
  );
  if (selector === "[role=menuitem]") {
    await page.eval(
      `[...document.querySelectorAll(${q(selector)})].find(e=>(e.getAttribute("aria-label") ?? e.textContent.trim())===${q(text)}).setAttribute('data-zapper-test-item','')`,
    );
    await page.press("[data-zapper-test-item]");
  } else
    await page.eval(
      `[...document.querySelectorAll(${q(selector)})].find(e=>(e.getAttribute("aria-label") ?? e.textContent.trim())===${q(text)}).click()`,
    );
};
const fill = async (selector, value) =>
  page.eval(
    `(()=>{const e=document.querySelector(${q(selector)});e.value=${q(value)};e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));})()`,
  );
const step = async (name, run) => {
  console.log(name);
  await run();
};
try {
  await page.waitFor(
    "window.__zuse?.state.zappersReady && window.__zuse.state.localDeviceId",
  );
  assert.equal(await page.eval("window.__zuse.state.zappersSupported"), true);
  const deviceId = await page.eval("window.__zuse.state.localDeviceId");
  await engine.rpc("Mutate", {
    op: "createSpace",
    spaceId: "zap-space",
    deviceId,
    path: project,
    name: "Zapper fixture",
    gitDetected: true,
  });
  const config = {
    harness: "claude-code",
    model: null,
    reasoning: null,
    sandbox: "workspace-write",
  };
  await engine.rpc("Mutate", {
    op: "createChat",
    chatId: "zap-worker",
    spaceId: "zap-space",
    branch: "main",
    config,
  });
  await engine.rpc("Mutate", {
    op: "renameChat",
    chatId: "zap-worker",
    title: "Adopt me",
  });
  await page.waitFor(
    '!!document.querySelector(".sb-row[data-chat-id=zap-worker]")',
  );
  await step("Composer stays borderless while focused", async () => {
    await page.waitFor('!!document.querySelector(".cmp-input")');
    const style = await page.eval(
      `(()=>{const e=document.querySelector('.cmp-input');e.focus();return {outline:getComputedStyle(e).outlineStyle, border:getComputedStyle(document.querySelector('.cmp-card')).borderTopWidth}})()`,
    );
    assert.equal(style.outline, "none");
    assert.equal(style.border, "0px");
  });
  await step("Create Zapper in browser", async () => {
    await page.click('[aria-label="New Zapper"]');
    await page.waitFor('!!document.querySelector(".zp-config input")');
    await fill(".zp-config input[required]", "Release captain");
    await fill(".zp-config textarea", "Ship the release");
    await page.click(".zp-config .zp-check input");
    await page.click(".zp-config button[type=submit]");
    await page.waitFor(
      'window.__zuse.state.zappers.length===1 && !!document.querySelector(".zp-inspector")',
    );
  });
  const zapperId = await page.eval("window.__zuse.state.zappers[0].id");
  const mutate = (action) =>
    engine.rpc("ZapperCommand", {
      commandId: randomUUID(),
      zapperId,
      expectedRevision: null,
      action,
      targetDeviceId: deviceId,
    });
  await step("Adopted thread moves exclusively into Zappers", async () => {
    await clickText(".zp-tabs button", "Threads");
    await clickText(".zp-inspector button", "Adopt thread");
    await fill("[role=dialog] select:nth-of-type(1)", "0");
    await page.eval(
      `(()=>{const e=[...document.querySelectorAll('[role=dialog] select')].find(e=>[...e.options].some(o=>o.value==='zap-worker'));e.value='zap-worker';e.dispatchEvent(new Event('change',{bubbles:true}));})()`,
    );
    await fill("[role=dialog] textarea", "Prepare release notes");
    await page.click("[role=dialog] button[type=submit]");
    await page.waitFor(
      'window.__zuse.state.zappers[0].workers.length===1 && window.__zuse.view.selectedChatId==="zap-worker"',
    );
    assert.equal(await page.eval("window.__zuse.view.route.kind"), "zappers");
    assert.equal(
      await page.eval(
        '!!document.querySelector(".sb-row[data-chat-id=zap-worker]")',
      ),
      false,
    );
  });
  await step("Issues and comments persist", async () => {
    await clickText(".zp-tabs button", "Issues");
    await clickText(".zp-inspector button", "New issue");
    await fill("[role=dialog] input[required]", "Review release");
    await fill("[role=dialog] textarea", "Check the changelog");
    await page.click("[role=dialog] button[type=submit]");
    await page.waitFor("window.__zuse.state.zappers[0].issues.length===1");
    await page.waitFor('!!document.querySelector(".zp-issue-detail")');
    await fill(".zp-issue-detail textarea", "Looks ready");
    await page.eval("document.querySelector('.zp-issue-detail textarea').form.requestSubmit()");
    await page.waitFor(
      'window.__zuse.state.zappers[0].issues[0].activity.some(a=>a.text==="Looks ready")',
    );
    await page.click('[aria-label="Close ZAP-1"]');
  });
  await step("Drive creates and versions notes", async () => {
    await clickText(".zp-tabs button", "Overview");
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Drive");
    await page.click('[aria-label="New file"]');
    await fill("[role=dialog] input[required]", "notes.md");
    await fill("[role=dialog] textarea", "Version one");
    await page.click("[role=dialog] button[type=submit]");
    await page.waitFor('!!document.querySelector(".zp-tree-row[data-path=\'notes.md\']") && !document.querySelector("[role=dialog]")');
    await page.click(".zp-tree-row[data-path='notes.md']");
    await page.waitFor('document.querySelector(".zp-page-surface:not([hidden])")?.textContent.includes("Version one")');
    await clickText(".zp-page-surface:not([hidden]) button", "Edit");
    await fill("[role=dialog] textarea", "Version two");
    await page.eval(
      "window.zapperDraftEditor=document.querySelector('[role=dialog] textarea')",
    );
    const issueId = await page.eval(
      "window.__zuse.state.zappers[0].issues[0].id",
    );
    for (let i = 0; i < 12; i++)
      await mutate({ type: "commentIssue", issueId, text: `Live update ${i}` });
    await page.waitFor(
      'window.__zuse.state.zappers[0].issues[0].activity.some(a=>a.text==="Live update 11")',
    );
    assert.equal(
      await page.eval(
        "document.querySelector('[role=dialog] textarea')===window.zapperDraftEditor && window.zapperDraftEditor.value==='Version two'",
      ),
      true,
      "live updates must preserve the open editor and its draft",
    );

    await page.click("[role=dialog] button[type=submit]");
    await page.waitFor(
      'document.querySelector(".zp-inspector-body")?.textContent.includes("Version two") && !document.querySelector("[role=dialog]")',
    );
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Version history");
    await page.eval(
      `[...document.querySelectorAll('.zp-page-surface:not([hidden]) .zp-card')].find(e=>e.querySelector('strong')?.textContent==='notes.md').querySelector('button').click()`,
    );
    await clickText("[role=dialog] button", "Restore version");
    await page.waitFor('!document.querySelector("[role=dialog]")');
    const files = await engine.rpc("ZapperFiles", {
      zapperId,
      history: false,
      targetDeviceId: deviceId,
    });
    assert.equal(
      files.find((f) => f.path === "notes.md").content,
      "Version one",
    );
    assert.equal(files.find((f) => f.path === "notes.md").version, 3);
  });
  await step("Folders nest, move and delete through the engine", async () => {
    await clickText(".zp-tabs button", "Drive");
    await page.waitFor('!!document.querySelector(".zp-tree-row[data-path=\'handoffs\']")');
    await page.click('[aria-label="New folder"]');
    await fill("[role=dialog] input[required]", "handoffs/design");
    await page.click("[role=dialog] button[type=submit]");
    await page.waitFor('window.__zuse.state.zappers[0].folders?.includes("handoffs/design") && !document.querySelector("[role=dialog]")');
    await page.click('[aria-label="New file"]');
    await page.waitFor('document.querySelector("[role=dialog] input[required]")?.value==="handoffs/design/"');
    await fill("[role=dialog] input[required]", "handoffs/design/brief.md");
    await fill("[role=dialog] textarea", "Brief");
    await page.click("[role=dialog] button[type=submit]");
    await page.waitFor('!!document.querySelector(".zp-tree-row[data-path=\'handoffs/design/brief.md\']")');
    await page.press(".zp-tree-row[data-path='handoffs'] .zp-tree-more-button");
    await clickText("[role=menuitem]", "Delete folder…");
    await page.waitFor('document.querySelector("[role=dialog]")?.textContent.includes("1 file and 1 folder inside handoffs")');
    await clickText("[role=dialog] button", "Delete 1 file");
    await page.waitFor('!document.querySelector("[role=dialog]") && !document.querySelector(".zp-tree-row[data-path=\'handoffs\']")');
    const live = await engine.rpc("ZapperFiles", { zapperId, history: false, targetDeviceId: deviceId });
    assert.ok(!live.some((f) => f.path.startsWith("handoffs/")));
    const history = await engine.rpc("ZapperFiles", { zapperId, history: true, targetDeviceId: deviceId });
    assert.deepEqual(
      history.filter((f) => f.path === "handoffs/design/brief.md").map((f) => [f.version, !!f.deleted, f.content]),
      [[2, true, ""], [1, false, "Brief"]],
    );
    assert.deepEqual(JSON.parse(await page.eval("JSON.stringify(window.__zuse.state.zappers[0].folders)")), ["reports"]);
  });
  await step(
    "Delegation creates an isolated worker through the browser",
    async () => {
      await clickText(".zp-tabs button", "Threads");
      await clickText(".zp-inspector button", "Delegate");
      await fill("[role=dialog] textarea", "Run release verification");
      await page.click("[role=dialog] button[type=submit]");
      await page.waitFor(
        'window.__zuse.state.zappers[0].workers.some(w=>w.objective==="Run release verification") && !document.querySelector("[role=dialog]")',
      );
      const workerId = await page.eval(
        'window.__zuse.state.zappers[0].workers.find(w=>w.objective==="Run release verification").chatId',
      );
      assert.ok(workerId.startsWith("zapper-worker-"));
      assert.equal(await page.eval("window.__zuse.view.route.kind"), "zappers");
      await page.waitFor(`!!window.__zuse.state.chats[${q(workerId)}]`);
      assert.equal(
        await page.eval(
          `!!document.querySelector('.sb-row[data-chat-id="${workerId}"]')`,
        ),
        false,
      );
    },
  );
  await step("Decisions reach the coordinator", async () => {
    await mutate({
      type: "decide",
      decision: {
        id: "release-decision",
        workerChatId: "zap-worker",
        question: "Ship today?",
        answer: null,
      },
    });
    await clickText(".zp-tabs button", "Overview");
    await page.waitFor(
      '!!document.querySelector(".zp-inspector .zp-card textarea")',
    );
    await fill(".zp-inspector .zp-card textarea", "Ship tomorrow");
    await page.click(".zp-inspector .zp-card button[type=submit]");
    await page.waitFor(
      'window.__zuse.state.zappers[0].decisions.some(d=>d.answer==="Ship tomorrow")',
    );
  });
  await step("Settings save and pause/resume use engine commands", async () => {
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Zapper settings");
    await page.waitFor('!!document.querySelector(".zp-config textarea")');
    await fill(".zp-config textarea", "Ship the release with verification");
    await page.click(".zp-config button[type=submit]");
    await page.waitFor(
      'window.__zuse.state.zappers[0].config.purpose==="Ship the release with verification" && !!document.querySelector(".zp-inspector")',
    );
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Pause");
    await page.waitFor('window.__zuse.state.zappers[0].status==="paused"');
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Resume");
    await page.waitFor('window.__zuse.state.zappers[0].status==="active"');
  });
  await step("Fresh conversations and reload preserve ownership", async () => {
    const old = await page.eval(
      "window.__zuse.state.zappers[0].coordinatorChatId",
    );
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Fresh conversation");
    await page.waitFor(
      `window.__zuse.state.zappers[0].coordinatorChatId!==${q(old)} && window.__zuse.view.selectedChatId===window.__zuse.state.zappers[0].coordinatorChatId`,
    );
    assert.equal(
      await page.eval(
        `window.__zuse.state.zappers[0].previousCoordinatorChats.includes(${q(old)})`,
      ),
      true,
    );
    await page.goto(engine.url);
    await page.waitFor(
      'window.__zuse?.state.zappersReady && window.__zuse.view.route.kind==="zappers" && !!document.querySelector(".zp-inspector")',
    );
  });
  await step(
    "Workspace shortcuts keep the Zapper conversation open",
    async () => {
      await page.key("e", "KeyE", 69, 2);
      await page.waitFor(
        'window.__zuse.view.rightPanel==="files" && !document.querySelector(".zp-inspector")',
      );
      assert.equal(await page.eval("window.__zuse.view.route.kind"), "zappers");
      await page.key("e", "KeyE", 69, 2);
      await page.waitFor(
        'window.__zuse.view.rightPanel===null && !!document.querySelector(".zp-inspector")',
      );
    },
  );
  await step("Released workers return to regular projects", async () => {
    await mutate({ type: "releaseWorker", chatId: "zap-worker" });
    await page.waitFor(
      '!window.__zuse.state.zappers[0].workers.some(w=>w.chatId==="zap-worker")',
    );
    await page.click('[aria-label="Open Zapper Release captain"]');
    await page.waitFor('!!document.querySelector("[data-chat-id=zap-worker]")');
  });
  await clickText(".zp-tabs button", "Overview");
  await page.screenshot("/tmp/zuse-zappers-desktop.png");
  await page.call("Emulation.setDeviceMetricsOverride", {
    width: 390,
    height: 844,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await page.screenshot("/tmp/zuse-zappers-mobile.png");
  await page.call("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await step("Archive and delete stay in sync with the sidebar", async () => {
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Archive Zapper");
    await clickText("[role=dialog] button", "Confirm");
    await page.waitFor(
      'window.__zuse.state.zappers[0].status==="archived" && !document.querySelector(".zp-sidebar-group")',
    );
    await page.press('[aria-label="Zapper actions"]');
    await clickText("[role=menuitem]", "Delete Zapper");
    await clickText("[role=dialog] button", "Confirm");
    await page.waitFor(
      'window.__zuse.state.zappers.length===0 && !!document.querySelector(".zp-directory")',
    );
    assert.equal(
      await page.eval('!!window.__zuse.state.chats["zap-worker"]'),
      true,
    );
  });
  assert.deepEqual(page.errors, []);
  console.log("Zapper browser checks passed");
} catch (error) {
  console.error("Browser errors:", page.errors);
  console.error(await page.eval("document.body.innerText"));
  throw error;
} finally {
  await page.close();
  await engine.close();
  rmSync(project, { recursive: true, force: true });
  if (!process.env.ZUSE_TEST_ASSETS)
    rmSync(assets, { recursive: true, force: true });
}
