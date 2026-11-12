// Test-only frame for projects.e2e.mjs: the core plus this area, so other areas cannot interfere.
import { render } from "solid-js/web";
import { Match, Switch } from "solid-js";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/features/settings/settings.css";
import "@/shell/theme";
import "@/core/chat";
import { start } from "@/core/connection";
import { state } from "@/core/store";
import * as viewModule from "@/core/view";
import { NewProject, ProjectArchive, ProjectActions, ProjectCreationNotice } from "@/features/projects";
import { ToastRegion } from "@/ui/primitives";
import * as actions from "@/features/projects/actions";
import * as setupModule from "@/features/projects/setup";

const { view } = viewModule;
(window as unknown as { __zuse: unknown }).__zuse = { state, view, viewModule, actions, setupModule };

function Frame() {
  const chat = () => (view.selectedChatId ? state.chats[view.selectedChatId] : undefined);
  return (
    <div style={{ display: "flex", height: "100%", background: "var(--bg)" }}>
      <aside style={{ width: "232px", background: "var(--shell)" }} />
      <main style={{ flex: 1, "min-width": 0, position: "relative", display: "flex", "flex-direction": "column" }}>
        <Switch>
          <Match when={view.route.kind === "newProject"}><NewProject /></Match>
          <Match when={view.route.kind === "archive"}><ProjectArchive /></Match>
          <Match when={true}>
            <div style={{ padding: "16px", display: "flex", gap: "24px" }}>
              <div style={{ width: "300px" }} id="actions-host"><ProjectActions /></div>
              <div style={{ flex: 1 }} id="notice-host"><ProjectCreationNotice spaceId={chat()?.spaceId ?? Object.values(state.spaces).find((s) => s.path === view.draftProjectPath)?.id} /></div>
            </div>
          </Match>
        </Switch>
      </main>
      <ToastRegion />
    </div>
  );
}
render(() => <Frame />, document.getElementById("root")!);
void start();
