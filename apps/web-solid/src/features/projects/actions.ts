// Host-owned project Actions for the selected chat's project (crates/ui/src/project_actions.rs, shell/actions_ui.rs).
import { createEffect, createRoot, on } from "solid-js";
import { createStore } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { setView, view } from "@/core/view";
import { errorText, target, unknownMethod } from "./hosts";
import type { ProjectAction, ProjectActionDraft, ProjectActionIcon, ProjectActionRun, ProjectActionsSnapshot } from "./types";

export const ACTION_ICONS: { icon: ProjectActionIcon; label: string }[] = [
  { icon: "play", label: "Play" },
  { icon: "test", label: "Test" },
  { icon: "lint", label: "Lint" },
  { icon: "configure", label: "Configure" },
  { icon: "build", label: "Build" },
  { icon: "debug", label: "Debug" },
];

export const actionGlyph = (icon: ProjectActionIcon) => `action-${icon}`;

export interface ActionsContext {
  key: string;
  deviceId: string;
  spaceId: string;
  chatId: string;
}

export type ActionsStatus =
  | { state: "loading" }
  | { state: "ready"; snapshot: ProjectActionsSnapshot }
  | { state: "saving"; snapshot: ProjectActionsSnapshot }
  | { state: "unavailable"; snapshot: ProjectActionsSnapshot | null; message: string }
  | { state: "unsupported" };

const keyOf = (deviceId: string, spaceId: string) => `${deviceId}\u0000${spaceId}`;

/** Actions belong to the project of the selected chat, on the computer that hosts both. */
export function actionsContext(): ActionsContext | null {
  const chat = view.selectedChatId ? state.chats[view.selectedChatId] : undefined;
  if (!chat?.spaceId) return null;
  const space = state.spaces[chat.spaceId];
  if (!space || space.deviceId !== chat.deviceId) return null;
  return { key: keyOf(chat.deviceId, space.id), deviceId: chat.deviceId, spaceId: space.id, chatId: chat.id };
}

const [cache, setCache] = createStore<Record<string, ActionsStatus>>({});

export const snapshotOf = (status: ActionsStatus | undefined) =>
  status && (status.state === "ready" || status.state === "saving" || status.state === "unavailable") ? status.snapshot : null;

export const canRun = (status: ActionsStatus | undefined) => status?.state === "ready";

export function actionsStatus(context: ActionsContext | null): ActionsStatus | undefined {
  return context ? cache[context.key] : undefined;
}

let loadGeneration = 0;
let mutationGeneration = 0;
let activeKey: string | null = null;

function markUnavailable(key: string, message: string) {
  if (unknownMethod(message)) {
    setCache({ [key]: { state: "unsupported" } });
    return;
  }
  setCache({ [key]: { state: "unavailable", snapshot: snapshotOf(cache[key]), message } });
}

export async function loadActions(context: ActionsContext) {
  const generation = ++loadGeneration;
  const current = cache[context.key];
  // Revalidation keeps what is shown; only a first load shows the loading row.
  if (current?.state !== "unsupported" && !snapshotOf(current)) setCache({ [context.key]: { state: "loading" } });
  if (!rpc.connected) {
    markUnavailable(context.key, "Engine not connected");
    return;
  }
  try {
    const snapshot = await rpc.call<ProjectActionsSnapshot>(M.LIST_PROJECT_ACTIONS, target(context.deviceId, { spaceId: context.spaceId }));
    if (generation !== loadGeneration || activeKey !== context.key) return;
    setCache({ [context.key]: { state: "ready", snapshot } });
  } catch (error) {
    if (generation !== loadGeneration || activeKey !== context.key) return;
    markUnavailable(context.key, errorText(error));
  }
}

// Declared before the effect below, which runs as soon as this module loads.
const [editorState, setEditorState] = createStore<{ value: Editor | null }>({ value: null });
export const editor = () => editorState.value;
function setEditor(value: Editor | null) {
  setEditorState("value", value);
}

// Load when the selected project changes, and again on every reconnect.
createRoot(() => {
  createEffect(
    on(
      () => actionsContext(),
      (context) => {
        const changed = (context?.key ?? null) !== activeKey;
        activeKey = context?.key ?? null;
        if (changed) {
          mutationGeneration++;
          setEditor(null);
        }
        if (context && (changed || !cache[context.key])) void loadActions(context);
      },
    ),
  );
});

onConnected(() => {
  const context = actionsContext();
  if (context) void loadActions(context);
});

// ── Editor ───────────────────────────────────────────────────────────────

export interface Editor {
  key: string;
  actionId: string | null;
  name: string;
  command: string;
  icon: ProjectActionIcon;
  runOnWorktreeCreate: boolean;
  error: string | null;
  saving: boolean;
  confirmDelete: boolean;
}

export function updateEditor(patch: Partial<Editor>) {
  if (editorState.value) setEditorState("value", { ...patch });
}

export function openEditor(action?: ProjectAction, draft?: ProjectActionDraft) {
  const context = actionsContext();
  if (!context) return;
  const source = action ?? draft;
  setEditor({
    key: context.key,
    actionId: action?.id ?? null,
    name: source?.name ?? "",
    command: source?.command ?? "",
    icon: source?.icon ?? "play",
    runOnWorktreeCreate: source?.runOnWorktreeCreate ?? false,
    error: null,
    saving: false,
    confirmDelete: false,
  });
}

export const closeEditor = () => setEditor(null);

function validate(name: string, command: string): string | null {
  if (!name) return "Action name is required";
  if ([...name].length > 80) return "Action name must not exceed 80 characters";
  if (!command) return "Action command is required";
  if (new TextEncoder().encode(command).length > 16 * 1024) return "Action command must not exceed 16384 bytes";
  return null;
}

const mutationError = (error: unknown) =>
  error instanceof RpcError && error.outcomeUnknown
    ? "The connection dropped before the computer answered. The change may have been saved; check the list before trying again."
    : errorText(error);

export async function saveEditor() {
  const draft = editorState.value;
  const context = actionsContext();
  if (!draft || draft.saving) return;
  if (!context || context.key !== draft.key) {
    updateEditor({ error: "The selected project changed" });
    return;
  }
  const name = draft.name.trim();
  const command = draft.command.trim();
  const invalid = validate(name, command);
  if (invalid) {
    updateEditor({ error: invalid });
    return;
  }
  const generation = ++mutationGeneration;
  updateEditor({ saving: true, error: null });
  const shown = snapshotOf(cache[context.key]);
  if (shown) setCache({ [context.key]: { state: "saving", snapshot: shown } });
  const action: ProjectActionDraft = { name, command, icon: draft.icon, runOnWorktreeCreate: draft.runOnWorktreeCreate };
  try {
    const snapshot = await rpc.call<ProjectActionsSnapshot>(
      M.UPSERT_PROJECT_ACTION,
      target(context.deviceId, { spaceId: context.spaceId, actionId: draft.actionId, action }),
    );
    if (generation !== mutationGeneration) return;
    setCache({ [context.key]: { state: "ready", snapshot } });
    if (editorState.value?.key === context.key) setEditor(null);
  } catch (error) {
    if (generation !== mutationGeneration) return;
    const message = mutationError(error);
    markUnavailable(context.key, errorText(error));
    if (editorState.value?.key === context.key) updateEditor({ saving: false, error: message });
  }
}

export async function deleteEditorAction() {
  const draft = editorState.value;
  const context = actionsContext();
  if (!draft || draft.saving || !context || context.key !== draft.key) return;
  const actionId = draft.actionId;
  if (!actionId) {
    setEditor(null);
    return;
  }
  const generation = ++mutationGeneration;
  updateEditor({ saving: true, error: null });
  try {
    const snapshot = await rpc.call<ProjectActionsSnapshot>(
      M.DELETE_PROJECT_ACTION,
      target(context.deviceId, { spaceId: context.spaceId, actionId }),
    );
    if (generation !== mutationGeneration) return;
    setCache({ [context.key]: { state: "ready", snapshot } });
    forgetLastAction(context.spaceId, actionId);
    if (editorState.value?.key === context.key) setEditor(null);
  } catch (error) {
    if (generation !== mutationGeneration) return;
    markUnavailable(context.key, errorText(error));
    if (editorState.value?.key === context.key) updateEditor({ saving: false, confirmDelete: false, error: mutationError(error) });
  }
}

// ── Running ──────────────────────────────────────────────────────────────

/** A terminal an action started; the workspace terminal panel attaches it (it is also listed by ListTerminals). */
export interface ActionTerminal {
  chatId: string;
  deviceId: string;
  terminalId: string;
  title: string;
  startedAt: number;
}

const [terminals, setTerminals] = createStore<{ list: ActionTerminal[] }>({ list: [] });

/** Terminals started by actions, newest last. */
export const actionTerminals = () => terminals.list;

/** The panel took ownership of this terminal. */
export function claimActionTerminal(terminalId: string) {
  setTerminals("list", (list) => list.filter((t) => t.terminalId !== terminalId));
}

const LAST_ACTIONS = "projectActionBySpace";

function lastActions(): Record<string, string> {
  const value = view.preferences[LAST_ACTIONS];
  return value && typeof value === "object" ? (value as Record<string, string>) : {};
}

export const lastActionId = (spaceId: string) => lastActions()[spaceId] ?? null;

function rememberLastAction(spaceId: string, actionId: string) {
  setView("preferences", LAST_ACTIONS, { ...lastActions(), [spaceId]: actionId });
}

function forgetLastAction(spaceId: string, actionId: string) {
  const current = lastActions();
  if (current[spaceId] !== actionId) return;
  const next = { ...current };
  delete next[spaceId];
  setView("preferences", LAST_ACTIONS, next);
}

/** The action a single Run control would start: the last one used, else the first non-setup action. */
export function preferredAction(actions: readonly ProjectAction[], spaceId: string): ProjectAction | undefined {
  const last = lastActionId(spaceId);
  return (last ? actions.find((a) => a.id === last) : undefined) ?? actions.find((a) => !a.runOnWorktreeCreate) ?? actions[0];
}

export async function runAction(action: ProjectAction): Promise<ProjectActionRun | null> {
  const context = actionsContext();
  if (!context || !canRun(cache[context.key])) return null;
  try {
    const run = await rpc.call<ProjectActionRun>(
      M.RUN_PROJECT_ACTION,
      target(context.deviceId, { spaceId: context.spaceId, chatId: context.chatId, actionId: action.id, cols: 80, rows: 24 }),
    );
    setTerminals("list", (list) => [
      ...list,
      { chatId: context.chatId, deviceId: context.deviceId, terminalId: run.terminal.id, title: run.actionName, startedAt: Date.now() },
    ]);
    rememberLastAction(context.spaceId, action.id);
    if (view.selectedChatId === context.chatId) setView("rightPanel", "terminal");
    return run;
  } catch (error) {
    markUnavailable(context.key, mutationError(error));
    return null;
  }
}

export function retryActions() {
  const context = actionsContext();
  if (context) void loadActions(context);
}
