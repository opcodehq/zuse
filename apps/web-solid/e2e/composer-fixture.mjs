// A scripted agent CLI for composer checks. The prompt picks the behaviour; nothing leaves the machine.
//   "slowly"      replies after a pause, so the run stays live for queue, steer and stop checks
//   "usagelimit"  ends on a usage-limit error
//   "authfail"    ends on a sign-in error
//   "bigcontext"  reports 92% context occupancy
//   anything else replies at once with 20% context occupancy
// It also answers the command probe with two slash commands.
import { chmodSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const line = (event) => `printf '%s\\n' '${JSON.stringify(event).replace(/'/g, "'\\''")}'`;
const init = line({ type: "system", subtype: "init", session_id: "fixture", model: "fixture", tools: [] });
// Main-agent text only reaches the transcript as stream deltas; the assistant frame carries usage.
const text = (body, tokens) =>
  [
    ...body.match(/.{1,6}/gs).map((piece) => line({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: piece } } })),
    line({ type: "assistant", message: { role: "assistant", model: "fixture", content: [{ type: "text", text: body }], usage: { input_tokens: tokens, output_tokens: 10 } } }),
  ].join("; ");
const result = (body) =>
  line({ type: "result", subtype: "success", session_id: "fixture", is_error: false, result: body, usage: { input_tokens: 1, output_tokens: 1 }, modelUsage: { fixture: { contextWindow: 200000 } } });
const failure = (code) => line({ type: "assistant", error: code, message: { role: "assistant", model: "fixture", content: [] } });
const failed = line({ type: "result", subtype: "error_during_execution", session_id: "fixture", is_error: true, errors: [], usage: { input_tokens: 1, output_tokens: 1 } });

// The initialize answer, with the request id spliced in by the script.
const commands = JSON.stringify({
  type: "control_response",
  response: { subtype: "success", request_id: "@ID@", response: { commands: [
    { name: "review", description: "Review the current changes", argumentHint: "[path]" },
    { name: "compact", description: "Summarize the conversation so far" },
  ] } },
}).replace(/'/g, "'\\''");

export function writeComposerAgent(path) {
  const script = `#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
# Answer initialize requests (the command probe exits after its answer), then read the prompt.
while IFS= read -r prompt; do
  case "$prompt" in
    *'"subtype":"initialize"'*)
      id=$(printf '%s' "$prompt" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
      printf '%s\\n' '${commands}' | sed "s/@ID@/$id/"
      [ "$id" = "zuse-command-probe" ] && exit 0;;
    *) break;;
  esac
done
${init}
case "$prompt" in
  *slowly*) sleep 6; ${text("Slow reply done.", 30000)}; ${result("Slow reply done.")};;
  *usagelimit*) ${failure("rate_limit")}; ${failed};;
  *authfail*) ${failure("authentication_failed")}; ${failed};;
  *bigcontext*) ${text("Big context reply.", 184000)}; ${result("Big context reply.")};;
  *) ${text("Fixture reply.", 40000)}; ${result("Fixture reply.")};;
esac
`;
  writeFileSync(path, script);
  chmodSync(path, 0o700);
  return path;
}

export const agentPath = (root) => join(root, "bin", "claude");
