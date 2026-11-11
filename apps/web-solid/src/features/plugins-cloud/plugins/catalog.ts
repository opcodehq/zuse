// Plugin discovery rules shared with desktop (crates/viewport/src/plugin_catalog.rs).
import bundled from "@repo/crates/protocol/src/plugin-catalog.json?raw";
import iconTable from "@repo/crates/icons/src/plugins.rs?raw";

export interface PluginDefinition {
  id: string;
  name: string;
  description: string;
  domain: string;
  category?: string | null;
  featured?: boolean;
  custom?: boolean;
  url?: string | null;
  apiKeyHeader?: string | null;
  command?: string | null;
}

export type ConnectionState = "connecting" | "connected" | "needs-auth" | "error";
export interface PluginConnection {
  id: string;
  pluginId: string;
  label: string;
  owner: string;
  state: ConnectionState;
  createdAt: number;
  enabled?: boolean;
}
export interface PluginTenant {
  id: string;
  kind: string;
  name: string;
}
export interface PluginSnapshot {
  kind: "snapshot";
  canManage?: boolean;
  tenants: PluginTenant[];
  tenantId: string;
  endpoint: string;
  catalog: PluginDefinition[];
  connections: PluginConnection[];
}
export interface PluginAttempt {
  kind: "attempt";
  id: string;
  connectionId: string;
  state: "pending" | "connected" | "failed" | "cancelled";
  authorizationUrl?: string | null;
  expiresAt: number;
  error?: string | null;
  pluginId?: string;
}

function normalize(item: unknown): PluginDefinition | null {
  const row = item as Partial<PluginDefinition> | null;
  if (!row || typeof row.id !== "string" || typeof row.name !== "string" || typeof row.description !== "string") return null;
  return { ...row, id: row.id, name: row.name, description: row.description, domain: row.domain ?? "" };
}

const fallback = (JSON.parse(bundled) as unknown[]).map(normalize).filter((row): row is PluginDefinition => !!row);

export function catalog(snapshot: PluginSnapshot | null, query = "", connectedOnly = false): PluginDefinition[] {
  const source = snapshot ? snapshot.catalog.map(normalize).filter((row): row is PluginDefinition => !!row) : fallback;
  const needle = query.trim().toLowerCase();
  const seen = new Set<string>();
  return source.filter((item) => {
    if (seen.has(item.id)) return false;
    if (connectedOnly && !connected(snapshot, item.id)) return false;
    if (needle && !`${item.name} ${item.description} ${item.domain} ${item.url ?? ""} ${item.command ?? ""}`.toLowerCase().includes(needle)) return false;
    seen.add(item.id);
    return true;
  });
}

export function connected(snapshot: PluginSnapshot | null, id: string) {
  return !!snapshot?.connections.some((row) => row.pluginId === id && row.state === "connected");
}

export function connectedCount(snapshot: PluginSnapshot | null) {
  return new Set(snapshot?.connections.filter((row) => row.state === "connected").map((row) => row.pluginId)).size;
}

/** Choose an unused connection label, counting disabled accounts too. */
export function nextLabel(snapshot: PluginSnapshot | null, plugin: PluginDefinition) {
  for (let index = 1; ; index++) {
    const label = index === 1 ? plugin.name : `${plugin.name} ${index}`;
    const taken = snapshot?.connections.some((row) => row.pluginId === plugin.id && row.label.trim().toLowerCase() === label.toLowerCase());
    if (!taken) return label;
  }
}

export const PAGE_SIZE = 40;
export interface Section {
  title: string;
  key: string;
  items: PluginDefinition[];
}

export function browse(snapshot: PluginSnapshot | null, query: string, connectedOnly: boolean, page: number) {
  const items = catalog(snapshot, query, connectedOnly);
  const limit = (page + 1) * PAGE_SIZE;
  if (connectedOnly || query.trim()) {
    return {
      sections: items.length ? [{ title: connectedOnly ? "Connected" : "Results", key: "plugin-results", items: items.slice(0, limit) }] : [],
      more: items.length > limit,
    };
  }
  const custom = items.filter((p) => p.custom);
  const featured = items.filter((p) => !p.custom && p.featured).slice(0, 12);
  const all = items.filter((p) => !p.custom);
  const sections: Section[] = [
    { title: "Custom servers", key: "plugin-custom", items: custom },
    { title: "Featured", key: "plugin-featured", items: featured },
    { title: "All plugins", key: "plugin-all", items: all.slice(0, limit) },
  ].filter((section) => section.items.length > 0);
  return { sections, more: all.length > limit };
}

/** Subtitle under a plugin's name: custom servers without copy show where they run. */
export function subtitle(item: PluginDefinition) {
  if (item.custom && !item.description.trim()) return item.url ?? item.command ?? item.domain;
  return item.description;
}

// Brand tiles are the desktop's bundled images, keyed by domain in crates/icons/src/plugins.rs.
const images = import.meta.glob("../../../../../../crates/icons/assets/plugins/*", { query: "?url", import: "default", eager: true }) as Record<string, string>;
const byFile = new Map(Object.entries(images).map(([path, url]) => [path.slice(path.lastIndexOf("/") + 1), url]));
const logos = new Map<string, string>();
for (const match of iconTable.matchAll(/"plugin-icons\/([^"]+)"\s*=>\s*Some\(include_bytes!\("\.\.\/assets\/plugins\/([^"]+)"\)\)/g)) {
  const url = byFile.get(match[2]!);
  if (url) logos.set(match[1]!, url);
}
export const logoUrl = (domain: string) => logos.get(domain) ?? null;
