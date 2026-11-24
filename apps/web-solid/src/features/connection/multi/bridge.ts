// Same-origin navigation bridge. Every computer retains an isolated app runtime and RPC client.
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { navigate, view } from "@/core/view";
import { composer } from "@/features/composer/drafts";
import { newChat, newProject, selectChat, setArchived } from "@/features/sidebar/actions";
import { projectKey as localProjectKey } from "@/features/sidebar/model";
import { hasUnsavedFiles } from "@/features/workspace/files/model";
import { createEffect, createRoot, createSignal } from "solid-js";
import { applyComputerSnapshot, frameId, inComputerFrame, multiMode, type Snapshot } from "./model";
export type ComputerAction = { kind: "chat"; chat: string } | { kind: "new"; project?: string } | { kind: "newProject" } | { kind: "settings"; section?: string } | { kind: "archive"; chat: string; archived: boolean };
export type GlobalKey = { kind: "search" } | { kind: "cycle"; forward: boolean } | { kind: "jump"; slot: number } | { kind: "sidebar" };
let keyHandler: ((key: GlobalKey) => void) | null = null;
export function registerGlobalKeys(handler: (key: GlobalKey) => void) { keyHandler = handler; }
export function dispatchComputerAction(action: ComputerAction) {
  if (action.kind === "chat") { if (state.chats[action.chat]) selectChat(action.chat); }
  else if (action.kind === "new") {
    const space = action.project ? state.spaces[action.project] : undefined;
    newChat(space ? localProjectKey(space) : undefined);
  } else if (action.kind === "newProject") newProject();
  else if (action.kind === "settings") navigate({ kind: "settings", section: action.section });
  else if (action.kind === "archive" && state.chats[action.chat]) setArchived(action.chat, action.archived);
}
function globalKey(event: KeyboardEvent): GlobalKey | null {
  const mod = /Mac|iPhone|iPad/.test(navigator.platform) ? event.metaKey : event.ctrlKey;
  if (mod && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") return { kind: "search" };
  if (event.ctrlKey && event.key === "Tab" && !event.altKey && !event.metaKey) return { kind: "cycle", forward: !event.shiftKey };
  if (mod && !event.shiftKey && /^[1-9]$/.test(event.key)) return { kind: "jump", slot: Number(event.key) - 1 };
  if (mod && !event.shiftKey && event.key.toLowerCase() === "b") return { kind: "sidebar" };
  return null;
}
let installed = false;
export function installComputerBridge() {
  if (installed) return;
  installed = true;
  const frame = frameId();
  window.addEventListener("message", event => {
    if (!inComputerFrame() || event.source !== parent || event.origin !== location.origin || event.data?.type !== "zuse.computer.action" || event.data.frame !== frame) return;
    const action = event.data.action;
    if (action && ["chat", "new", "newProject", "settings", "archive"].includes(action.kind)) dispatchComputerAction(action as ComputerAction);
  });
  window.addEventListener("keydown", event => {
    if (!inComputerFrame() && !multiMode()) return;
    const key = globalKey(event);
    if (!key) return;
    event.preventDefault(); event.stopImmediatePropagation();
    if (inComputerFrame()) parent.postMessage({ type: "zuse.computer.key", frame, key }, location.origin);
    else keyHandler?.(key);
  }, true);
  const [catalog, setCatalog] = createSignal<Snapshot["catalog"]>();
  let catalogBusy = false;
  const refreshCatalog = async () => {
    const subject = ((state.account?.user ?? {}) as { id?: string }).id;
    if (!subject || state.connection.kind !== "connected" || catalogBusy) return;
    const epoch = state.epoch;
    catalogBusy = true;
    try {
      const reply = await rpc.call<{ environments: { environmentId: string }[] }>(M.LIST_ACCOUNT_ENVIRONMENTS);
      if (state.epoch === epoch && ((state.account?.user ?? {}) as { id?: string }).id === subject) setCatalog({ subject, environments: reply.environments.map(value => value.environmentId) });
    } catch { /* A failed read never removes a saved computer. */ }
    finally { catalogBusy = false; }
  };
  setInterval(() => void refreshCatalog(), 15000);
  createRoot(() => {
    createEffect(() => [state.connection.kind, state.epoch, ((state.account?.user ?? {}) as { id?: string }).id] as const, () => { void refreshCatalog(); });
    createEffect(() => {
      const device = state.localDeviceId ? state.devices[state.localDeviceId] : undefined;
      const subject = ((state.account?.user ?? {}) as { id?: string }).id;
      const snapshot: Snapshot = {
        deviceId: state.localDeviceId, label: device?.name ?? "", connected: state.connection.kind === "connected", synced: state.workspaceSynced,
        selected: view.selectedChatId, spaces: Object.values(state.spaces), chats: Object.values(state.chats), sessions: state.sessions,
        dirty: hasUnsavedFiles() || Object.values(composer.drafts).some(draft => !!draft.text.trim()) || Object.values(composer.attachments).some(items => items.length > 0),
        catalog: catalog()?.subject === subject ? catalog() : undefined,
      };
      return JSON.stringify(snapshot);
    }, text => {
      const snapshot = JSON.parse(text) as Snapshot;
      if (inComputerFrame()) parent.postMessage({ type: "zuse.computer.snapshot", frame, snapshot }, location.origin);
      else applyComputerSnapshot("primary", snapshot);
    });
  });
}
