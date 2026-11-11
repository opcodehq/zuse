// One plugin: identity, connected accounts and information (crates/viewport/src/plugin_detail.rs).
import { writeClipboard } from "@/ui/clipboard";
import { createMemo, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { Menu, Toggle, toast } from "@/ui/primitives";
import { openExternal } from "../account";
import { formatDate, Logo } from "../ui";
import type { PluginConnection, PluginDefinition } from "./catalog";
import { InlineKey } from "./parts";
import { askRemoval, canManage, connect, disconnect, openDetail, plugins, setEnabled, activeAttempt } from "./state";

// Label-derived avatar hue, matching desktop's string hash.
function hue(label: string) {
  let hash = 0;
  for (let i = 0; i < label.length; i++) hash = (Math.imul(hash, 31) + label.charCodeAt(i)) >>> 0;
  return hash % 360;
}
function initials(label: string) {
  const words = label.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => [...w][0] ?? "");
  const value = words.join("");
  return ([...value].length > 1 ? value : [...label].slice(0, 2).join("")).toUpperCase();
}

function connectionStatus(connection: PluginConnection) {
  if (connection.enabled === false) return "Off · agents can't use this account";
  switch (connection.state) {
    case "connected": {
      const date = formatDate(connection.createdAt);
      return date ? `Connected ${date}` : "Connected";
    }
    case "needs-auth":
      return "Needs authorization";
    case "connecting":
      return "Connecting…";
    default:
      return "Connection failed";
  }
}

export function Detail(props: { plugin: PluginDefinition }) {
  const busy = () => plugins.busy || activeAttempt();
  const available = () => canManage() && !busy();
  const connections = createMemo(() => plugins.snapshot?.connections.filter((row) => row.pluginId === props.plugin.id) ?? []);
  const asking = () => {
    const setup = plugins.setup;
    return setup?.kind === "catalog" && setup.plugin.id === props.plugin.id ? setup : null;
  };
  const subtitle = () => {
    const plugin = props.plugin;
    if (plugin.custom && plugin.url) {
      try {
        return new URL(plugin.url).host;
      } catch {
        return plugin.url;
      }
    }
    return plugin.command ? "Local MCP server" : plugin.domain;
  };
  const website = () => `https://${props.plugin.domain}`;
  const copyUrl = async () => {
    if (await writeClipboard(props.plugin.url ?? "")) toast("Server URL copied");
    else toast("Could not copy the URL. Copy it from Information instead.", "danger");
  };
  const authentication = () => {
    const header = props.plugin.apiKeyHeader;
    if (!header) return "Sign in or none";
    return header.toLowerCase() === "authorization" ? "Bearer token" : `Custom header (${header})`;
  };
  const privacy = () =>
    props.plugin.command
      ? "Runs on the computer hosting this Zuse engine with its owner's permissions. Its command and environment are stored there and are not sent to Zuse servers. Turning it off stops the process."
      : plugins.snapshot?.tenantId.startsWith("organization:")
        ? "Connections are shared with members of this organization. Tokens stay encrypted on Zuse servers and never reach agent machines. Disconnecting stops new calls immediately."
        : "Connections are private to your Zuse account. Tokens stay encrypted on Zuse servers and never reach agent machines. Disconnecting stops new calls immediately.";

  return (
    <article class="pc-detail">
      <nav class="pc-crumbs" aria-label="Breadcrumb">
        <button type="button" class="pc-crumb" onClick={() => openDetail(null)}>
          Plugins
        </button>
        <span aria-hidden="true">/</span>
        <span class="pc-truncate" aria-current="page">
          {props.plugin.name}
        </span>
      </nav>
      <header class="pc-detail-head">
        <Logo domain={props.plugin.domain} name={props.plugin.name} size={48} radius={14} />
        <div class="pc-detail-heading">
          <div class="pc-detail-title-row">
            <div class="pc-detail-title">
              <h1 class="pc-truncate">{props.plugin.name}</h1>
              <Show when={props.plugin.custom}>
                <span class="pc-tag">Custom</span>
              </Show>
            </div>
            <div class="pc-detail-actions">
              <Show when={busy()}>
                <span class="pc-waiting" role="status">
                  <GlyphSpinner decorative />
                  Waiting…
                </span>
              </Show>
              <Show when={!props.plugin.custom || canManage()}>
                <Menu
                  placement="bottom-end"
                  trigger={(trigger) => (
                    <button {...trigger} type="button" class="icon-btn" aria-label="More actions" title="More actions">
                      <Icon name="more-horizontal" size={16} />
                    </button>
                  )}
                  items={
                    props.plugin.custom
                      ? [{ label: "Remove server", icon: "trash-bin-minimalistic", danger: true, disabled: !available(), onSelect: () => askRemoval("server", props.plugin.id) }]
                      : [{ label: "Visit website", icon: "global", onSelect: () => openExternal(website()) }]
                  }
                />
              </Show>
              <Show when={props.plugin.custom && props.plugin.url}>
                <button type="button" class="pc-detail-button" data-tinted="true" onClick={() => void copyUrl()}>
                  <Icon name="copy" size={14} />
                  Copy URL
                </button>
              </Show>
              <Show when={connections().length === 0 && canManage() && !asking()}>
                <button type="button" class="pc-action" data-primary="true" disabled={!available()} onClick={() => connect(props.plugin)}>
                  <span class="pc-action-label">Connect</span>
                </button>
              </Show>
            </div>
          </div>
          <Show when={subtitle()}>
            <p class="pc-detail-sub pc-truncate">{subtitle()}</p>
          </Show>
        </div>
      </header>
      <Show when={props.plugin.description || !canManage()}>
        <div class="pc-detail-description">
          <Show when={props.plugin.description}>
            <p>{props.plugin.description}</p>
          </Show>
          <Show when={!canManage()}>
            <p class="pc-muted">Only workspace administrators can manage these connections.</p>
          </Show>
        </div>
      </Show>
      <Show when={connections().length > 0 || asking()}>
        <section class="pc-detail-section" aria-label="Connected accounts">
          <h2>Connected accounts</h2>
          <div class="pc-accounts">
            <For each={connections()}>
              {(connection) => {
                const enabled = () => connection.enabled !== false;
                return (
                  <div class="pc-account">
                    <span
                      class="pc-avatar"
                      aria-hidden="true"
                      style={{ background: enabled() ? `oklch(0.6 0.14 ${hue(connection.label)})` : "oklch(0.6 0 0)", opacity: enabled() ? 1 : 0.4 }}
                    >
                      {initials(connection.label || props.plugin.name)}
                    </span>
                    <div class="pc-account-text">
                      <span class="pc-account-label pc-truncate" data-off={enabled() ? undefined : "true"}>
                        {connection.label || props.plugin.name}
                      </span>
                      <span class="pc-account-status pc-truncate">{connectionStatus(connection)}</span>
                    </div>
                    <Toggle checked={enabled()} label={`Enable ${connection.label || props.plugin.name}`} disabled={!available()} onChange={(value) => setEnabled(connection.id, value)} />
                    <Show when={canManage()}>
                      <Menu
                        placement="bottom-end"
                        trigger={(trigger) => (
                          <button {...trigger} type="button" class="icon-btn pc-account-menu" disabled={!available()} aria-label={`Account actions: ${connection.label || props.plugin.name}`}>
                            <Icon name="more-horizontal" size={16} />
                          </button>
                        )}
                        items={[
                          { label: enabled() ? "Turn off" : "Turn on", onSelect: () => setEnabled(connection.id, !enabled()) },
                          { label: "Disconnect", icon: "close-circle", danger: true, onSelect: () => disconnect(connection.id) },
                        ]}
                      />
                    </Show>
                  </div>
                );
              }}
            </For>
            <Show
              when={asking()}
              fallback={
                <Show when={canManage() && !props.plugin.id.startsWith("local-")}>
                  <button type="button" class="pc-account pc-connect-another" disabled={!available()} onClick={() => connect(props.plugin)}>
                    <span class="pc-add-glyph" aria-hidden="true">
                      <Icon name="plus" size={12} />
                    </span>
                    Connect another account
                  </button>
                </Show>
              }
            >
              {(setup) => <InlineKey plugin={setup().plugin} label={setup().label} />}
            </Show>
          </div>
        </section>
      </Show>
      <section class="pc-detail-section" aria-label="Information">
        <h2>Information</h2>
        <dl class="pc-info">
          <Show when={props.plugin.command}>
            {(command) => (
              <>
                <dt>Command</dt>
                <dd class="pc-mono pc-truncate">{command()}</dd>
                <dt>Transport</dt>
                <dd>Local (stdio)</dd>
              </>
            )}
          </Show>
          <Show when={!props.plugin.command && props.plugin.custom}>
            <dt>Server URL</dt>
            <dd class="pc-mono pc-truncate">{props.plugin.url ?? ""}</dd>
            <dt>Authentication</dt>
            <dd>{authentication()}</dd>
          </Show>
          <Show when={!props.plugin.command && !props.plugin.custom && props.plugin.domain}>
            <dt>Website</dt>
            <dd>
              <a class="pc-inline-link" href={website()} target="_blank" rel="noopener noreferrer">
                {props.plugin.domain}
                <Icon name="arrow-up-right" size={12} class="pc-muted" />
              </a>
            </dd>
          </Show>
          <Show when={props.plugin.category}>
            {(category) => (
              <>
                <dt>Category</dt>
                <dd class="pc-capitalize">{category().replace(/-/g, " ")}</dd>
              </>
            )}
          </Show>
          <dt>Access</dt>
          <dd>Read and write, within each chat's permissions</dd>
        </dl>
        <p class="pc-privacy">{privacy()}</p>
      </section>
    </article>
  );
}
