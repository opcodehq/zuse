// Plugins and Cloud. The plugins route is the catalog alone, as on desktop. Settings mounts the same export for its
// plugins and cloud sections, so it follows the route to the installed list or the Cloud workspaces page.
import { Match, Switch } from "solid-js";
import { navigate, view } from "@/core/view";
import { PageTitle } from "./ui";
import { Browse } from "./plugins/Browse";
import { Installed } from "./plugins/Installed";
import { openDetail } from "./plugins/state";
import { CloudSettings, selectCloudTab, type CloudTab } from "./cloud";
import { Archives } from "./cloud/Archives";
import "./plugins-cloud.css";

const settingsSection = () => (view.route.kind === "settings" ? view.route.section ?? null : null);

/** Opens the plugin catalog, optionally on one plugin's detail page. */
export function openPlugins(pluginId: string | null = null) {
  openDetail(pluginId);
  navigate({ kind: "plugins" });
}

/** Opens Settings → Cloud workspaces on the given tab. */
export function openCloud(tab: CloudTab = "setup") {
  selectCloudTab(tab);
  navigate({ kind: "settings", section: "cloud" });
}

export function Plugins() {
  return (
    <div class="pc-root">
      <div class="pc-scroll">
        <Switch fallback={<Browse />}>
          <Match when={settingsSection() === "cloud"}>
            <CloudSettings />
          </Match>
          <Match when={settingsSection() === "plugins"}>
            <PluginSettings />
          </Match>
        </Switch>
      </div>
    </div>
  );
}

/** Settings → Plugins: connected plugins with on/off, disconnect and add. */
export function PluginSettings() {
  return (
    <div class="pc-page">
      <PageTitle title="Plugins" subtitle="Turn plugins and MCP servers on or off for your agents." />
      <Installed onBrowse={() => openPlugins()} />
    </div>
  );
}

/** Settings → Cloud: the full Cloud workspaces page. */
export { CloudSettings };
/** Settings → Archived chats: saved cloud archives. */
export { Archives as CloudArchives };
