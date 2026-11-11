// Plugin management, ported from crates/protocol/src/plugin_state.rs: writes are reconciled by reads, never replayed.
import { createEffect, createRoot, on } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { errorText, signIn, signedIn, user } from "../account";
import { catalog, nextLabel, type PluginAttempt, type PluginDefinition, type PluginSnapshot } from "./catalog";
import { connectRequest, type PluginRequest } from "./setup";

export type RemovalKind = "connection" | "server";
export interface Confirmation {
  kind: RemovalKind;
  label: string;
  tenant: string;
  id: string;
}
/** An open setup form. Catalog keys render inline on the detail page; servers open a dialog. */
export type SetupTarget = { kind: "catalog"; plugin: PluginDefinition; label: string } | { kind: "server" };

interface PluginsState {
  snapshot: PluginSnapshot | null;
  attempt: PluginAttempt | null;
  attemptTenant: string | null;
  confirmation: Confirmation | null;
  notice: string | null;
  error: string | null;
  busy: boolean;
  setup: SetupTarget | null;
  /** Catalog id whose detail page is open. */
  selected: string | null;
}

const [plugins, setPlugins] = createStore<PluginsState>({
  snapshot: null,
  attempt: null,
  attemptTenant: null,
  confirmation: null,
  notice: null,
  error: null,
  busy: false,
  setup: null,
  selected: null,
});
export { plugins };

// Request bookkeeping that never renders.
let pending: { action: string; tenant: string | null } | null = null;
let selectionUncertain = false;
let generation = 0;
let poll: ReturnType<typeof setTimeout> | undefined;

const ACTIONS = new Set(["list", "connect", "addServer", "addLocalServer", "removeServer", "poll", "cancel", "complete", "disconnect", "setEnabled"]);

export const tenant = () => plugins.snapshot?.tenantId || null;
export const canManage = () => plugins.snapshot?.canManage !== false;
export const activeAttempt = () =>
  plugins.attempt?.state === "pending" && typeof plugins.attempt.id === "string" && plugins.attemptTenant === tenant();
/** Writes are unavailable while one is in flight or a hosted sign-in is open. */
export const locked = () => plugins.busy || activeAttempt() || !canManage();

const listRequest = (id: string | null): PluginRequest => (id ? { action: "list", tenantId: id } : { action: "list" });
const refreshRequest = () => listRequest(selectionUncertain ? null : tenant());

function begin(input: PluginRequest, selectingWorkspace = false): PluginRequest | string {
  if (selectingWorkspace) {
    if (activeAttempt()) return "Finish or cancel the current plugin sign-in first.";
    if (input.action !== "list" || typeof input.tenantId !== "string" || !input.tenantId) return "Choose a plugin workspace.";
  }
  if (pending) return "Wait for the current plugin action to finish.";
  const action = input.action;
  if (!ACTIONS.has(action)) return "Choose a supported plugin action.";
  const request = { ...input };
  if (action === "list" && request.tenantId == null && !selectionUncertain && tenant()) request.tenantId = tenant();
  const target = typeof request.tenantId === "string" ? request.tenantId : null;
  if (action !== "list") {
    if (!canManage()) return "Only workspace administrators can manage these connections.";
    if (!target || target !== tenant()) return "Refresh the selected workspace before changing plugins.";
  }
  if (activeAttempt() && ((action === "list" && target !== tenant()) || !["list", "poll", "cancel", "complete"].includes(action))) {
    return "Finish or cancel the current plugin sign-in first.";
  }
  if ((action === "poll" || action === "cancel") && (!activeAttempt() || request.attemptId !== plugins.attempt?.id)) {
    return "This plugin sign-in is no longer active. Refresh connections.";
  }
  if (action !== "removeServer" && action !== "list") setPlugins("confirmation", null);
  if (action !== "list" && action !== "poll") setPlugins("notice", null);
  pending = { action: selectingWorkspace ? "selectWorkspace" : action, tenant: target };
  return request;
}

function removable(kind: RemovalKind, id: string) {
  const snapshot = plugins.snapshot;
  if (!snapshot) return null;
  return kind === "connection"
    ? snapshot.connections.find((row) => row.id === id) ?? null
    : snapshot.catalog.find((row) => row.id === id && row.custom === true) ?? null;
}

/** Applies a response; returns the follow-up read it requires. */
function apply(response: unknown): PluginRequest | null {
  const current = pending;
  if (!current) throw new Error("No plugin request is awaiting a response.");
  const kind = (response as { kind?: unknown } | null)?.kind;
  if (kind === "snapshot") {
    const snapshot = response as PluginSnapshot;
    if (typeof snapshot.tenantId !== "string" || !Array.isArray(snapshot.catalog) || !Array.isArray(snapshot.connections) || !Array.isArray(snapshot.tenants)) {
      throw new Error("The plugin service returned an invalid response. Refresh connections.");
    }
    if (!["list", "selectWorkspace"].includes(current.action) || (current.tenant && current.tenant !== snapshot.tenantId)) {
      throw new Error("The plugin service returned a different workspace. Refresh connections.");
    }
    const changed = tenant() !== snapshot.tenantId;
    if (changed || plugins.attempt?.state !== "pending") setPlugins({ attempt: null, attemptTenant: null });
    if (changed) setPlugins("notice", null);
    selectionUncertain = false;
    setPlugins("snapshot", reconcile(snapshot, { key: "id", merge: true }));
    const confirmation = plugins.confirmation;
    if (confirmation && (confirmation.tenant !== snapshot.tenantId || !removable(confirmation.kind, confirmation.id))) setPlugins("confirmation", null);
    pending = null;
    return null;
  }
  if (kind === "attempt") {
    const attempt = response as PluginAttempt;
    if (!["connect", "addServer", "addLocalServer", "poll", "complete"].includes(current.action) || !["pending", "connected", "failed", "cancelled"].includes(attempt.state)) {
      throw new Error("The plugin service returned an invalid connection state. Refresh connections.");
    }
    if (current.action === "poll" && attempt.id !== plugins.attempt?.id) {
      throw new Error("The plugin service returned a different sign-in attempt. Refresh connections.");
    }
    // Attempts replace each other whole; a merge would keep a stale authorization link.
    setPlugins("attempt", reconcile(attempt));
    setPlugins({
      attemptTenant: current.tenant,
      notice: attempt.state === "failed" ? "Plugin connection failed. Try connecting again." : attempt.state === "cancelled" ? "Plugin sign-in canceled." : null,
    });
    pending = null;
    return attempt.state !== "pending" ? listRequest(current.tenant) : null;
  }
  if (kind === "ok") {
    if (!["removeServer", "disconnect", "setEnabled", "cancel", "complete"].includes(current.action)) {
      throw new Error("The plugin service did not confirm the requested action. Refresh connections.");
    }
    if (current.action === "removeServer") setPlugins("confirmation", null);
    if (current.action === "cancel" || current.action === "complete") {
      setPlugins({ attempt: null, attemptTenant: null, notice: current.action === "cancel" ? "Plugin sign-in canceled." : null });
    }
    pending = null;
    return listRequest(current.tenant);
  }
  throw new Error("The plugin service returned an invalid response. Refresh connections.");
}

function failed(): PluginRequest | null {
  const current = pending;
  pending = null;
  if (!current) return null;
  if (current.action === "selectWorkspace") {
    selectionUncertain = true;
    setPlugins("notice", "Workspace selection could not be confirmed. Refreshing the saved selection.");
    return refreshRequest();
  }
  if (current.action === "list") return null;
  setPlugins("notice", "The plugin action could not be confirmed. Check connections before trying again.");
  return listRequest(current.tenant);
}

/** `followUp` reads reconcile a write; they keep the write's error visible instead of clearing it. */
async function request(input: PluginRequest, selectingWorkspace = false, followUp = false) {
  if (plugins.busy) return;
  const prepared = begin(input, selectingWorkspace);
  if (typeof prepared === "string") {
    setPlugins("error", prepared);
    return;
  }
  const mine = generation;
  setPlugins(followUp ? { busy: true, setup: null } : { busy: true, error: null, setup: null });
  clearTimeout(poll);
  let next: PluginRequest | null = null;
  try {
    const value = await rpc.call(selectingWorkspace ? M.SELECT_PLUGIN_WORKSPACE : M.PLUGINS, prepared);
    if (mine !== generation) return;
    try {
      next = apply(value);
    } catch (error) {
      setPlugins("error", errorText(error));
      next = failed();
    }
  } catch (error) {
    if (mine !== generation) return;
    setPlugins("error", errorText(error));
    next = failed();
  }
  setPlugins("busy", false);
  // While disconnected the follow-up read waits for the reconnect refresh instead of failing again.
  if (next) {
    if (rpc.connected) void request(next, false, true);
  } else schedulePoll();
}

function schedulePoll() {
  clearTimeout(poll);
  if (!activeAttempt() || plugins.busy) return;
  const mine = generation;
  const id = plugins.attempt!.id;
  const target = tenant();
  poll = setTimeout(() => {
    if (mine === generation && signedIn() && !plugins.busy && activeAttempt() && plugins.attempt?.id === id && tenant() === target) {
      void request({ action: "poll", tenantId: target, attemptId: id });
    }
  }, 1500);
}

function reset() {
  generation++;
  pending = null;
  selectionUncertain = false;
  clearTimeout(poll);
  setPlugins({ snapshot: null, attempt: null, attemptTenant: null, confirmation: null, notice: null, error: null, busy: false, setup: null, selected: null });
}

export function refresh() {
  if (signedIn()) void request(refreshRequest());
}

createRoot(() => {
  createEffect(
    on(
      () => user()?.id ?? null,
      (id, previous) => {
        if (id === previous) return;
        reset();
        if (id) refresh();
      },
    ),
  );
});
// A reconnect drops in-flight work; the refreshed read shows whether it landed.
onConnected(() => {
  if (pending) {
    pending = null;
    setPlugins({ busy: false, notice: "Connection interrupted. Check refreshed connections before trying an action again.", confirmation: null });
  }
  generation++;
  setPlugins("busy", false);
  refresh();
});

export function definition(id: string | null) {
  return id ? catalog(plugins.snapshot).find((p) => p.id === id) ?? null : null;
}

/** Signed out: sign in first. Keyed plugins collect the key inline; the rest start hosted sign-in. */
export function connect(plugin: PluginDefinition) {
  if (!signedIn()) {
    void signIn();
    return;
  }
  if (plugins.busy || !canManage()) return;
  if (activeAttempt()) {
    setPlugins("error", "Finish or cancel the current plugin sign-in first.");
    return;
  }
  const label = nextLabel(plugins.snapshot, plugin);
  const target = tenant();
  if (!target) return;
  if (plugin.apiKeyHeader) {
    setPlugins({ selected: plugin.id, setup: { kind: "catalog", plugin, label } });
    return;
  }
  const result = connectRequest(target, plugin, label, null);
  if ("ok" in result) void request(result.ok);
}

export function openServerSetup() {
  if (!signedIn()) {
    void signIn();
    return;
  }
  if (activeAttempt()) {
    setPlugins("error", "Finish or cancel the current plugin sign-in first.");
    return;
  }
  if (plugins.busy || !canManage() || !tenant()) return;
  setPlugins("setup", { kind: "server" });
}

export const closeSetup = () => setPlugins("setup", null);

/** Submits a validated setup request if the workspace it was built for is still current. */
export function submitSetup(prepared: PluginRequest) {
  if (plugins.busy || !signedIn() || prepared.tenantId !== tenant() || !canManage()) return;
  void request(prepared);
}

export function setEnabled(connectionId: string, enabled: boolean) {
  if (locked()) return;
  void request({ action: "setEnabled", tenantId: tenant(), connectionId, enabled });
}

export function disconnect(connectionId: string) {
  if (locked()) return;
  void request({ action: "disconnect", tenantId: tenant(), connectionId });
}

export function cancelAttempt() {
  if (plugins.busy || !plugins.attempt) return;
  void request({ action: "cancel", tenantId: tenant(), attemptId: plugins.attempt.id });
}

export function selectWorkspace(id: string) {
  void request({ action: "list", tenantId: id }, true);
}

export function askRemoval(kind: RemovalKind, id: string) {
  if (pending || plugins.busy || activeAttempt()) return setPlugins("error", "Finish the current plugin action first.");
  if (!canManage()) return setPlugins("error", "Only workspace administrators can manage these connections.");
  const target = tenant();
  if (!target) return setPlugins("error", "Choose a workspace first.");
  const row = removable(kind, id);
  if (!row) return setPlugins("error", "This connection or custom server is no longer available.");
  const label = ("label" in row ? row.label : row.name) || "Plugin";
  setPlugins("confirmation", { kind, label, tenant: target, id });
}

export const dismissRemoval = () => setPlugins("confirmation", null);

export function confirmRemoval() {
  if (pending || plugins.busy) return setPlugins("error", "Wait for the current plugin action to finish.");
  const confirmed = plugins.confirmation;
  if (!confirmed) return setPlugins("error", "Choose a connection or custom server to remove.");
  if (!canManage() || tenant() !== confirmed.tenant || !removable(confirmed.kind, confirmed.id)) {
    setPlugins({ confirmation: null, error: "Connections changed. Review them before removing anything." });
    return;
  }
  const next: PluginRequest =
    confirmed.kind === "connection"
      ? { action: "disconnect", tenantId: confirmed.tenant, connectionId: confirmed.id }
      : { action: "removeServer", tenantId: confirmed.tenant, pluginId: confirmed.id };
  // Server removal stays open until confirmed so a lost reply can be retried from the same dialog.
  if (confirmed.kind !== "server") setPlugins("confirmation", null);
  void request(next);
}

export function openDetail(id: string | null) {
  setPlugins({ selected: id, setup: null });
}

export const clearError = () => setPlugins("error", null);
