// The main-area plugin catalog: title, toolbar, Browse / Connected tabs, then Featured and All plugins.
import { createMemo, createSignal, For, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Button } from "@/ui/primitives";
import { signedIn } from "../account";
import { Logo, PageTitle, SearchField, Segmented } from "../ui";
import { browse, connected, connectedCount, subtitle, type PluginDefinition } from "./catalog";
import { Detail } from "./Detail";
import { PluginStatus, Refresh, SignedOut } from "./parts";
import { canManage, connect, definition, locked, openDetail, openServerSetup, plugins, refresh, tenant } from "./state";

const [query, setQuery] = createSignal("");
const [connectedOnly, setConnectedOnly] = createSignal(false);
const [page, setPage] = createSignal(0);

export function Browse() {
  const selected = () => definition(plugins.selected);
  return (
    <div class="pc-content">
      <Show when={selected()} keyed fallback={<Catalog />}>
        {(plugin) => (
          <>
            <PluginStatus />
            <Detail plugin={plugin} />
          </>
        )}
      </Show>
    </div>
  );
}

function Catalog() {
  const listing = createMemo(() => browse(plugins.snapshot, query(), connectedOnly(), page()));
  const count = () => connectedCount(plugins.snapshot);
  return (
    <>
      <div class="pc-head">
        <PageTitle title="Plugins" subtitle="Connect your tools so agents can work across them in any chat." />
        <div class="pc-toolbar">
          <SearchField
            value={query()}
            width={224}
            placeholder="Search plugins"
            label="Search plugins"
            onInput={(value) => {
              setQuery(value);
              setPage(0);
            }}
          />
          <Show when={signedIn()}>
            <Button variant="outline" disabled={!canManage() || plugins.busy || !tenant()} onClick={openServerSetup}>
              Add MCP server
            </Button>
          </Show>
          <Refresh label="Refresh plugins" busy={plugins.busy} onClick={refresh} />
        </div>
      </div>
      <div>
        <Segmented
          label="Plugin catalog"
          value={connectedOnly() ? "connected" : "browse"}
          onChange={(value) => {
            setConnectedOnly(value === "connected");
            setPage(0);
          }}
          items={[
            { value: "browse", label: "Browse" },
            { value: "connected", label: "Connected", count: count() || undefined },
          ]}
        />
      </div>
      <PluginStatus />
      <Show when={signedIn()} fallback={<SignedOut />}>
        <Show
          when={plugins.snapshot}
          fallback={<p class="pc-muted pc-pad">{plugins.busy ? "Loading plugins…" : "Plugins are unavailable. Refresh to try again."}</p>}
        >
          <Show
            when={listing().sections.length > 0}
            fallback={<p class="pc-muted pc-empty-line">{connectedOnly() && !query().trim() ? "No plugins connected yet" : "No plugins match your search"}</p>}
          >
            <For each={listing().sections}>
              {(section) => (
                <section class="pc-catalog-section" aria-label={section.title}>
                  <h2>{section.title}</h2>
                  <div class="pc-catalog-grid">
                    <For each={section.items}>{(item) => <CatalogRow plugin={item} />}</For>
                  </div>
                </section>
              )}
            </For>
            <Show when={listing().more}>
              <div>
                <Button variant="outline" onClick={() => setPage(page() + 1)}>
                  Show more
                </Button>
              </div>
            </Show>
          </Show>
        </Show>
      </Show>
    </>
  );
}

function CatalogRow(props: { plugin: PluginDefinition }) {
  const isConnected = () => connected(plugins.snapshot, props.plugin.id);
  return (
    <div class="pc-catalog-row">
      <button type="button" class="pc-catalog-open" aria-label={`Open ${props.plugin.name} details`} onClick={() => openDetail(props.plugin.id)}>
        <Logo domain={props.plugin.domain} name={props.plugin.name} size={36} />
        <span class="pc-catalog-text">
          <span class="pc-catalog-name">
            <span class="pc-truncate">{props.plugin.name}</span>
            <Show when={props.plugin.custom}>
              <span class="pc-tag">Custom</span>
            </Show>
          </span>
          <span class="pc-catalog-sub pc-truncate">{subtitle(props.plugin)}</span>
        </span>
      </button>
      <button
        type="button"
        class="icon-btn"
        aria-label={isConnected() ? `${props.plugin.name} connected` : `Connect ${props.plugin.name}`}
        title={isConnected() ? "Connected" : "Connect plugin"}
        aria-disabled={!isConnected() && locked() ? "true" : undefined}
        data-dim={!isConnected() && locked() ? "true" : undefined}
        onClick={() => (isConnected() ? openDetail(props.plugin.id) : connect(props.plugin))}
      >
        <Icon name={isConnected() ? "check" : "plus"} size={15} />
      </button>
    </div>
  );
}
