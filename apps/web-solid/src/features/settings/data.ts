// Engine data that only Settings and Usage read. Engine replies land here; pages render from it.
import { createStore, reconcile } from "solid-js/store";
import { M } from "@/core/methods";
import { onConnected, rpc } from "@/core/connection";
import { RpcError } from "@/core/rpc";
import { setState, state } from "@/core/store";
import type { HarnessDescriptor, HarnessUpdateStatus } from "./harness";

export interface UsageWindow {
  label: string;
  usedFraction: number;
  resetsAt?: string | null;
}

export interface AgentAccount {
  id: string;
  harness: string;
  email?: string | null;
  planLabel?: string | null;
  active: boolean;
  usageWindows?: UsageWindow[];
  usageFetchedAt?: number;
  usageError?: string;
  displayName?: string;
  organization?: string;
  authKind?: "oauth" | "api-key";
  switchable?: boolean;
  provider?: string;
}

export interface AccountsSnapshot {
  accounts: AgentAccount[];
  warnings: { harness: string; message: string }[];
}

export interface CustomAcpAgent {
  id: string;
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  modelIds: string[];
}

export interface CustomAcpAgents {
  revision: number;
  agents: CustomAcpAgent[];
}

export interface RegistryAgent {
  id: string;
  name: string;
  description?: string;
  icon?: string | null;
  repository?: string | null;
  version?: string | null;
  distribution?: {
    npx?: { package: string; args?: string[]; env?: Record<string, string> } | null;
    uvx?: { package: string; args?: string[]; env?: Record<string, string> } | null;
    binary?: Record<string, { cmd: string; args?: string[]; env?: Record<string, string> }>;
    local?: { command: string; args?: string[]; env?: Record<string, string> } | null;
  };
}

export interface Catalog {
  platform: string;
  agents: RegistryAgent[];
  error?: string | null;
}

export interface TitleSettings {
  harness?: string | null;
  model?: string | null;
}

export interface WorktreeStatus {
  settings: { useCustomDirectory: boolean; customDirectory?: string | null };
  defaultDirectory: string;
  effectiveDirectory: string;
  environmentOverride?: string | null;
}

export interface AppUpdateStatus {
  currentVersion: string;
  latestVersion?: string;
  updateAvailable?: boolean;
  checkedAt?: number;
  error?: string;
}

export interface Model {
  id: string;
  label: string;
  description?: string;
}

interface SettingsData {
  updates: HarnessUpdateStatus[] | null;
  updatesError: string | null;
  appUpdate: AppUpdateStatus | null;
  accounts: AccountsSnapshot | null;
  accountsError: string | null;
  accountsLoading: boolean;
  custom: CustomAcpAgents | null;
  customError: string | null;
  models: Record<string, Model[] | undefined>;
  modelErrors: Record<string, string | undefined>;
}

export const [data, setData] = createStore<SettingsData>({
  updates: null,
  updatesError: null,
  appUpdate: null,
  accounts: null,
  accountsError: null,
  accountsLoading: false,
  custom: null,
  customError: null,
  models: {},
  modelErrors: {},
});

export const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
export const uncertain = (error: unknown) => error instanceof RpcError && error.outcomeUnknown;

onConnected(() =>
  rpc.subscribe<HarnessUpdateStatus[]>(
    M.WATCH_HARNESS_UPDATES,
    {},
    (rows) => {
      setData("updates", reconcile(Array.isArray(rows) ? rows : [], { key: "harness" }));
      setData("updatesError", null);
    },
    (error) => {
      if (error) setData("updatesError", error.message);
    },
  ),
);

onConnected(() =>
  rpc.subscribe<AppUpdateStatus>(
    M.UPDATE_STATUS,
    {},
    (status) => setData("appUpdate", status),
    () => {},
  ),
);

// A new connection may be a different computer: forget what the last one said.
onConnected(() => {
  setData({ accounts: null, accountsError: null, custom: null, customError: null, models: {}, modelErrors: {} });
});

export function setHarnesses(rows: unknown) {
  if (Array.isArray(rows)) setState("harnesses", reconcile(rows as never[], { key: "id" }));
}

export const harnessList = () => state.harnesses as unknown as HarnessDescriptor[];

export async function loadAccounts(forceUsage: boolean) {
  setData("accountsLoading", true);
  try {
    const snapshot = await rpc.call<AccountsSnapshot>(M.LIST_AGENT_ACCOUNTS, { forceUsage });
    setData("accounts", reconcile(snapshot, { key: "id" }));
    setData("accountsError", null);
  } catch (error) {
    setData("accountsError", message(error));
  } finally {
    setData("accountsLoading", false);
  }
}

export async function loadCustomAgents() {
  try {
    const snapshot = await rpc.call<CustomAcpAgents>(M.GET_CUSTOM_ACP_AGENTS, {});
    setData("custom", reconcile(snapshot, { key: "id" }));
    setData("customError", null);
  } catch (error) {
    setData("customError", message(error));
  }
}

const modelRequests = new Map<string, Promise<void>>();
export function loadModels(harness: string, force = false) {
  if (!force && (data.models[harness] || modelRequests.has(harness))) return modelRequests.get(harness) ?? Promise.resolve();
  const request = rpc
    .call<Model[]>(M.LIST_MODELS, { harness, force })
    .then(
      (models) => {
        setData("models", harness, Array.isArray(models) ? models : []);
        setData("modelErrors", harness, undefined);
      },
      (error) => {
        setData("modelErrors", harness, message(error));
      },
    )
    .finally(() => modelRequests.delete(harness));
  modelRequests.set(harness, request);
  return request;
}
