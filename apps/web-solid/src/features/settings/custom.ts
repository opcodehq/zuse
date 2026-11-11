// Custom ACP agents and the public agent catalog (crates/protocol/src/custom_acp.rs, crates/viewport/src/custom_acp.rs).
import { createStore } from "solid-js/store";
import { M } from "@/core/methods";
import { onConnected, refreshHarnesses, rpc } from "@/core/connection";
import { type Catalog, type CustomAcpAgent, type CustomAcpAgents, data, loadCustomAgents, loadModels, message, type RegistryAgent, setData, uncertain } from "./data";

export type Probe = { kind: "testing" } | { kind: "ready" } | { kind: "error"; message: string };
export type Offer = { kind: "builtIn"; harness: string } | { kind: "add"; agent: CustomAcpAgent } | { kind: "manual"; agent: CustomAcpAgent };

interface CustomState {
  busy: string | null;
  error: string | null;
  probes: Record<string, Probe | undefined>;
  catalog: Catalog | null;
  catalogLoading: boolean;
  catalogError: string | null;
  icons: Record<string, string | null | undefined>;
}

export const [custom, setCustom] = createStore<CustomState>({
  busy: null,
  error: null,
  probes: {},
  catalog: null,
  catalogLoading: false,
  catalogError: null,
  icons: {},
});

onConnected(() => setCustom({ busy: null, error: null, probes: {}, catalog: null, catalogError: null }));

const BUILT_IN: Record<string, string> = {
  "claude-acp": "claude-code",
  "codex-acp": "codex",
  cursor: "cursor",
  devin: "devin",
  "grok-build": "grok",
  opencode: "opencode",
  "pi-acp": "pi",
  "antigravity-acp": "antigravity",
};

/** The custom ACP identity a registry entry is added under. */
export function customId(registryId: string) {
  let slug = "";
  for (const raw of registryId) {
    const ch = raw.toLowerCase();
    if (/[a-z0-9]/.test(ch)) slug += ch;
    else if (slug && !slug.endsWith("-")) slug += "-";
  }
  slug = slug.replace(/-+$/, "");
  if (slug.endsWith("-acp")) slug = slug.slice(0, -4);
  slug = slug.slice(0, 96) || "agent";
  return `acp-${slug}`;
}

const ID = /^acp-[a-z0-9][a-z0-9-]{0,95}$/;
const RESERVED_ENV = new Set(["ZUSE_CHAT_ID", "ZUSE_DEVICE_ID", "ZUSE_IPC_PORT"]);

/** Mirrors CustomAcpAgent::validate so the form explains a refusal before saving. */
export function validate(agent: CustomAcpAgent): string | null {
  if (!ID.test(agent.id)) return "Custom ACP IDs need 1–96 lowercase letters, digits or hyphens, starting with a letter or digit";
  if (!agent.name.trim() || new TextEncoder().encode(agent.name).length > 256 || /[\0\n\r]/.test(agent.name)) return "Enter a provider name of up to 256 bytes";
  if (!agent.command.trim() || agent.command.length > 8192 || agent.command.includes("\0")) return "Enter an executable path or command name";
  if (agent.args.length > 128 || agent.args.some((a) => a.length > 8192 || a.includes("\0"))) return "ACP arguments exceed the launch limit";
  const env = Object.entries(agent.env);
  if (env.length > 128 || env.some(([k, v]) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(k) || k.length > 256 || RESERVED_ENV.has(k) || v.length > 65536 || v.includes("\0")))
    return "ACP environment is invalid or exceeds the launch limit";
  if (agent.modelIds.length > 128 || agent.modelIds.some((id) => !id.trim() || id.length > 512)) return "ACP model IDs are invalid or exceed the catalog limit";
  return null;
}

const SPACE = /[\t\n\v\f\r \u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]/;

/** Quoted command fields are tokenized as data; saving never expands a shell expression. */
export function words(line: string): string[] {
  const result: string[] = [];
  let current: string | null = null;
  let quote: "'" | '"' | null = null;
  const chars = [...line];
  for (let i = 0; i < chars.length; i++) {
    const ch = chars[i]!;
    if (quote === "'") {
      if (ch === "'") quote = null;
      else current = (current ?? "") + ch;
    } else if (ch === "\\" && i + 1 < chars.length) {
      current = (current ?? "") + chars[++i];
    } else if (quote === '"') {
      if (ch === '"') quote = null;
      else current = (current ?? "") + ch;
    } else if (ch === "'" || ch === '"') {
      quote = ch;
      current ??= "";
    } else if (SPACE.test(ch)) {
      if (current !== null) result.push(current);
      current = null;
    } else current = (current ?? "") + ch;
  }
  if (quote) throw new Error("Close the quoted value before saving.");
  if (current !== null) result.push(current);
  return result;
}

export const argumentsLine = (values: string[]) =>
  values.map((v) => (v && /^[A-Za-z0-9_@%+=:,./-]+$/.test(v) ? v : `'${v.replace(/'/g, "'\\''")}'`)).join(" ");

export interface AgentFields {
  id: string;
  name: string;
  command: string;
  args: string;
  env: string;
  models: string;
}

export const fieldsFor = (agent: CustomAcpAgent | null): AgentFields =>
  agent
    ? { id: agent.id, name: agent.name, command: agent.command, args: argumentsLine(agent.args), env: "", models: agent.modelIds.join("\n") }
    : { id: `acp-${crypto.randomUUID().replace(/-/g, "")}`, name: "", command: "", args: "", env: "", models: "" };

/** A blank environment field keeps the saved values unless they were cleared. */
export function definition(fields: AgentFields, preserved: Record<string, string> | null): CustomAcpAgent {
  const pairs = words(fields.env);
  let env: Record<string, string> = {};
  for (const pair of pairs) {
    const at = pair.indexOf("=");
    if (at < 0) throw new Error("Environment entries need NAME=value.");
    const key = pair.slice(0, at);
    if (key in env) throw new Error(`Duplicate environment variable: ${key}`);
    env[key] = pair.slice(at + 1);
  }
  if (!pairs.length) env = { ...(preserved ?? {}) };
  const agent: CustomAcpAgent = {
    id: fields.id.trim(),
    name: fields.name.trim(),
    command: fields.command.trim(),
    args: words(fields.args),
    env,
    modelIds: fields.models
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean),
  };
  const problem = validate(agent);
  if (problem) throw new Error(problem);
  return agent;
}

/** How the catalog offers one registry entry on the engine's platform. */
export function offer(entry: RegistryAgent, platform: string): Offer | null {
  const builtIn = BUILT_IN[entry.id];
  if (builtIn) return { kind: "builtIn", harness: builtIn };
  const id = customId(entry.id);
  const make = (command: string, args: string[], env: Record<string, string> | undefined) => {
    const agent: CustomAcpAgent = { id, name: entry.name.trim(), command, args, env: { ...(env ?? {}) }, modelIds: [] };
    return validate(agent) ? null : agent;
  };
  const dist = entry.distribution ?? {};
  if (dist.npx) {
    const agent = make("npx", ["-y", dist.npx.package, ...(dist.npx.args ?? [])], dist.npx.env);
    return agent && { kind: "add", agent };
  }
  if (dist.uvx) {
    const agent = make("uvx", [dist.uvx.package, ...(dist.uvx.args ?? [])], dist.uvx.env);
    return agent && { kind: "add", agent };
  }
  const binary = dist.binary?.[platform];
  if (!binary) {
    if (!dist.local) return null;
    const agent = make(dist.local.command, dist.local.args ?? [], dist.local.env);
    return agent && { kind: "add", agent };
  }
  // Installed by hand, the executable is on PATH under its own name.
  const name = binary.cmd.split(/[/\\]/).pop() || binary.cmd;
  const agent = make(name, binary.args ?? [], binary.env);
  return agent && { kind: "manual", agent };
}

export const matches = (entry: RegistryAgent, query: string) => {
  const q = query.trim().toLowerCase();
  return !q || `${entry.name} ${entry.description ?? ""}`.toLowerCase().includes(q);
};

function failure(error: unknown) {
  return uncertain(error) ? "The connection dropped before the engine answered. Refresh custom agents before trying again." : message(error);
}

function isSnapshot(value: unknown): value is CustomAcpAgents {
  return !!value && typeof value === "object" && Array.isArray((value as CustomAcpAgents).agents) && typeof (value as CustomAcpAgents).revision === "number";
}

async function mutate(label: string, method: string, params: Record<string, unknown>): Promise<boolean> {
  if (custom.busy) return false;
  setCustom({ busy: label, error: null });
  try {
    const reply = await rpc.call(method, params);
    if (!isSnapshot(reply)) throw new Error("Could not read custom agent configuration. Refresh before retrying.");
    setData("custom", reply);
    for (const id of Object.keys(custom.probes)) if (!reply.agents.some((a) => a.id === id)) setCustom("probes", id, undefined);
    void refreshHarnesses();
    return true;
  } catch (error) {
    setCustom({ error: failure(error) });
    return false;
  } finally {
    setCustom({ busy: null });
  }
}

/** Saves a new or edited agent, then tests that it starts. Resolves to an error message, or null. */
export async function saveAgent(agent: CustomAcpAgent): Promise<string | null> {
  const revision = data.custom?.revision;
  if (revision === undefined) return "Custom agents are still loading. Try again in a moment.";
  const ok = await mutate("Saving…", M.SET_CUSTOM_ACP_AGENT, { agent, expectedRevision: revision });
  if (!ok) return custom.error;
  setCustom("probes", agent.id, undefined);
  void probe(agent.id);
  return null;
}

export function duplicateAgent(agent: CustomAcpAgent) {
  const revision = data.custom?.revision;
  if (revision === undefined) return;
  const id = `acp-${crypto.randomUUID().replace(/-/g, "")}`;
  void mutate("Duplicating…", M.SET_CUSTOM_ACP_AGENT, { agent: { ...agent, id, name: `${agent.name} copy` }, expectedRevision: revision });
}

export function removeAgent(id: string) {
  const revision = data.custom?.revision;
  if (revision === undefined) return;
  void mutate("Removing agent…", M.DELETE_CUSTOM_ACP_AGENT, { harness: id, expectedRevision: revision });
}

/** Starting the agent and listing its models proves the connection. */
export async function probe(id: string) {
  if (custom.probes[id]?.kind === "testing") return;
  setCustom("probes", id, { kind: "testing" });
  await loadModels(id, true);
  const error = data.modelErrors[id];
  setCustom("probes", id, error ? { kind: "error", message: error } : { kind: "ready" });
}

export async function loadCatalog() {
  if (custom.catalogLoading) return;
  setCustom({ catalogLoading: true, catalogError: null });
  try {
    const catalog = await rpc.call<Catalog>(M.ACP_CATALOG_GET, {});
    setCustom({ catalog });
    for (const entry of catalog.agents) if (entry.icon && custom.icons[entry.id] === undefined) void loadIcon(entry.id);
  } catch (error) {
    setCustom({ catalogError: message(error) });
  } finally {
    setCustom({ catalogLoading: false });
  }
}

async function loadIcon(id: string) {
  setCustom("icons", id, null);
  try {
    const svg = await rpc.call<string>(M.ACP_CATALOG_ICON, { id });
    // Drawn through <img>, so registry markup can never run script in the page.
    if (typeof svg === "string" && svg.trim().startsWith("<"))
      setCustom("icons", id, `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg.replace(/currentColor/g, "#8a8a8a"))}`);
  } catch {
    // The entry keeps its generic glyph.
  }
}

export { loadCustomAgents };
