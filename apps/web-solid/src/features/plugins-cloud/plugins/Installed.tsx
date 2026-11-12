// Settings → Plugins: connected plugins with on/off and disconnect (desktop PluginsCard::render_installed).
import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Button, Menu, Toggle } from "@/ui/primitives";
import { signIn, signedIn } from "../account";
import { Group, Logo, SearchField, Segmented } from "../ui";
import { catalog, subtitle } from "./catalog";
import { PluginStatus } from "./parts";
import { canManage, disconnect, locked, openServerSetup, plugins, setEnabled } from "./state";

function describe(state: string, description: string) {
  switch (state) {
    case "connected":
    case "":
      return description;
    case "needs-auth":
      return "Needs authorization";
    case "connecting":
      return "Connecting…";
    case "error":
      return "Connection failed — reconnect from the catalog";
    default:
      return state;
  }
}

export function Installed(props: { onBrowse: () => void }) {
  const [query, setQuery] = createSignal("");
  const rows = createMemo(() => {
    const definitions = catalog(plugins.snapshot);
    return (plugins.snapshot?.connections ?? []).map((row) => {
      const definition = definitions.find((item) => item.id === row.pluginId);
      return {
        id: row.id,
        label: row.label || definition?.name || "Plugin",
        description: definition ? subtitle(definition) : "",
        domain: definition?.domain ?? "",
        state: row.state,
        enabled: row.enabled !== false,
      };
    });
  });
  const shown = createMemo(() => {
    const needle = query().trim().toLowerCase();
    return needle ? rows().filter((row) => `${row.label} ${row.description}`.toLowerCase().includes(needle)) : rows();
  });
  return (
    <div class="pc-installed">
      <div class="pc-installed-head">
        <Segmented label="Plugin settings" value="plugins" onChange={() => {}} items={[{ value: "plugins", label: "Plugins", count: rows().length }]} />
        <span class="pc-grow" />
        <SearchField value={query()} onInput={setQuery} placeholder="Search" label="Search connected plugins" width={192} />
        <Menu
          placement="bottom-end"
          trigger={(trigger) => (
            <button {...trigger} type="button" class="btn" data-variant="outline" data-size="md">
              Add
              <Icon name="alt-arrow-down" size={12} />
            </button>
          )}
          items={[
            { label: "Browse plugins", icon: "widget", onSelect: props.onBrowse },
            { label: "Add MCP server", icon: "plus", disabled: signedIn() && (!canManage() || plugins.busy), onSelect: openServerSetup },
          ]}
        />
      </div>
      <PluginStatus />
      <Group title="Connected">
        <Show
          when={shown().length > 0}
          fallback={
            <div class="pc-installed-empty">
              <strong>{rows().length === 0 ? "No plugins connected yet" : "No connected plugins match your search"}</strong>
              <Show when={rows().length === 0}>
                <span>Connect tools from the catalog so agents can use them in any chat.</span>
                <div class="pc-installed-empty-actions">
                  <Button variant="outline" onClick={() => props.onBrowse()}>
                    Browse plugins
                  </Button>
                  <Show when={!signedIn()}>
                    <Button variant="outline" onClick={() => void signIn()}>
                      Sign in
                    </Button>
                  </Show>
                </div>
              </Show>
            </div>
          }
        >
          <For each={shown()}>
            {(row) => (
              <div class="pc-row pc-installed-row">
                <div class="pc-row-main">
                  <Logo domain={row.domain} name={row.label} size={32} />
                  <div class="pc-row-copy">
                    <div class="pc-row-title pc-truncate">{row.label}</div>
                    <div class="pc-row-description pc-truncate">{describe(row.state, row.description)}</div>
                  </div>
                  <button type="button" class="btn pc-reveal" data-variant="ghost" data-size="sm" disabled={locked()} onClick={() => disconnect(row.id)}>
                    Disconnect
                  </button>
                  <Toggle checked={row.enabled} label={row.label} disabled={locked()} onChange={(value) => setEnabled(row.id, value)} />
                </div>
              </div>
            )}
          </For>
        </Show>
      </Group>
    </div>
  );
}
