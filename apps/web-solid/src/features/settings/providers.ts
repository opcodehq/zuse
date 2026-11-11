// Providers page actions: enable, install, updates. Replies refresh the shared harness list in the store.
import { createStore } from "solid-js/store";
import { M } from "@/core/methods";
import { onConnected, rpc } from "@/core/connection";
import { data, harnessList, message, setHarnesses, uncertain } from "./data";
import { descriptorEnabled, type HarnessDescriptor, type UpdatePolicy } from "./harness";

interface ProvidersState {
  /** A toggle the engine hasn't answered yet: the switch shows it at once. */
  pendingToggle: { harness: string; enabled: boolean } | null;
  installing: string | null;
  checking: boolean;
  updateAction: string | null;
  error: string | null;
  listError: string | null;
  loading: boolean;
}

export const [providers, setProviders] = createStore<ProvidersState>({
  pendingToggle: null,
  installing: null,
  checking: false,
  updateAction: null,
  error: null,
  listError: null,
  loading: false,
});

onConnected(() => {
  setProviders({ pendingToggle: null, installing: null, checking: false, updateAction: null, error: null });
});

const failure = (error: unknown, prefix?: string) =>
  uncertain(error) ? "The connection dropped before the engine answered. Check the current state before trying again." : prefix ? `${prefix} — ${message(error)}` : message(error);

export async function loadHarnesses() {
  setProviders({ loading: true });
  try {
    setHarnesses(await rpc.call(M.LIST_HARNESSES));
    setProviders({ listError: null });
  } catch (error) {
    setProviders({ listError: message(error) });
  } finally {
    setProviders({ loading: false });
  }
}

/** Mock is a development rig; hide it whenever a real agent exists. */
export function visibleHarnesses(): HarnessDescriptor[] {
  const list = harnessList();
  const real = list.filter((d) => d.id !== "mock");
  return real.length ? real : list;
}

export const effectiveEnabled = (d: HarnessDescriptor) =>
  providers.pendingToggle?.harness === d.id ? providers.pendingToggle.enabled : descriptorEnabled(d);

export async function setEnabled(harness: string, enabled: boolean) {
  setProviders({ error: null, pendingToggle: { harness, enabled } });
  try {
    setHarnesses(await rpc.call(M.SET_HARNESS_ENABLED, { harness, enabled }));
  } catch (error) {
    setProviders({ error: failure(error) });
  } finally {
    if (providers.pendingToggle?.harness === harness && providers.pendingToggle.enabled === enabled) setProviders({ pendingToggle: null });
  }
}

export async function install(harness: string) {
  if (providers.installing) return;
  setProviders({ installing: harness, error: null });
  try {
    setHarnesses(await rpc.call(M.INSTALL_HARNESS, { harness }));
  } catch (error) {
    setProviders({ error: failure(error, "Installation failed") });
  } finally {
    setProviders({ installing: null });
  }
}

export async function cancelInstall() {
  const harness = providers.installing;
  if (!harness) return;
  try {
    await rpc.call(M.CANCEL_INSTALL, { harness });
  } catch (error) {
    setProviders({ error: `Cancellation failed — ${message(error)}` });
  }
}

export async function checkUpdates() {
  if (providers.checking) return;
  setProviders({ checking: true, error: null });
  try {
    await rpc.call(M.CHECK_HARNESS_UPDATES, {});
  } catch (error) {
    setProviders({ error: failure(error, "Couldn't check for updates") });
  } finally {
    setProviders({ checking: false });
  }
}

async function updateAction(method: string, harness: string, extra: Record<string, unknown> = {}) {
  if (providers.updateAction && method !== M.CANCEL_HARNESS_UPDATE) return;
  setProviders({ updateAction: harness, error: null });
  try {
    await rpc.call(method, { harness, ...extra });
  } catch (error) {
    setProviders({ error: failure(error) });
  } finally {
    if (providers.updateAction === harness) setProviders({ updateAction: null });
  }
}

export const applyUpdate = (harness: string) => updateAction(M.APPLY_HARNESS_UPDATE, harness);
export const cancelUpdate = (harness: string) => updateAction(M.CANCEL_HARNESS_UPDATE, harness);
export const setPolicy = (harness: string, policy: UpdatePolicy) => updateAction(M.SET_HARNESS_UPDATE_POLICY, harness, { policy });

export const updateFor = (harness: string) => data.updates?.find((u) => u.harness === harness);

export function newestCheck() {
  let newest: number | undefined;
  for (const row of data.updates ?? []) if (row.checkedAt && (!newest || row.checkedAt > newest)) newest = row.checkedAt;
  return newest;
}
