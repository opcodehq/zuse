// Per-run access enforcement through the public tool and shell event hooks.
export default function (pi) {
  const config = JSON.parse(process.env.ZUSE_PI_ACCESS);
  if (!["ask", "full", "read-only"].includes(config.mode) ||
      typeof config.ready !== "string" || !config.ready.startsWith("zuse-access-")) {
    throw new Error("Invalid Zuse access configuration");
  }
  const inspection = new Set(["read", "grep", "find", "ls"]);
  async function allowed(name, input, ctx) {
    if (ctx.signal?.aborted) return false;
    if (config.mode === "full") return true;
    if (inspection.has(name)) return true;
    if (config.mode === "read-only") return false;
    try {
      const confirmed = await ctx.ui.confirm(
        "Allow tool execution?",
        `${name}\n${JSON.stringify(input)}`,
        {signal: ctx.signal},
      );
      return confirmed === true && !ctx.signal?.aborted;
    } catch {
      return false;
    }
  }
  pi.on("tool_call", async (event, ctx) => {
    if (!await allowed(event.toolName, event.input, ctx)) {
      return {block: true, reason: "Zuse access policy declined this tool execution"};
    }
  });
  // Shell commands entered through ! / !! do not produce tool_call events.
  pi.on("user_bash", async (event, ctx) => {
    if (!await allowed("bash", {command: event.command, cwd: event.cwd}, ctx)) {
      return {result: {output: "Zuse access policy declined this command", exitCode: 1,
        cancelled: true, truncated: false}};
    }
  });
  // The adapter must see this per-process nonce before admitting a prompt.
  pi.registerCommand(config.ready, {description: "Zuse access guard readiness", handler: async () => {}});
}
