// Input rules shared with desktop (crates/protocol/src/plugin_setup.rs). Credentials never leave the request.
import { newId } from "../account";
import type { PluginDefinition } from "./catalog";

export type ServerAuth = "automatic" | "bearer" | "header";
export type PluginRequest = Record<string, unknown> & { action: string };
type Result = { ok: PluginRequest } | { error: string };

const RETURN_TO = { kind: "web" } as const;
const NAME_INVALID = "Enter a server name between 1 and 60 characters.";
export const LOCAL_CONFIG_TEMPLATE = `{"command":"","args":[]}`;
export const LOCAL_CONFIG_INVALID = "Enter a valid stdio MCP configuration with one command, an args array, and optional env values.";

const length = (text: string) => [...text].length;
const CONTROL = /[\u0000-\u001f\u007f]/;

export function apiKey(header: string, raw: string): { header: string; value: string } | string {
  const name = header.trim();
  if (!name || !/^[A-Za-z0-9!#$%&'*+\-.^_`|~]+$/.test(name)) return "Enter a valid HTTP header name.";
  const value = raw.trim();
  if (!value || CONTROL.test(value)) return "Enter an API key without line breaks or control characters.";
  const [scheme, ...tail] = value.split(/\s+/);
  const bearer = scheme?.toLowerCase() === "bearer" && tail.join(" ").trim() !== "";
  return { header: name, value: name.toLowerCase() === "authorization" && !bearer ? `Bearer ${value}` : value };
}

export function serverRequest(tenant: string, draft: { name: string; url: string; auth: ServerAuth; header: string; secret: string }): Result {
  if (!tenant) return { error: "Choose a workspace before adding a server." };
  const name = draft.name.trim();
  if (!name || length(name) > 60) return { error: NAME_INVALID };
  const url = draft.url.trim();
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return { error: "Enter an HTTPS server URL." };
  }
  if (parsed.protocol !== "https:" || !parsed.hostname || parsed.username || parsed.password || url.includes("#")) {
    return { error: "Enter an HTTPS server URL without embedded credentials or a fragment." };
  }
  let key: { header: string; value: string } | undefined;
  if (draft.auth !== "automatic") {
    const result = apiKey(draft.auth === "bearer" ? "Authorization" : draft.header, draft.secret);
    if (typeof result === "string") return { error: result };
    key = result;
  }
  return {
    ok: { action: "addServer", tenantId: tenant, requestId: newId(), name, url, ...(key ? { apiKey: key } : {}), returnTo: RETURN_TO },
  };
}

function parseLocalConfig(text: string): { command: string; args: string[]; env: Record<string, string> } | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  const isObject = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
  if (!isObject(value)) return null;
  if ("mcpServers" in value) {
    const servers = value.mcpServers;
    if (!isObject(servers) || Object.keys(servers).length !== 1) return null;
    value = Object.values(servers)[0];
    if (!isObject(value)) return null;
  }
  const object = value as Record<string, unknown>;
  if ("type" in object && object.type !== "stdio") return null;
  if (typeof object.command !== "string" || !object.command.trim()) return null;
  const args = object.args === undefined ? [] : object.args;
  if (!Array.isArray(args) || !args.every((arg) => typeof arg === "string")) return null;
  const env = object.env === undefined ? {} : object.env;
  if (!isObject(env) || !Object.values(env).every((v) => typeof v === "string")) return null;
  return { command: object.command.trim(), args: args as string[], env: env as Record<string, string> };
}

const envName = (name: string) => name.length > 0 && name.length <= 256 && /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
const bytes = (text: string) => new TextEncoder().encode(text).length;

function validateLocal(name: string, command: string, args: string[], env: Record<string, string>): string | null {
  if (!name.trim() || length(name) > 60 || /[\0\n\r]/.test(name)) return NAME_INVALID;
  if (!command.trim() || bytes(command) > 8192 || /[\0\n\r]/.test(command)) return "Enter a command or executable path.";
  if (args.length > 128 || args.some((arg) => bytes(arg) > 8192 || arg.includes("\0")) || args.reduce((n, arg) => n + bytes(arg), 0) > 128 * 1024) {
    return "Arguments exceed the launch limit or contain invalid characters.";
  }
  const entries = Object.entries(env);
  if (entries.some(([key]) => !envName(key))) return "Environment variable names use letters, digits and underscores.";
  if (entries.length > 128 || entries.some(([, v]) => bytes(v) > 64 * 1024 || v.includes("\0")) || entries.reduce((n, [k, v]) => n + bytes(k) + bytes(v), 0) > 128 * 1024) {
    return "Environment values exceed the launch limit or contain invalid characters.";
  }
  return null;
}

export function localRequest(tenant: string, draft: { name: string; config: string }): Result {
  if (!tenant.startsWith("personal:")) return { error: "Local servers can only be added to your personal workspace." };
  const name = draft.name.trim();
  if (!name || length(name) > 60) return { error: NAME_INVALID };
  const parsed = parseLocalConfig(draft.config);
  if (!parsed) return { error: LOCAL_CONFIG_INVALID };
  const invalid = validateLocal(name, parsed.command, parsed.args, parsed.env);
  if (invalid) return { error: invalid };
  return { ok: { action: "addLocalServer", tenantId: tenant, requestId: newId(), name, ...parsed } };
}

export function connectRequest(tenant: string, plugin: PluginDefinition, label: string, secret: string | null): Result {
  const name = label.trim();
  if (!name || length(name) > 60) return { error: "Enter a connection name between 1 and 60 characters." };
  let key: string | undefined;
  if (plugin.apiKeyHeader) {
    const result = apiKey(plugin.apiKeyHeader, secret ?? "");
    if (typeof result === "string") return { error: result };
    key = result.value;
  }
  return {
    ok: { action: "connect", tenantId: tenant, pluginId: plugin.id, label: name, requestId: newId(), returnTo: RETURN_TO, ...(key ? { apiKey: key } : {}) },
  };
}
