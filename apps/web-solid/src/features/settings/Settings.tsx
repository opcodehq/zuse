// Settings: section navigation on the window surface and the section in a rounded pane (crates/ui/src/shell.rs
// render_settings_page). Sections owned by other areas are mounted from their exports.
import { createMemo, createSignal, For, type JSX, lazy, Match, onMount, Show, Switch } from "solid-js";
import { navigate, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { DevicesSettings } from "@/features/connection";
import { CloudArchives, Plugins } from "@/features/plugins-cloud";
import { ProjectArchive } from "@/features/projects";
import { t } from "./i18n";
import { General } from "./pages/General";
import { Segmented } from "./widgets";

const Providers = lazy(() => import("./pages/Providers").then((m) => ({ default: m.Providers })));
const Defaults = lazy(() => import("./pages/Defaults").then((m) => ({ default: m.Defaults })));
const Appearance = lazy(() => import("./pages/Appearance").then((m) => ({ default: m.Appearance })));
const Notifications = lazy(() => import("./pages/Notifications").then((m) => ({ default: m.Notifications })));
const Shortcuts = lazy(() => import("./pages/Shortcuts").then((m) => ({ default: m.Shortcuts })));

export type SectionId = "general" | "providers" | "defaults" | "plugins" | "devices" | "cloud" | "appearance" | "notifications" | "shortcuts" | "archived";

interface Section {
  id: SectionId;
  icon: string;
  label: () => string;
}

const SECTIONS: Section[] = [
  { id: "general", icon: "dither-gear", label: () => t("settings-settings_page_general", "General") },
  { id: "providers", icon: "dither-term", label: () => t("settings-settings_page_providers", "Providers") },
  { id: "defaults", icon: "dither-sliders", label: () => t("settings-settings_page_default_models", "Default models") },
  { id: "plugins", icon: "dither-plug", label: () => t("plugins-plugins_title", "Plugins") },
  { id: "devices", icon: "dither-monitor", label: () => t("settings-settings_page_remote_access", "Remote access") },
  { id: "cloud", icon: "dither-cloud", label: () => t("native-cloud_workspaces", "Cloud workspaces") },
  { id: "appearance", icon: "dither-pen", label: () => t("settings-settings_page_appearance", "Appearance") },
  { id: "notifications", icon: "dither-bell", label: () => t("settings-settings_page_notifications", "Notifications") },
  { id: "shortcuts", icon: "dither-keyboard", label: () => t("settings-settings_page_keyboard_shortcuts", "Keyboard shortcuts") },
  { id: "archived", icon: "dither-archive", label: () => t("projects-projects_sidebar_archived_chats", "Archived chats") },
];

/** Route slugs, including desktop's former page names. */
const ALIASES: Record<string, SectionId> = {
  general: "general",
  conversations: "general",
  providers: "providers",
  harnesses: "providers",
  agents: "providers",
  accounts: "providers",
  defaults: "defaults",
  "default-models": "defaults",
  plugins: "plugins",
  devices: "devices",
  remote: "devices",
  cloud: "cloud",
  appearance: "appearance",
  notifications: "notifications",
  shortcuts: "shortcuts",
  archived: "archived",
};

export const currentSection = (): SectionId => {
  const slug = view.route.kind === "settings" ? view.route.section : undefined;
  return (slug && ALIASES[slug]) || "general";
};

export const openSection = (id: SectionId) => navigate({ kind: "settings", section: id });

export function closeSettings() {
  navigate({ kind: "chat" });
}

/** Something layered above the page (a menu, select or dialog) owns Escape first. */
const layerOpen = () => !!document.querySelector('[role="dialog"]:not(.st-page), [role="listbox"], [role="menu"]');

export function Settings() {
  let page: HTMLDivElement | undefined;
  const section = createMemo(currentSection);
  onMount(() => page?.focus({ preventScroll: true }));

  return (
    <div
      ref={page}
      class="st-page"
      role="group"
      aria-label={t("common-settings", "Settings")}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key !== "Escape" || event.defaultPrevented || layerOpen()) return;
        event.preventDefault();
        closeSettings();
      }}
    >
      <div class="st-top">
        <button type="button" class="st-back" onClick={closeSettings}>
          <Icon name="alt-arrow-left" size={14} />
          {t("settings-settings_page_back_to_app", "Back to app")}
        </button>
      </div>
      <div class="st-body">
        <Nav section={section()} />
        <div class="st-pane" role="tabpanel" id="settings-panel" aria-labelledby={`settings-tab-${section()}`}>
          <Switch>
            <Match when={section() === "general"}><Scroll><General /></Scroll></Match>
            <Match when={section() === "providers"}><Scroll><Providers /></Scroll></Match>
            <Match when={section() === "defaults"}><Scroll><Defaults /></Scroll></Match>
            <Match when={section() === "appearance"}><Scroll><Appearance /></Scroll></Match>
            <Match when={section() === "notifications"}><Scroll><Notifications /></Scroll></Match>
            <Match when={section() === "shortcuts"}><Scroll><Shortcuts /></Scroll></Match>
            <Match when={section() === "devices"}><Scroll><DevicesSettings /></Scroll></Match>
            <Match when={section() === "plugins" || section() === "cloud"}><Plugins /></Match>
            <Match when={section() === "archived"}><Archived /></Match>
          </Switch>
        </div>
      </div>
    </div>
  );
}

/** Saved chats on the connected computers, and Cloud archives from the account. */
function Archived() {
  const [tab, setTab] = createSignal<"local" | "cloud">("local");
  return (
    <div class="st-archived">
      <div class="st-archived-tabs">
        <Segmented
          label="Archive"
          size="sm"
          value={tab()}
          options={[
            { value: "local", label: "Saved chats" },
            { value: "cloud", label: "Cloud archives" },
          ]}
          onChange={setTab}
        />
      </div>
      <Show when={tab() === "local"} fallback={<Scroll><CloudArchives /></Scroll>}>
        <ProjectArchive />
      </Show>
    </div>
  );
}

function Scroll(props: { children: JSX.Element }) {
  return (
    <div class="st-scroll">
      <div class="st-column">{props.children}</div>
    </div>
  );
}

function Nav(props: { section: SectionId }) {
  const buttons: HTMLButtonElement[] = [];
  const move = (event: KeyboardEvent, index: number) => {
    const last = SECTIONS.length - 1;
    const next =
      event.key === "ArrowDown" || event.key === "ArrowRight"
        ? (index + 1) % SECTIONS.length
        : event.key === "ArrowUp" || event.key === "ArrowLeft"
          ? (index + last) % SECTIONS.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : -1;
    if (next < 0) return;
    event.preventDefault();
    openSection(SECTIONS[next]!.id);
    buttons[next]?.focus();
  };
  return (
    <nav class="st-nav" role="tablist" aria-orientation="vertical" aria-label={t("native-settings_sections", "Settings sections")}>
      <For each={SECTIONS}>
        {(item, index) => (
          <button
            ref={(el) => (buttons[index()] = el)}
            type="button"
            role="tab"
            id={`settings-tab-${item.id}`}
            class="st-tab"
            aria-selected={props.section === item.id}
            aria-controls="settings-panel"
            aria-label={item.label()}
            title={item.label()}
            tabIndex={props.section === item.id ? 0 : -1}
            onClick={() => openSection(item.id)}
            onKeyDown={(event) => move(event, index())}
          >
            <Icon name={item.icon} size={14} />
            <span class="st-tab-label">{item.label()}</span>
          </button>
        )}
      </For>
    </nav>
  );
}
