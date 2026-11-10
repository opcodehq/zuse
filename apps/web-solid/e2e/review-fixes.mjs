// Regression checks for the second review round, against a disposable engine and a scripted agent in headless Chromium.
// Usage: node e2e/review-fixes.mjs [name ...]   (ZUSE_SHOTS=/dir saves screenshots, ZUSE_ASSETS reuses a build)
import { execFileSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { buildApp, openBrowser, startEngine } from "./harness.mjs";

const only = process.argv.slice(2);
const shots = process.env.ZUSE_SHOTS;
const scratch = mkdtempSync(join(tmpdir(), "zuse-review-"));
const project = join(scratch, "demo");
mkdirSync(project, { recursive: true });
writeFileSync(join(project, "README.md"), "# demo\n");
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" };
execFileSync("git", ["init", "-q", "-b", "main"], { cwd: project });
execFileSync("git", ["add", "."], { cwd: project });
execFileSync("git", ["commit", "-qm", "init"], { cwd: project, env: gitEnv });
mkdirSync(join(scratch, "home"));
process.env.ZUSE_WORKTREES_DIR = join(scratch, "worktrees");
process.env.PATH = "/usr/local/bin:/usr/bin:/bin";

const line = (event) => `printf '%s\\n' '${JSON.stringify(event).replace(/'/g, "'\\''")}'`;
const delta = (text) => ({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } } });
const say = (text) => [
  line(delta(text)),
  line({ type: "assistant", message: { role: "assistant", model: "fixture", content: [{ type: "text", text }] } }),
  line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: text }),
].join("\n");
const words = Array.from({ length: 150 }, (_, i) => `alpha${i} `);
function writeAgent(path) {
  writeFileSync(path, `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
while IFS= read -r prompt; do
  case "$prompt" in
    *'"subtype":"initialize"'*)
      id=$(printf '%s' "$prompt" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"commands":[]}}}\\n' "$id"
      [ "$id" = "zuse-command-probe" ] && exit 0;;
    *) break;;
  esac
done
${line({ type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] })}
case "$prompt" in
  *slowly*) sleep 20
${say("Slow reply done.")};;
  *streamwords*)
${words.map((w) => `${line(delta(w))}\nsleep 0.04`).join("\n")}
${line({ type: "assistant", message: { role: "assistant", model: "fixture", content: [{ type: "text", text: words.join("") }] } })}
${line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: words.join("") })};;
  *permission*)
${line({ type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "p1", name: "Bash", input: { command: "rm -rf build" } }] } })}
${line({ type: "control_request", request_id: "perm-1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "rm -rf build" } } })}
while IFS= read -r answer; do case "$answer" in *control_response*) break;; esac; done
${line({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "p1", content: "removed" }] } })}
${say("Cleaned the build folder.")};;
  *workslow*)
${line(delta("Working on it. "))}
sleep 4
${say("All done.")};;
  *)
${say("Fixture reply.")};;
esac
`);
  chmodSync(path, 0o700);
}

const assets = process.env.ZUSE_ASSETS ?? buildApp();
const engine = await startEngine({ assets, env: { HOME: join(scratch, "home") } });
writeAgent(join(engine.root, "bin", "claude"));
const page = await openBrowser(engine.url, { width: 1280, height: 820 });
// Outgoing rules as in defects.mjs; incoming `__tamper` rewrites the next transcript append frame.
const SOCKETS = `(() => {
  const Real = window.WebSocket;
  window.__ws = { sockets: [], rules: [], tamper: null, tampered: 0 };
  window.WebSocket = class extends Real {
    constructor(url, protocols) { super(url, protocols); window.__ws.sockets.push(this); }
    set onmessage(handler) {
      super.onmessage = (event) => {
        const t = window.__ws.tamper;
        if (t && typeof event.data === "string" && event.data.includes('"append"')) {
          const frame = JSON.parse(event.data);
          const update = frame.item;
          if (update && Array.isArray(update.append) && update.append.length) {
            if (t === "len") update.append[0].len += 1;
            else update.count += 1;
            window.__ws.tamper = null;
            window.__ws.tampered++;
            return handler.call(this, new MessageEvent("message", { data: JSON.stringify(frame) }));
          }
        }
        return handler.call(this, event);
      };
    }
    get onmessage() { return super.onmessage; }
    send(data) {
      const text = typeof data === "string" ? data : "";
      const rule = window.__ws.rules.find((r) => text.includes(r.match));
      if (!rule) return super.send(data);
      window.__ws.rules = window.__ws.rules.filter((r) => r !== rule);
      if (rule.action === "drop") return this.close();
      if (rule.action === "sendThenClose") { super.send(data); return setTimeout(() => this.close(), 50); }
      setTimeout(() => { try { super.send(data); } catch {} }, rule.ms);
    }
  };
})();`;
await page.call("Page.addScriptToEvaluateOnNewDocument", { source: SOCKETS });
await page.call("Page.reload", { ignoreCache: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const q = JSON.stringify;
const E = (expression) => page.eval(expression);
const results = [];
const shot = async (name) => shots && (await page.screenshot(join(shots, `review-${name}.png`)));
const expect = (value, what) => {
  if (!value) throw new Error(what);
};
const input = `document.querySelector(".cmp-input")`;
const status = (id) => `window.__zuse.state.sessions[${q(id)}]?.status`;
const connected = `window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`;
const config = { harness: "claude-code", model: null, reasoning: null, sandbox: "workspace-write" };
let deviceId;
const newChat = async (chatId, title) => {
  await engine.rpc("Mutate", { op: "createChat", chatId, spaceId: "demo-space", config });
  if (title) await engine.rpc("Mutate", { op: "renameChat", chatId, title });
  await page.waitFor(`!!document.querySelector('[data-chat-id=${q(chatId)}] .sb-row-open')`);
};
const openChat = async (chatId) => {
  await page.waitFor(`!!document.querySelector('[data-chat-id=${q(chatId)}] .sb-row-open')`);
  await page.click(`[data-chat-id=${q(chatId)}] .sb-row-open`);
  await page.waitFor(`window.__zuse.view.selectedChatId === ${q(chatId)}`);
};
const run = (chatId, prompt, model = null) =>
  engine.rpc("QueueCommand", {
    chatId,
    targetDeviceId: deviceId,
    command: { kind: "run", messageId: crypto.randomUUID(), request: { prompt, harness: "claude-code", model, cwd: project, sandbox: "workspace-write", plan: false, autoApprove: false, resume: null, attachments: [] } },
  });
const typeDraft = async (text) => {
  await E(`(() => { const t = ${input}; t.focus(); t.select(); return true })()`);
  await page.key("Backspace", "Backspace", 8);
  if (text) await page.type(text);
};
const ctrl = 2;
const shift = 8;

async function check(name, fn) {
  if (only.length && !only.includes(name)) return;
  process.stdout.write(`- ${name} … `);
  try {
    await fn();
    console.log("ok");
    results.push([name, "ok"]);
  } catch (error) {
    console.log(`FAILED: ${error.message}`);
    results.push([name, error.message]);
    await shot(`${name}-failed`);
  }
}

try {
  await page.waitFor(`typeof window.__ws === "object" && ${connected}`, 30000);
  deviceId = await E(`window.__zuse.state.localDeviceId`);
  await engine.rpc("Mutate", { op: "createSpace", spaceId: "demo-space", deviceId, path: project, name: "demo", gitDetected: true });

  for (const kind of ["len", "count"]) {
    await check(`desync-${kind}`, async () => {
      const chat = `chat-desync-${kind}`;
      await newChat(chat, `Desync ${kind}`);
      await openChat(chat);
      await run(chat, "streamwords please");
      await page.waitFor(`document.querySelector(".t-scroller")?.textContent.includes("alpha10 ")`, 20000);
      await E(`window.__ws.tamper = ${q(kind)}`);
      await page.waitFor(`window.__ws.tampered > 0`, 5000);
      await page.waitFor(`${status(chat)} !== "working"`, 30000);
      await page.waitFor(`window.__zuse.state.transcript?.ready`, 5000);
      const reply = `document.querySelector('.t-entry[data-role="assistant"] .md')?.textContent ?? ""`;
      await page.waitFor(`${reply}.includes("alpha149")`, 5000).catch(async () => {
        const shown = (await E(reply)).trim().split(/\s+/).length;
        throw new Error(`reply froze at ${shown} words after a ${kind} desync`);
      });
      const store = await E(`window.__zuse.state.transcript.messages.at(-1).parts.filter((p) => p.kind === "text").map((p) => p.text).join("")`);
      expect(store.trim().split(/\s+/).length === 150, "store has every word");
    });
  }

  await check("queue-edit-reload", async () => {
    await newChat("chat-q", "Queue");
    await openChat("chat-q");
    await run("chat-q", "please reply slowly");
    await page.waitFor(`${status("chat-q")} === "working"`, 15000);
    await engine.rpc("QueueMessage", { chatId: "chat-q", targetDeviceId: deviceId, text: "queued one", attachments: [], holdForTurnEnd: true });
    const row = `[...document.querySelectorAll(".cmp-queue-row")].find((r) => r.innerText.includes("queued one"))`;
    await page.waitFor(`!!${row}`, 8000);
    await typeDraft("my precious draft");
    await sleep(400);
    await E(`${row}.querySelector('[aria-label="Edit"]').click()`);
    await page.waitFor(`${input}.value === "queued one" && !!document.querySelector('[aria-label="Save to queue"]')`, 8000);
    await E(`(() => { const t = ${input}; t.focus(); t.setSelectionRange(t.value.length, t.value.length); return true })()`);
    await page.type(", edited");
    await sleep(600);
    const stored = JSON.parse((await E(`localStorage.getItem("zuse.web.composer.drafts.v1")`)) ?? "{}");
    expect(stored["chat-q"]?.text === "my precious draft", `stored draft during the edit: ${q(stored["chat-q"])}`);
    await page.call("Page.reload", { ignoreCache: false });
    await page.waitFor(connected, 20000);
    await openChat("chat-q");
    await page.waitFor(`${input}?.value === "my precious draft"`, 5000).catch(async () => {
      throw new Error(`after reload the composer shows ${q(await E(`${input}?.value`))}`);
    });
    expect(!(await E(`!!document.querySelector('[aria-label="Save to queue"]')`)), "still in edit mode after reload");
    // The old lease was handed back, so the row is editable again here.
    await page.waitFor(`!${row}?.innerText.includes("Editing")`, 8000).catch(async () => {
      throw new Error(`row still reads ${q(await E(`${row}?.innerText`))}`);
    });
    await E(`${row}.querySelector('[aria-label="Edit"]').click()`);
    await page.waitFor(`${input}.value === "queued one" && !!document.querySelector('[aria-label="Save to queue"]:not([disabled])')`, 8000);
    await E(`document.querySelector('.cmp-queue-row [aria-label="Cancel"]').click()`);
    await page.waitFor(`${input}.value === "my precious draft"`, 8000);
    await typeDraft("");
    await engine.rpc("QueueCommand", { chatId: "chat-q", targetDeviceId: deviceId, command: { kind: "interrupt" } }).catch(() => {});
  });

  await check("new-chat-unknown", async () => {
    await page.click(`button[aria-label="New chat"]`);
    await page.waitFor(`window.__zuse.view.selectedChatId === null`, 4000);
    await sleep(300);
    await typeDraft("please do the thing");
    const before = await E(`Object.keys(window.__zuse.state.chats).length`);
    const epoch = await E(`window.__zuse.state.epoch`);
    await E(`window.__ws.rules.push({ match: '"QueueCommand"', action: "drop" })`);
    await page.key("Enter", "Enter", 13);
    await page.waitFor(`window.__zuse.state.epoch > ${epoch} && ${connected}`, 20000);
    await sleep(800);
    const chatId = await E(`window.__zuse.view.selectedChatId`);
    expect(chatId && (await E(`Object.keys(window.__zuse.state.chats).length`)) === before + 1, `selected ${chatId}`);
    expect((await E(`${input}.value`)) === "please do the thing", `composer shows ${q(await E(`${input}.value`))}`);
    const tray = await E(`document.querySelector(".cmp-tray, .cmp-failure, [role=alert]")?.innerText ?? document.body.innerText`);
    expect(/may have/i.test(tray), `no 'may have completed' notice beside the text: ${q(tray.slice(0, 200))}`);
    await shot("new-chat-unknown");
    await typeDraft("");
  });

  await check("agent-switch", async () => {
    await newChat("chat-switch", "Switch");
    await openChat("chat-switch");
    await run("chat-switch", "hello", "claude-fable-5-1");
    await page.waitFor(`${status("chat-switch")} !== "working" && document.querySelector(".t-scroller")?.textContent.includes("Fixture reply.")`, 15000);
    await run("chat-switch", "hello again", "claude-fable-5");
    await page.waitFor(`!!document.querySelector(".t-agent-switch")`, 15000);
    const pieces = await E(`[...document.querySelector(".t-agent-switch").children].map((c) => ({ t: c.textContent, l: c.getBoundingClientRect().left, r: c.getBoundingClientRect().right }))`);
    const text = pieces.map((p) => p.t.trim()).filter(Boolean).join(" ");
    expect(text === "Model switched from Fable 5.1 to Fable 5 · Context handed off.", `notice reads ${q(text)}`);
    for (let i = 1; i < pieces.length; i++) expect(pieces[i].l - pieces[i - 1].r >= 4.5, `no gap before ${q(pieces[i].t)}`);
    await shot("agent-switch");
  });

  await check("find-in-chat", async () => {
    await newChat("chat-find", "Find");
    await openChat("chat-find");
    for (let n = 1; n <= 9; n++) {
      await run("chat-find", `message number ${n} zebra${n}`);
      await page.waitFor(`${status("chat-find")} !== "working" && window.__zuse.state.transcript.messages.length >= ${n * 2}`, 15000);
    }
    await sleep(500);
    await E(`${input}.focus()`);
    const inPage = await E(`document.querySelector(".t-root").innerText.includes("zebra1 ")`);
    await page.key("f", "KeyF", 70, ctrl);
    await page.waitFor(`document.activeElement?.classList.contains("t-find-input")`, 3000).catch(() => {
      throw new Error("Ctrl+F opened no find bar");
    });
    await page.type("zebra1");
    await page.waitFor(`document.querySelector(".t-find-count")?.textContent === "1 of 1"`, 3000).catch(async () => {
      throw new Error(`count reads ${q(await E(`document.querySelector(".t-find-count")?.textContent`))}`);
    });
    await page.waitFor(`CSS.highlights.get("t-find-active")?.size === 1`, 4000).catch(() => {
      throw new Error("the match was not highlighted");
    });
    const visible = await E(`(() => { const r = [...CSS.highlights.get("t-find-active")][0].getBoundingClientRect(); const s = document.querySelector(".t-scroller").getBoundingClientRect(); return r.top >= s.top && r.bottom <= s.bottom && r.width > 0 })()`);
    expect(visible, "the match is not scrolled into view");
    await shot("find-in-chat");
    // "zebra" hits every prompt; Enter steps and wraps.
    await typeDraftIn(".t-find-input", "zebra");
    await page.waitFor(`/^\\d+ of 9$/.test(document.querySelector(".t-find-count")?.textContent)`, 3000);
    const first = await E(`document.querySelector(".t-find-count").textContent`);
    await page.key("Enter", "Enter", 13);
    const second = await E(`document.querySelector(".t-find-count").textContent`);
    expect(first !== second, `Enter did not step (${first})`);
    await page.key("Enter", "Enter", 13, shift);
    expect((await E(`document.querySelector(".t-find-count").textContent`)) === first, "Shift+Enter did not step back");
    await page.key("Escape", "Escape", 27);
    await page.waitFor(`!document.querySelector(".t-find")`, 2000);
    expect(await E(`document.activeElement === ${input}`), "focus did not return to the composer");
    expect(!(await E(`CSS.highlights.has("t-find")`)), "highlights left behind");
    results.push(["find-in-chat: 'zebra1' in the page before (virtualized)", String(inPage)]);
  });

  async function typeDraftIn(selector, text) {
    await E(`(() => { const t = document.querySelector(${q(selector)}); t.focus(); t.select(); return true })()`);
    await page.key("Backspace", "Backspace", 8);
    await page.type(text);
  }

  await check("permission-focus", async () => {
    await newChat("chat-perm", "Approvals");
    await openChat("chat-perm");
    // Typing elsewhere (the sidebar search) keeps focus when a request arrives.
    await E(`document.querySelector('button[aria-label="Search"], button[aria-label="Search chats"]')?.click()`);
    await sleep(200);
    const elsewhere = await E(`(() => { const s = document.querySelector(".sb-search input") ?? document.querySelector(".t-find-input"); if (!s) return null; s.focus(); return s.className || s.tagName })()`);
    if (!elsewhere) {
      await page.key("f", "KeyF", 70, ctrl);
      await page.waitFor(`document.activeElement?.classList.contains("t-find-input")`, 3000);
    }
    const holder = await E(`document.activeElement.className`);
    await run("chat-perm", "Clean up, permission needed.");
    await page.waitFor(`!!document.querySelector(".t-permission")`, 15000);
    await sleep(300);
    expect((await E(`document.activeElement.className`)) === holder, `focus jumped to ${await E(`document.activeElement.className`)}`);
    // From the composer it moves to the card; answering hands it back.
    await E(`document.querySelector('.t-permission button[aria-label="Deny this action"]').click()`);
    await page.waitFor(`!document.querySelector(".t-permission")`, 15000);
    await page.waitFor(`${status("chat-perm")} !== "working"`, 15000);
    await E(`${input}.focus()`);
    await run("chat-perm", "Clean up, permission needed.");
    await page.waitFor(`!!document.querySelector(".t-permission")`, 15000);
    await page.waitFor(`document.activeElement?.classList.contains("t-permission")`, 3000).catch(async () => {
      throw new Error(`card did not take focus from the composer: ${await E(`document.activeElement.className`)}`);
    });
    await page.key("Enter", "Enter", 13, ctrl);
    await page.waitFor(`!document.querySelector(".t-permission")`, 5000);
    await sleep(100);
    const after = await E(`document.activeElement?.className ?? ""`);
    expect(after.includes("cmp-input"), `focus after answering is on ${q(after)} (${await E(`document.activeElement.tagName`)})`);
  });

  await check("live-region", async () => {
    await newChat("chat-live", "Live");
    await openChat("chat-live");
    await run("chat-live", "workslow please");
    await page.waitFor(`!!document.querySelector(".t-working-timer")`, 15000);
    const facts = await E(`({
      trailerRole: document.querySelector(".t-working").getAttribute("role"),
      trailerLive: document.querySelector(".t-working").getAttribute("aria-live"),
      timerHidden: document.querySelector(".t-working-timer").getAttribute("aria-hidden"),
      scrollerRole: document.querySelector(".t-scroller").getAttribute("role"),
      status: document.querySelector(".t-sr[role=status]")?.textContent,
    })`);
    expect(!facts.trailerRole && !facts.trailerLive, `trailer is live: ${q(facts)}`);
    expect(facts.timerHidden === "true", "timer is exposed");
    expect(facts.scrollerRole !== "log", "scroller is still a log");
    expect(/is working$/.test(facts.status ?? ""), `status reads ${q(facts.status)}`);
    await page.waitFor(`${status("chat-live")} !== "working"`, 15000);
    const done = await E(`document.querySelector(".t-sr[role=status]")?.textContent`);
    expect(/finished$/.test(done ?? ""), `status after the turn reads ${q(done)}`);
  });

  await check("sidebar-trail", async () => {
    await newChat("chat-long", "Please read the project and summarise every file in it");
    await sleep(300);
    // Stand in for "1m ago" so the check does not wait a minute; the layout must hold for any label.
    for (const label of ["1m ago", "59m ago"]) {
      const m = await E(`(() => {
        const row = document.querySelector('[data-chat-id="chat-long"]');
        const time = row.querySelector(".sb-row-time");
        time.textContent = ${q(label)};
        const title = row.querySelector(".sb-row-title").getBoundingClientRect();
        const t = time.getBoundingClientRect();
        return { titleRight: title.right, timeLeft: t.left, size: getComputedStyle(time).fontSize };
      })()`);
      expect(m.titleRight <= m.timeLeft, `title ends at ${m.titleRight}, ${label} starts at ${m.timeLeft}`);
      results.push([`sidebar-trail: ${label} gap px, font`, `${(m.timeLeft - m.titleRight).toFixed(1)}, ${m.size}`]);
    }
    await shot("sidebar-trail");
  });

  await check("sidebar-focus", async () => {
    await openChat("chat-long");
    // Keyboard modality, so :focus-visible applies.
    await E(`document.querySelector('[data-chat-id="chat-long"] .sb-row-open').blur()`);
    await page.key("Tab", "Tab", 9);
    await E(`document.querySelector('[data-chat-id="chat-long"] .sb-row-open').focus()`);
    const ring = await E(`(() => { const s = getComputedStyle(document.querySelector('[data-chat-id="chat-long"] .sb-row-open'), "::after"); return { w: s.outlineWidth, c: s.outlineColor, style: s.outlineStyle } })()`);
    expect(ring.w === "2px" && ring.style === "solid" && !/rgb\(50, 50, 50\)/.test(ring.c), `selected row ring ${q(ring)}`);
    await E(`document.querySelector('button[aria-label="New chat"]').focus()`);
    const plain = await E(`(() => { const s = getComputedStyle(document.querySelector('button[aria-label="New chat"]')); return { w: s.outlineWidth, c: s.outlineColor } })()`);
    expect(plain.w === "2px", `New chat ring ${q(plain)}`);
    results.push(["sidebar-focus: ring colour", ring.c]);
    await shot("sidebar-focus");
  });

  await check("visuals", async () => {
    for (const theme of ["light", "dark"]) {
      await E(`document.documentElement.dataset.theme = ${q(theme)}`);
      await openChat("chat-switch");
      await sleep(300);
      await shot(`${theme}-agent-switch`);
      await openChat("chat-find");
      await E(`${input}.focus()`);
      await page.key("f", "KeyF", 70, ctrl);
      await page.waitFor(`document.activeElement?.classList.contains("t-find-input")`, 3000);
      await typeDraftIn(".t-find-input", "zebra");
      await sleep(400);
      await shot(`${theme}-find`);
      await page.key("Escape", "Escape", 27);
      // Keyboard focus on the selected row.
      await page.key("Tab", "Tab", 9);
      await E(`document.querySelector('[data-chat-id="chat-find"] .sb-row-open').focus()`);
      await sleep(100);
      await shot(`${theme}-row-focus`);
      const contrast = await E(`(() => {
        const time = document.querySelector('[data-chat-id="chat-long"] .sb-row-time');
        const probe = document.createElement("div");
        probe.style.background = "var(--shell)";
        document.querySelector(".sb").appendChild(probe);
        const bgCss = getComputedStyle(probe).backgroundColor;
        probe.remove();
        const ctx = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
        ctx.fillStyle = bgCss; ctx.fillRect(0, 0, 1, 1);
        const bg = [...ctx.getImageData(0, 0, 1, 1).data];
        ctx.fillStyle = getComputedStyle(time).color; ctx.fillRect(0, 0, 1, 1);
        const fg = [...ctx.getImageData(0, 0, 1, 1).data];
        const lum = (c) => { const [r, g, b] = c.slice(0, 3).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
        const L1 = lum(fg), L2 = lum(bg);
        return ((Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05)).toFixed(2) + ":1 on " + bgCss;
      })()`);
      results.push([`visuals: ${theme} row age contrast`, contrast]);
    }
    await page.call("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
    await sleep(400);
    await shot("phone-agent-switch");
    await E(`${input}.focus()`);
    await page.key("f", "KeyF", 70, ctrl);
    await sleep(300);
    await shot("phone-find");
    const fits = await E(`(() => { const r = document.querySelector(".t-find")?.getBoundingClientRect(); return r ? r.left >= 0 && r.right <= innerWidth : false })()`);
    await page.key("Escape", "Escape", 27);
    await page.call("Emulation.setDeviceMetricsOverride", { width: 1280, height: 820, deviceScaleFactor: 1, mobile: false });
    expect(fits, "find bar overflows a phone screen");
  });
} finally {
  console.log(JSON.stringify(results, null, 1));
  if (page.errors.length) console.log("page errors:", page.errors.join("\n"));
  await page.close();
  await engine.close();
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
  rmSync(scratch, { recursive: true, force: true });
}
const failed = results.filter(([name, outcome]) => !name.includes(":") && outcome !== "ok");
if (failed.length) process.exit(1);
