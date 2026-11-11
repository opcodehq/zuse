// Pull request surface against a page-level stand-in for the GitHub-backed PR methods (no GitHub calls); every
// other request reaches a disposable engine. Covers detail, merge refusal, status changes, discussion, an uncertain
// comment that must not be resent, the files diff, list filters and paging, errors, and sanitized markdown.
// Usage: node e2e/workspace-pr.mjs  (ZUSE_ASSETS=<build> skips the build, ZUSE_SHOTS=<dir> keeps screenshots)
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const shots = process.env.ZUSE_SHOTS;
if (shots) mkdirSync(shots, { recursive: true });
const env = { ...process.env, GIT_AUTHOR_NAME: "F", GIT_AUTHOR_EMAIL: "f@example.invalid", GIT_COMMITTER_NAME: "F", GIT_COMMITTER_EMAIL: "f@example.invalid" };
const git = (cwd, ...args) => execFileSync("git", args, { cwd, encoding: "utf8", env });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const root = mkdtempSync(join(tmpdir(), "zuse-wspr-"));
const project = join(root, "p");
git(root, "init", "-q", "-b", "main", project);
writeFileSync(join(project, "README.md"), "# Fixture\n");
git(project, "add", ".");
git(project, "commit", "-qm", "Initial");
git(project, "checkout", "-qb", "feature/pr");

function install() {
  const Native = window.WebSocket;
  const fake = (window.__pr = { log: [], hold: {}, held: [], sockets: [], offline: false, fail: {} });
  const now = Date.now();
  const iso = (m) => new Date(now - m * 60000).toISOString();
  const head = "a".repeat(40);
  const base = "b".repeat(40);
  const own = "https://github.com/acme/widgets/pull/7";
  const comment = (id, login, body, m, extra = {}) => ({ id, url: `${own}#c${id}`, author: { login }, body, state: "", createdAt: iso(m), submittedAt: "", diffHunk: "", ...extra });
  fake.detail = {
    title: "Render the widget list faster", body: "Speeds up **rendering**.\n\n- virtualized rows\n- [docs](https://example.com/docs)\n\n<script>window.__xss = 1</script><img src=x onerror=\"window.__xss=2\">",
    url: own, number: 7, author: { login: "octo" }, baseRefName: "main", baseRefOid: base, headRefName: "feature/pr", headRefOid: head,
    ci: { state: "failed", totalCount: 3 }, state: "OPEN", isDraft: false, reviewDecision: "APPROVED", mergeable: "MERGEABLE", additions: 12, deletions: 3,
    comments: [comment("1", "octo", "First comment with `code`.", 50)],
    reviews: [comment("2", "reviewer", "Looks good", 40, { state: "APPROVED", submittedAt: iso(40) })],
    reviewThreads: [{ id: "t1", isResolved: false, isOutdated: false, path: "src/list.ts", line: 4, comments: [comment("3", "reviewer", "Nit: rename this.", 30, { diffHunk: "@@ -1,3 +1,4 @@\n a\n+b\n c" })] }],
    files: [{ path: "src/list.ts", additions: 10, deletions: 2 }, { path: "README.md", additions: 2, deletions: 1 }],
    statusCheckRollup: [
      { name: "build", context: "", status: "COMPLETED", conclusion: "SUCCESS", state: "", detailsUrl: "https://example.com/build", targetUrl: "" },
      { name: "lint", context: "", status: "COMPLETED", conclusion: "FAILURE", state: "", detailsUrl: "https://example.com/lint", targetUrl: "" },
      { name: "e2e", context: "", status: "IN_PROGRESS", conclusion: "", state: "", detailsUrl: "", targetUrl: "" },
    ],
    autoMergeRequest: null, viewerLogin: "octo",
  };
  const item = (n, title, extra = {}) => ({ provider: "github", author: { login: "octo" }, repository: "acme/widgets", headRefOid: head, ci: { state: "passed", totalCount: 1 }, number: n, title, url: `https://github.com/acme/widgets/pull/${n}`, state: "open", isDraft: false, reviewDecision: "unknown", additions: n, deletions: 1, mergeability: "mergeable", createdAt: iso(n * 10), updatedAt: iso(n), ...extra });
  const pages = { authored: [[item(7, "Render the widget list faster"), item(5, "Draft idea", { isDraft: true })], [item(3, "Older change", { state: "merged" })]], reviewing: [[item(9, "Please review me")]], all: [[item(7, "Render the widget list faster"), item(9, "Please review me"), item(5, "Draft idea", { isDraft: true })]] };
  const patch = "diff --git a/src/list.ts b/src/list.ts\n--- a/src/list.ts\n+++ b/src/list.ts\n@@ -1,2 +1,3 @@\n const a = 1;\n-const b = 2;\n+const b = 3;\n+const c = 4;\n";
  const streams = new Map();
  fake.push = () => { for (const [id, s] of streams) if (s.__kinds?.[id] === "detail") s.__deliver({ id, item: { refreshing: false, snapshot: (({ reviewThreads, ...rest }) => rest)(fake.detail), updatedAt: Date.now() } }); };
  const handlers = {
    GetChangeRequestRepository: () => "acme/widgets",
    ListChangeRequestPage: (p) => { const list = pages[p.filter] ?? []; const i = p.after ? Number(p.after) : 0; return { items: list[i] ?? [], nextCursor: list[i + 1] ? String(i + 1) : null, totalCount: list.flat().length }; },
    GetChangeRequest: () => fake.detail,
    GetChangeRequestDiff: () => patch,
    UpdateChangeRequest: (p) => { if (p.update.kind === "markDraft") fake.detail.isDraft = true; if (p.update.kind === "markReady") fake.detail.isDraft = false; if (p.update.kind === "close") fake.detail.state = "CLOSED"; if (p.update.kind === "reopen") fake.detail.state = "OPEN"; return { refusal: null }; },
    UpdateChangeRequestAtHead: (p) => (p.update.kind === "merge" ? { refusal: "Merging is blocked by a failing required check." } : { refusal: null }),
    PostChangeRequestComment: (p) => { fake.detail.comments.push(comment(String(Date.now()), "octo", p.body, 0)); return {}; },
  };
  window.WebSocket = class extends Native {
    constructor(url, protocols) { super(fake.offline ? "ws://127.0.0.1:9/" : url, protocols); fake.sockets.push(this); this.addEventListener("close", () => { for (const [id, s] of streams) if (s === this) streams.delete(id); }); }
    __deliver(frame) { if (this.readyState === 1) this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(frame) })); }
    send(text) {
      let frame;
      try { frame = JSON.parse(text); } catch { return super.send(text); }
      if (frame.cancel) { if (streams.delete(frame.id)) return; return super.send(text); }
      const params = frame.params ?? {};
      if (frame.method === "WatchCheckoutChangeRequest") {
        streams.set(frame.id, this);
        setTimeout(() => this.__deliver({ id: frame.id, item: { checkoutId: "x", deviceId: "d", cwd: params.cwd, branch: "feature/pr", updatedAt: new Date().toISOString(), changeRequest: { provider: "github", number: 7, title: fake.detail.title, url: own, state: "open", baseRef: "main", headRef: "feature/pr" } } }), 20);
        return;
      }
      if (frame.method === "WatchChangeRequest") {
        streams.set(frame.id, this);
        (this.__kinds ??= {})[frame.id] = "detail";
        setTimeout(() => this.__deliver({ id: frame.id, item: { refreshing: false, snapshot: (({ reviewThreads, ...rest }) => rest)(fake.detail), updatedAt: Date.now() } }), 30);
        return;
      }
      const handler = handlers[frame.method];
      if (!handler) return super.send(text);
      fake.log.push({ method: frame.method, params });
      const answer = () => {
        if (fake.fail[frame.method]) return this.__deliver({ id: frame.id, err: fake.fail[frame.method] });
        this.__deliver({ id: frame.id, ok: JSON.parse(JSON.stringify(handler(params))) });
      };
      if (fake.hold[frame.method]) return void fake.held.push(answer);
      setTimeout(answer, 40);
    }
  };
}

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets });
const page = await openBrowser(engine.url, { width: 1500, height: 900 });
await page.call("Page.addScriptToEvaluateOnNewDocument", { source: `(${install.toString()})();` });
await page.call("Page.reload");
await sleep(600);
const results = [];
const step = async (name, fn) => {
  process.stdout.write(`- ${name} … `);
  try { await fn(); results.push([name, "ok"]); console.log("ok"); } catch (e) { results.push([name, `FAIL ${e.message}`]); console.log(`FAIL ${e.message}`); if (shots) await page.screenshot(join(shots, `fail-${name}.png`)).catch(() => {}); }
};
const shot = async (n) => shots && (await page.screenshot(join(shots, `${n}.png`)));
const pickMenu = async (label) => {
  const find = `[...document.querySelectorAll(".menu-item")].find((m) => m.textContent.trim() === ${JSON.stringify(label)})`;
  await page.waitFor(`!!${find}`);
  await page.eval(`(() => { document.querySelectorAll("[data-e2e-pick]").forEach((e) => e.removeAttribute("data-e2e-pick")); ${find}.setAttribute("data-e2e-pick", ""); return true })()`);
  await page.press("[data-e2e-pick]");
};
const log = () => page.eval(`JSON.stringify(window.__pr.log.map((l) => l.method + (l.params.update ? ":" + l.params.update.kind + (l.params.update.method ? "/" + l.params.update.method : "") : "")))`);
const txt = (sel) => page.eval(`document.querySelector(${JSON.stringify(sel)})?.textContent ?? ""`);
try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced && window.__zuse.state.localDeviceId`);
  const deviceId = await page.eval(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "s", deviceId, path: project, name: "fixture", gitDetected: true });
  await engine.rpc("Mutate", { op: "createChat", chatId: "c", spaceId: "s", branch: "feature/pr", config: { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" } });
  await page.waitFor(`!!document.querySelector('[data-chat-id="c"] .sb-row-open')`);
  await page.click(`[data-chat-id="c"] .sb-row-open`);
  await page.waitFor(`document.querySelector(".ws-branch-label")?.textContent === "feature/pr"`);
  await page.click('button[aria-label="Show panel"]');
  await sleep(400);
  await page.press('button[aria-label="Add to panel"]');
  await pickMenu("PR");

  await step("own-pr-detail", async () => {
    await page.waitFor(`/Render the widget list faster/.test(document.querySelector("#ws-surface-pullRequest .ws-pr-title")?.textContent ?? "")`, 10000);
    await sleep(500);
    await shot("detail-overview");
    const t = await txt("#ws-surface-pullRequest");
    for (const want of ["#7", "octo", "main", "feature/pr", "This branch", "Approved", "build", "lint", "e2e", "Checks", "Description", "virtualized rows"]) if (!t.includes(want)) throw new Error(`missing ${want}`);
    const xss = await page.eval(`window.__xss ?? null`);
    const scripts = await page.eval(`document.querySelectorAll("#ws-surface-pullRequest .ws-markdown script, #ws-surface-pullRequest .ws-markdown img[onerror]").length`);
    if (xss || scripts) throw new Error(`unsanitized markdown ${xss} ${scripts}`);
    const link = await page.eval(`(() => { const a = document.querySelector("#ws-surface-pullRequest .ws-markdown a"); return a && a.target + " " + a.rel })()`);
    if (link !== "_blank noopener noreferrer") throw new Error(`link attrs ${link}`);
    console.log(" merge box:", await txt("#ws-surface-pullRequest .ws-pr-merge"));
  });

  await step("merge-refusal", async () => {
    await page.press('#ws-surface-pullRequest .ws-pr-merge button');
    await pickMenu("Squash and merge");
    await page.waitFor(`/blocked by a failing/.test(document.querySelector("#ws-surface-pullRequest")?.textContent ?? "")`, 5000);
    await shot("merge-refused");
    const l = await log();
    if (!l.includes("UpdateChangeRequestAtHead:merge/squash")) throw new Error(l);
  });

  await step("status-draft", async () => {
    await page.press('#ws-surface-pullRequest button[aria-label="Pull request actions"]');
    await pickMenu("Convert to draft");
    await page.waitFor(`/still a draft/.test(document.querySelector("#ws-surface-pullRequest .ws-pr-merge")?.textContent ?? "")`, 5000);
    await shot("draft");
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .ws-pr-merge button")].find((b) => b.textContent.includes("Ready for review")).click()`);
    await page.waitFor(`!/still a draft/.test(document.querySelector("#ws-surface-pullRequest .ws-pr-merge")?.textContent ?? "")`, 5000);
  });

  await step("activity-and-comment", async () => {
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .tab")].find((t) => t.textContent.startsWith("Activity")).click()`);
    await page.waitFor(`document.querySelectorAll("#ws-surface-pullRequest .ws-comment").length === 3`, 5000);
    await shot("activity");
    // A watch frame carries the summary, without inline threads; it must not drop the thread comment.
    await page.eval(`window.__pr.detail.title = "Render the widget list faster (v2)"; window.__pr.push(); true`);
    await sleep(400);
    const n = await page.eval(`document.querySelectorAll("#ws-surface-pullRequest .ws-comment").length`);
    if (n !== 3) throw new Error(`watch frame changed comments to ${n}`);
    await page.eval(`(() => { const t = document.querySelector('#ws-surface-pullRequest textarea[aria-label="Comment"]'); t.focus(); return true })()`);
    await page.type("Thanks, merging soon.");
    await page.key("Enter", "Enter", 13, 2);
    await page.waitFor(`document.querySelectorAll("#ws-surface-pullRequest .ws-comment").length === 4`, 5000);
    const value = await page.eval(`document.querySelector('#ws-surface-pullRequest textarea[aria-label="Comment"]').value`);
    if (value) throw new Error("draft not cleared");
    await shot("commented");
  });

  await step("uncertain-comment", async () => {
    await page.eval(`window.__pr.hold.PostChangeRequestComment = true; true`);
    await page.eval(`(() => { const t = document.querySelector('#ws-surface-pullRequest textarea[aria-label="Comment"]'); t.focus(); return true })()`);
    await page.type("Will this land?");
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .ws-pr-compose button")].find((b) => b.textContent === "Comment").click()`);
    await sleep(200);
    await page.eval(`(() => { window.__pr.offline = true; for (const s of window.__pr.sockets) s.close(); return true })()`);
    await page.waitFor(`/uncertain/.test(document.querySelector("#ws-surface-pullRequest")?.textContent ?? "")`, 10000);
    await shot("uncertain");
    await page.eval(`(() => { window.__pr.offline = false; window.__pr.hold = {}; return true })()`);
    await page.waitFor(`window.__zuse.state.connection.kind === "connected"`, 30000);
    await sleep(1500);
    const posts = (await log()).split("PostChangeRequestComment").length - 1;
    const draft = await page.eval(`document.querySelector('#ws-surface-pullRequest textarea[aria-label="Comment"]')?.value`);
    console.log(" posts:", posts, "draft kept:", draft, "notice:", await txt("#ws-surface-pullRequest .ws-banner"));
    if (posts !== 2) throw new Error(`comment resent: ${posts}`);
  });

  await step("files-tab", async () => {
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .tab")].find((t) => t.textContent.startsWith("Files")).click()`);
    await page.waitFor(`[...document.querySelectorAll("#ws-surface-pullRequest .ws-diff-text")].some((e) => e.textContent === "const c = 4;")`, 5000);
    await shot("files");
  });

  await step("list-filters-paging", async () => {
    await page.click('#ws-surface-pullRequest button[aria-label="All pull requests"]');
    await page.waitFor(`document.querySelectorAll("#ws-surface-pullRequest .ws-pr-row").length === 2`, 5000);
    await shot("list-mine");
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .ws-pr-more button")][0].click()`);
    await page.waitFor(`document.querySelectorAll("#ws-surface-pullRequest .ws-pr-row").length === 3`, 5000);
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .tab")].find((t) => t.textContent === "Reviewing").click()`);
    await page.waitFor(`document.querySelectorAll("#ws-surface-pullRequest .ws-pr-row").length === 1`, 5000);
    await shot("list-reviewing");
    await page.eval(`document.querySelector("#ws-surface-pullRequest .ws-pr-row").click()`);
    await page.waitFor(`/#7|#9/.test(document.querySelector("#ws-surface-pullRequest .ws-panel-title")?.textContent ?? "")`, 5000);
    const own = await txt("#ws-surface-pullRequest .ws-pr-meta");
    console.log(" opened from list; meta:", own.slice(0, 120));
  });

  await step("detail-error", async () => {
    await page.eval(`window.__pr.fail.GetChangeRequest = "GitHub is not reachable"; true`);
    await page.click('#ws-surface-pullRequest button[aria-label="Refresh pull request"]');
    await page.waitFor(`/not reachable/.test(document.querySelector("#ws-surface-pullRequest .ws-banner")?.textContent ?? "")`, 5000);
    await shot("detail-error");
    await page.eval(`delete window.__pr.fail.GetChangeRequest; true`);
    await page.eval(`[...document.querySelectorAll("#ws-surface-pullRequest .ws-banner button")].find((b) => b.textContent === "Retry").click()`);
    await page.waitFor(`!document.querySelector("#ws-surface-pullRequest .ws-banner")`, 5000);
  });

  await step("light", async () => {
    await page.eval(`document.documentElement.dataset.theme = "light"; true`);
    await sleep(200);
    await shot("light");
  });
  const errors = page.errors.filter((e) => !/favicon/.test(e));
  if (errors.length) throw new Error(`page errors:\n${errors.join("\n")}`);
} catch (e) {
  console.error(e);
  process.exitCode = 1;
} finally {
  const failed = results.filter(([, r]) => r !== "ok");
  if (failed.length) {
    console.error(`${failed.length} step(s) failed: ${failed.map(([n]) => n).join(", ")}`);
    process.exitCode = 1;
  } else if (!process.exitCode) console.log("workspace PR e2e ok");
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(root, { recursive: true, force: true });
}
