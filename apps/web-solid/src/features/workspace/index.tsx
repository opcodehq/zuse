// Workspace area: the conversation top bar and the right panel's surfaces.
import "./workspace.css";
import { registerShortcut } from "@/core/keys";
import { view } from "@/core/view";
import { openPanel, toggleRightPanel } from "./store";

export { TopBar } from "./TopBar";
export { RightPanel } from "./RightPanel";

const inChat = () => view.route.kind === "chat" && !!view.selectedChatId;

// Desktop defaults (crates/protocol/src/keybindings.rs, crates/ui/src/settings.rs).
registerShortcut({
  keys: "mod+j",
  label: "Toggle terminal",
  inInputs: true,
  run: () => inChat() && (view.rightPanel === "terminal" ? toggleRightPanel() : openPanel("terminal")),
});
registerShortcut({
  keys: "mod+e",
  label: "Toggle files panel",
  inInputs: true,
  run: () => inChat() && (view.rightPanel === "files" ? toggleRightPanel() : openPanel("files")),
});
registerShortcut({
  keys: "mod+alt+b",
  label: "Toggle right panel",
  inInputs: true,
  run: () => inChat() && toggleRightPanel(),
});
