// Test-only stand-in for the Settings export, used by e2e/plugins-cloud.vite.config.mjs: it mounts the plugins-cloud
// sections for the plugins and cloud routes as Settings does, and exposes `window.__nav` so tests can open routes the
// signed-out sidebar does not offer.
import { Show } from "solid-js";
import { navigate, view, type Route } from "@/core/view";
import { Plugins } from "@/features/plugins-cloud";

(window as unknown as { __nav: (route: Route) => void }).__nav = navigate;

export function Settings() {
  const section = () => (view.route.kind === "settings" ? view.route.section : undefined);
  return (
    <Show when={section() === "plugins" || section() === "cloud"} fallback={<div class="unfinished">Settings stub: {section()}</div>}>
      <Plugins />
    </Show>
  );
}

export function Usage() {
  return <div class="unfinished">Usage stub</div>;
}
