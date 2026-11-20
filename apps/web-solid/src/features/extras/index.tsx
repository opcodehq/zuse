// Global overlays and app-wide behaviour: palette, shortcut help, chat import, voice, connection cues
// and uncaught-error reporting. Also exports the shared visual, empty, loading and error pieces.
import { registerShortcut } from "@/core/keys";
import { state } from "@/core/store";
import { navigate, setSidebarVisible, sidebarVisible } from "@/core/view";
import { lazy, Match, onCleanup, onSettled, Switch } from "solid-js";
import "./extras.css";
import { watchUncaughtErrors } from "./feedback";
import { ImportChat } from "./ImportChat";
import { startNotifications } from "./notifications";
import { closeOverlay, overlay, toggleOverlay, togglePalette } from "./overlay";
import { Palette } from "./Palette";
import { Shortcuts } from "./Shortcuts";
import { VoiceStage } from "./VoiceStage";

export { EmptyState,ErrorScreen,Skeleton } from "./feedback";
export { ImportChatButton } from "./ImportChat";
export { openImportChat,openPalette,openShortcuts } from "./overlay";
export { endVoice,startVoice,voiceCall } from "./voice";
// Agent visuals are rare; their renderer loads with the first one.
export const VisualPart = lazy(() => import("./Visual").then((m) => ({ default: m.VisualPart })));

/** Shortcuts only act once the frame is on screen, not over sign-in or pairing. */
const frameReady = () => state.connection.kind === "connected" || state.workspaceSynced;

function registerAppShortcuts() {
  const when = (run: () => void) => () => frameReady() && run();
  // Desktop defaults and labels (crates/protocol/src/keybindings.rs). Mod+N and Mod+T belong to the browser and can't be taken.
  return [
    registerShortcut({ keys: "mod+k", label: "Open chat switcher", inInputs: true, run: when(() => togglePalette("all")) }),
    registerShortcut({ keys: "mod+p", label: "Search files", inInputs: true, run: when(() => togglePalette("files")) }),
    registerShortcut({ keys: "mod+/", label: "Show keyboard shortcuts", inInputs: true, run: when(() => toggleOverlay("shortcuts")) }),
    registerShortcut({
      keys: "mod+,",
      label: "Settings",
      inInputs: true,
      run: when(() => {
        closeOverlay(false);
        navigate({ kind: "settings" });
      }),
    }),
    registerShortcut({
      keys: "mod+o",
      label: "Open project",
      run: when(() => {
        closeOverlay(false);
        navigate({ kind: "newProject" });
      }),
    }),
    registerShortcut({
      keys: "mod+b",
      label: "Toggle projects panel",
      inInputs: true,
      run: when(() => setSidebarVisible(!sidebarVisible())),
    }),
  ];
}

export function Extras() {
  const stopNotifications = startNotifications();
  onCleanup(stopNotifications);
  onSettled(() => {
    const cleanups: (() => void)[] = [];
    const stops = [watchUncaughtErrors(), ...registerAppShortcuts()];
    cleanups.push(() => stops.forEach((stop) => stop()));

    return () => cleanups.forEach(cleanup => cleanup());
  });
  return (
    <>
      <Switch>
        <Match when={overlay() === "palette"}><Palette /></Match>
        <Match when={overlay() === "shortcuts"}><Shortcuts /></Match>
        <Match when={overlay() === "import"}><ImportChat /></Match>
      </Switch>
      <VoiceStage />
    </>
  );
}
