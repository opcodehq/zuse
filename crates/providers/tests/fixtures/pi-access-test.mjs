import assert from "node:assert/strict";
const {default: extension} = await import(process.argv[2]);
function guard(mode) {
  process.env.ZUSE_PI_ACCESS = JSON.stringify({mode, ready: "zuse-access-test"});
  const handlers = {}, commands = {};
  extension({on: (name, handler) => handlers[name] = handler,
    registerCommand: (name, command) => commands[name] = command});
  assert.equal(typeof commands["zuse-access-test"].handler, "function");
  return handlers;
}
for (const mode of ["ask", "full", "read-only"]) {
  const handlers = guard(mode);
  let questions = 0;
  const ctx = {ui: {confirm: async () => {questions++; return true;}}};
  for (const toolName of ["read", "grep", "find", "ls"]) {
    assert.equal(await handlers.tool_call({toolName, input: {}}, ctx), undefined);
  }
  assert.equal(questions, 0);
  for (const toolName of ["bash", "write", "edit", "zuse_create_chat", "zuse_use_tools", "unknown"]) {
    const result = await handlers.tool_call({toolName, input: {marker: toolName}}, ctx);
    assert.equal(result?.block === true, mode === "read-only");
  }
  assert.equal(questions, mode === "ask" ? 6 : 0);
  const shell = await handlers.user_bash({command: "touch denied", cwd: "/"}, ctx);
  assert.equal(shell?.result?.exitCode === 1, mode === "read-only");
}
const handlers = guard("ask");
for (const answer of [false, undefined, "true", "Yes", 1]) {
  const result = await handlers.tool_call({toolName: "write", input: {}}, {ui: {confirm: async () => answer}});
  assert.equal(result.block, true);
}
for (const method of ["tool_call", "user_bash"]) {
  const event = {toolName: "bash", input: {command: "false"}, command: "false", cwd: "/"};
  const failed = await handlers[method](event, {ui: {confirm: async () => {throw Error("disconnected");}}});
  assert.ok(failed.block || failed.result.exitCode === 1);
  const controller = new AbortController();
  controller.abort();
  let asked = false;
  const cancelled = await handlers[method](event, {signal: controller.signal, ui: {confirm: async () => {asked = true; return true;}}});
  assert.ok(cancelled.block || cancelled.result.exitCode === 1);
  assert.equal(asked, false);
}
// Concurrent siblings receive independent decisions; cancelling after a response
// is delivered still prevents execution from starting.
const requests = [];
const ctx = {ui: {confirm: (_title, message) => new Promise(resolve => requests.push({message, resolve}))}};
const first = handlers.tool_call({toolName: "write", input: {path: "first"}}, ctx);
const second = handlers.tool_call({toolName: "bash", input: {command: "second"}}, ctx);
assert.match(requests[0].message, /first/);
assert.match(requests[1].message, /second/);
requests[1].resolve(false); requests[0].resolve(true);
assert.equal(await first, undefined);
assert.equal((await second).block, true);
const controller = new AbortController();
assert.equal((await handlers.tool_call({toolName: "write", input: {}}, {signal: controller.signal,
  ui: {confirm: async () => {controller.abort(); return true;}}})).block, true);
console.log("Pi access event contract passed");
