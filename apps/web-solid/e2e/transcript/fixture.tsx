// Test-only page: mounts the real Transcript on the real store and drives it with synthetic engine frames
// through the same applyTranscriptFrame path the WebSocket uses. Never bundled into the app.
import { render } from "solid-js/web";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/shell/shell.css";
import "@/core/chat";
import { setState, state } from "@/core/store";
import { setView } from "@/core/view";
import { applyTranscriptFrame } from "@/core/transcript";
import type { MessagePart, SessionMessageEntry, TranscriptFrame } from "@/core/protocol";
import type { SetStoreFunction } from "solid-js/store";
import { Transcript } from "@/features/transcript";

const CHAT = "fixture-chat";
const DEVICE = "fixture-device";
document.documentElement.dataset.theme = new URLSearchParams(location.search).get("theme") ?? "dark";

setState({
  localDeviceId: DEVICE,
  connection: { kind: "connected" },
  chats: { [CHAT]: { id: CHAT, deviceId: DEVICE, title: "Fixture chat", archived: false, cwd: "/tmp/fixture", createdAt: new Date().toISOString(), config: { harness: "claude-code", sandbox: "workspace-write" } } },
  harnesses: [{ id: "claude-code", name: "Fixture Agent" }],
});
// Not connected to an engine, so core/chat leaves the transcript to this page.
setState("connection", { kind: "connecting" });
setView({ selectedChatId: CHAT, route: { kind: "chat" } });
setState("transcript", { chatId: CHAT, ready: true, messages: [], contextUsage: null, notice: null });

const setMessages = ((...args: unknown[]) => (setState as (...a: unknown[]) => void)("transcript", "messages", ...args)) as SetStoreFunction<SessionMessageEntry[]>;
const apply = (frame: TranscriptFrame) => applyTranscriptFrame(setMessages, state.transcript!.messages, frame);

let seq = 0;
const id = (p: string) => `${p}-${++seq}`;
const WORDS = "the engine streams tokens into one part so a long reply never re-renders the whole transcript while the user reads history above it".split(" ");
const sentence = (n: number) => Array.from({ length: n }, (_, i) => WORDS[(i * 7 + n) % WORDS.length]).join(" ");

function assistantParts(i: number): MessagePart[] {
  const parts: MessagePart[] = [{ kind: "reasoning", id: id("r"), text: `Considering step ${i}. ${sentence(30)}.` }];
  parts.push({ kind: "tool", id: id("t"), call: { kind: "exec", command: `cargo test -p crate_${i % 9}` } as never, resolved: true, output: Array.from({ length: 12 }, (_, l) => `test case_${l} ... ok`).join("\n") });
  parts.push({ kind: "tool", id: id("t"), call: { kind: "readFile", path: `src/module_${i % 13}.rs` } as never, resolved: true, output: "fn main() {}\n" });
  if (i % 3 === 0)
    parts.push({ kind: "tool", id: id("t"), call: { kind: "editFile", path: `src/lib_${i % 5}.rs` } as never, resolved: true, diff: { path: `src/lib_${i % 5}.rs`, oldText: "fn a() {}\nfn b() {}\n", newText: "fn a() {}\nfn b() { todo!() }\nfn c() {}\n" } });
  let text = `## Result ${i}\n\n${sentence(40)}.\n\n- ${sentence(8)}\n- \`inline_code_${i}\` and [a link](https://example.com/${i})\n`;
  if (i % 4 === 0) text += `\n| Name | Value | Notes |\n| --- | --- | --- |\n| alpha | ${i} | ${sentence(5)} |\n| beta | ${i * 2} | ${sentence(4)} |\n`;
  if (i % 5 === 0) text += `\n\`\`\`rust\nfn handler_${i}(input: &str) -> Result<String, Error> {\n    let parsed = input.trim().parse::<u64>()?;\n    Ok(format!("{parsed}"))\n}\n\`\`\`\n`;
  parts.push({ kind: "text", id: id("x"), text });
  return parts;
}

function seed(n: number) {
  const now = Date.now() - n * 60_000;
  const reset: SessionMessageEntry[] = [];
  for (let i = 0; i < n; i++) {
    const user = i % 2 === 0;
    reset.push({
      id: id(user ? "u" : "a"),
      role: user ? "user" : "assistant",
      createdAt: now + i * 60_000,
      deviceId: DEVICE,
      status: "complete",
      durationMs: user ? undefined : 12_000 + i,
      parts: user ? [{ kind: "text", id: id("x"), text: `Prompt ${i}: ${sentence(12 + (i % 20))}` }] : assistantParts(i),
    });
  }
  apply({ reset });
  return state.transcript!.messages.length;
}

const last = () => state.transcript!.messages[state.transcript!.messages.length - 1]!.id;
const count = () => state.transcript!.messages.length;

function upsert(entry: SessionMessageEntry) {
  apply({ upsert: [{ after: last(), entry }], count: count() + 1 });
}

/** Stream `tokens` tokens into a fresh assistant entry, `perFrame` engine frames per macrotask, measuring frames. */
async function stream(opts: { tokens: number; batch: number; intervalMs: number }) {
  const entryId = id("a");
  const partId = id("x");
  upsert({ id: entryId, role: "assistant", createdAt: Date.now(), deviceId: DEVICE, status: "streaming", parts: [{ kind: "text", id: partId, text: "" }] });
  setState("sessions", CHAT, { chatId: CHAT, status: "working", startedAt: new Date().toISOString() });
  let len = 0;
  let sent = 0;
  const applyTimes: number[] = [];
  const frames: number[] = [];
  let lastFrame = performance.now();
  let running = true;
  const tick = (t: number) => {
    frames.push(t - lastFrame);
    lastFrame = t;
    if (running) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  const longTasks: number[] = [];
  const observer = new PerformanceObserver((list) => list.getEntries().forEach((e) => longTasks.push(e.duration)));
  observer.observe({ type: "longtask", buffered: false });
  const started = performance.now();
  let fence = false;
  while (sent < opts.tokens) {
    for (let b = 0; b < opts.batch && sent < opts.tokens; b++, sent++) {
      let token = `${WORDS[sent % WORDS.length]} `;
      if (sent % 60 === 59) token += "\n\n";
      if (sent % 400 === 200) { token += fence ? "\n```\n\n" : "\n\n```ts\nconst value = compute();\n"; fence = !fence; }
      if (sent % 500 === 499) token += "\n| a | b |\n| - | - |\n| 1 | 2 |\n\n";
      len += token.length;
      const t0 = performance.now();
      apply({ append: [{ entry: entryId, part: partId, text: token, len }], count: count() });
      applyTimes.push(performance.now() - t0);
    }
    await new Promise((r) => setTimeout(r, opts.intervalMs));
  }
  const elapsed = performance.now() - started;
  apply({ upsert: [{ after: state.transcript!.messages[count() - 2]!.id, entry: { ...JSON.parse(JSON.stringify(state.transcript!.messages[count() - 1])), status: "complete" } }], count: count() });
  setState("sessions", CHAT, { chatId: CHAT, status: "idle" });
  await new Promise((r) => setTimeout(r, 300));
  running = false;
  observer.disconnect();
  return { tokens: sent, elapsedMs: Math.round(elapsed), tokensPerSec: Math.round((sent / elapsed) * 1000), frames: summarize(frames.slice(2)), apply: summarize(applyTimes), longTasks: longTasks.length, longestTask: Math.round(Math.max(0, ...longTasks)), domNodes: document.querySelectorAll("*").length };
}

function summarize(values: number[]) {
  const sorted = [...values].sort((a, b) => a - b);
  const at = (p: number) => Math.round((sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0) * 100) / 100;
  return { n: values.length, p50: at(0.5), p95: at(0.95), p99: at(0.99), max: at(1), over50: values.filter((v) => v > 50).length };
}

/** Scroll the transcript from bottom to top in fixed steps, one per frame, and report frame times. */
async function scrollTest(step = 600) {
  const scroller = document.querySelector<HTMLElement>(".t-scroller")!;
  const frames: number[] = [];
  let lastFrame = performance.now();
  scroller.dispatchEvent(new WheelEvent("wheel", { deltaY: -1 }));
  while (scroller.scrollTop > 0) {
    scroller.dispatchEvent(new WheelEvent("wheel", { deltaY: -step }));
    scroller.scrollTop -= step;
    await new Promise((r) => requestAnimationFrame(r));
    const t = performance.now();
    frames.push(t - lastFrame);
    lastFrame = t;
  }
  return { steps: frames.length, frames: summarize(frames.slice(1)), rendered: document.querySelectorAll(".t-row").length };
}

function inject(entry: Partial<SessionMessageEntry> & { parts: MessagePart[] }) {
  upsert({ id: id(entry.role === "user" ? "u" : "a"), role: "assistant", createdAt: Date.now(), deviceId: DEVICE, status: "complete", ...entry });
}

/** One of every part kind, ending with a live turn and an open question. */
function kinds() {
  const long = Array.from({ length: 9 }, (_, i) => `Line ${i + 1}: ${sentence(10)}`).join("\n");
  inject({
    role: "user",
    parts: [
      { kind: "agentContext", id: id("c"), agent: { harness: "claude-code", model: "fixture-model-b" }, previousAgent: { harness: "claude-code", model: "fixture-model-a" } },
      { kind: "text", id: id("x"), text: `${long}\n\nAttached images (local files — open them to view):\n- /tmp/fixture/uploads/0a1b2c3d-screen.png\n- /tmp/fixture/uploads/notes.txt` },
    ],
  });
  inject({
    parts: [
      { kind: "reasoning", id: id("r"), text: "Checking the failing test first. Then the fix." },
      { kind: "tool", id: id("t"), call: { kind: "exec", command: "npm test -- --run parser" } as never, resolved: true, isError: true, output: "FAIL parser.test.ts\n  expected 2, received 3" },
      { kind: "tool", id: id("t"), call: { kind: "search", pattern: "parseHeader", path: "src" } as never, resolved: true, output: "src/parser.ts:12" },
      { kind: "text", id: id("x"), text: "The parser double counts headers. I fixed it and the suite passes." },
      { kind: "error", id: id("e"), message: "The agent stopped: rate limit reached. Try again in a minute." },
      { kind: "uiBlock", id: id("b"), spec: '{"type":"text","text":"hello"}' },
    ],
  });
  inject({ parts: [{ kind: "fork", id: id("f"), sourceChatId: "elsewhere", sourceTitle: "Parser cleanup" }] });
  const live = id("a");
  inject({
    id: live,
    status: "streaming",
    parts: [
      { kind: "reasoning", id: id("r"), text: "Planning the migration. I will update the schema, then the readers." },
      { kind: "tool", id: id("t"), call: { kind: "todo", items: [{ text: "Update schema", done: true, status: "completed" }, { text: "Migrate readers", done: false, status: "inProgress" }, { text: "Remove old path", done: false, status: "pending" }] } as never, resolved: true },
      { kind: "tool", id: id("t"), call: { kind: "editFile", path: "src/schema.ts" } as never, resolved: true, diff: { path: "src/schema.ts", oldText: "export const version = 1;\n", newText: "export const version = 2;\nexport const migrated = true;\n" } },
      { kind: "tool", id: id("t"), call: { kind: "readFile", path: "src/readers/index.ts" } as never, resolved: false },
      { kind: "tool", id: id("t"), call: { kind: "unknown", name: "Agent: scan readers", input: { model: "fixture-model-a" } } as never, resolved: false, subagentStatus: "running" },
      { kind: "tool", id: id("t"), call: { kind: "unknown", name: "Agent: check callers" } as never, resolved: true, subagentStatus: "done" },
      { kind: "input", id: id("i"), requestId: "req-1", questions: [{ id: "q1", header: "Migration", question: "Which readers should move first?", options: ["Legacy CSV", "JSON API", "Both"], multiSelect: false }] },
    ],
  });
  setState("sessions", CHAT, { chatId: CHAT, status: "awaitingInput", startedAt: new Date(Date.now() - 95_000).toISOString() });
}

function resolve(requestId: string) {
  const messages = state.transcript!.messages;
  for (const message of messages)
    for (const part of message.parts)
      if (part.kind === "input" && part.requestId === requestId) {
        const entry = JSON.parse(JSON.stringify(message)) as SessionMessageEntry;
        for (const p of entry.parts) if (p.kind === "input" && p.requestId === requestId) p.resolved = true;
        const at = messages.indexOf(message);
        apply({ upsert: [{ after: at === 0 ? null : messages[at - 1]!.id, entry }], count: count() });
        return true;
      }
  return false;
}

Object.assign(window, { __fixture: { seed, stream, scrollTest, inject, resolve, kinds, setState, state, apply, upsert, count } });
render(() => (
  <div class="app"><div class="app-main"><main class="app-pane"><div class="app-chat"><Transcript /></div></main></div></div>
), document.getElementById("root")!);
