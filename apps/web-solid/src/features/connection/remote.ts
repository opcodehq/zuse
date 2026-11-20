// Remote access data for Settings → Devices. Area-only: read from the engine on demand, never cached
// across connections, and refreshed after every action instead of being patched locally.
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { createEffect, createRoot, createStore, reconcile } from "solid-js";

export interface Peer {
  id: string;
  label: string;
  createdAt: number;
  expiresAt: number | null;
  pairedAt: number | null;
}

export interface Invitation {
  /** Opens this computer in a web browser. Shown once, never stored. */
  browserLink: string | null;
  /** For Zuse apps, when it differs from the browser link (a plain LAN or relay address). */
  appLink: string | null;
  /** The browser link uses this computer's generated certificate, which browsers warn about once. */
  selfSigned: boolean;
}

type Json = Record<string, unknown>;

interface Remote {
  network: Json | null;
  networkKnown: boolean;
  relay: Json | null;
  direct: Json | null;
  host: Json | null;
  peers: Peer[];
  environments: Json[];
  saved: Json[];
  accountDevices: Json[];
  invitation: Invitation | null;
  busy: string | null;
  /** The last action's failure; a refresh never clears it. */
  error: string | null;
  loadError: string | null;
  loading: boolean;
}

export const [remote, setRemote] = createStore<Remote>({
  network: null,
  networkKnown: false,
  relay: null,
  direct: null,
  host: null,
  peers: [],
  environments: [],
  saved: [],
  accountDevices: [],
  invitation: null,
  busy: null,
  error: null,
  loadError: null,
  loading: false,
});

const message = (error: unknown) => (error instanceof Error ? error.message : "Request failed");
const signedIn = () => typeof (state.account as { user?: unknown } | null)?.user === "object" && !!(state.account as { user?: unknown }).user;
export const accountUser = () => (state.account as { user?: { id?: string; email?: string } } | null)?.user ?? null;

let generation = 0;

/** Load everything the page shows. Older hosts without relay or direct access simply omit those cards. */
export async function refresh() {
  if (state.connection.kind !== "connected") return;
  const mine = ++generation;
  const epoch = state.epoch;
  const current = () => mine === generation && epoch === state.epoch;
  setRemote(draft => { draft["loading"] = true; });
  const settle = <T,>(promise: Promise<T>) => promise.then((value) => ({ ok: true as const, value }), (error: unknown) => ({ ok: false as const, error }));
  const [network, peers, relay, direct] = await Promise.all([
    settle(rpc.call<Json>(M.BROWSER_ACCESS_STATUS)),
    settle(rpc.call<Peer[]>(M.LIST_PAIRINGS)),
    settle(rpc.call<Json>(M.RELAY_ACCESS_STATUS)),
    settle(rpc.call<Json>(M.DIRECT_ACCESS_STATUS)),
  ]);
  if (!current()) return;
  const errors: string[] = [];
  if (network.ok) setRemote(draft => { Object.assign(draft, { network: network.value, networkKnown: true }); });
  else {
    setRemote(draft => { draft["networkKnown"] = false; });
    errors.push(message(network.error));
  }
  if (peers.ok) setRemote(draft => { { const next = Array.isArray(peers.value) ? peers.value : []; if (draft["peers"] == null) draft["peers"] = next; else reconcile(next, "id")(draft["peers"]); } });
  else errors.push(message(peers.error));
  if (relay.ok) setRemote(draft => { draft["relay"] = relay.value; });
  if (direct.ok) setRemote(draft => { draft["direct"] = direct.value; });
  if (signedIn()) {
    const [environments, saved, devices] = await Promise.all([
      settle(rpc.call<{ environments?: Json[] }>(M.LIST_ACCOUNT_ENVIRONMENTS)),
      settle(rpc.call<{ computers?: Json[] }>(M.LIST_ACCOUNT_ROUTES)),
      settle(rpc.call<{ clients?: Json[] }>(M.LIST_ACCOUNT_DEVICES)),
    ]);
    if (!current()) return;
    if (environments.ok) setRemote(draft => { { const next = environments.value.environments ?? []; if (draft["environments"] == null) draft["environments"] = next; else reconcile(next)(draft["environments"]); } });
    else errors.push(message(environments.error));
    if (saved.ok) setRemote(draft => { { const next = saved.value.computers ?? []; if (draft["saved"] == null) draft["saved"] = next; else reconcile(next)(draft["saved"]); } });
    else errors.push(message(saved.error));
    if (devices.ok) setRemote(draft => { { const next = devices.value.clients ?? []; if (draft["accountDevices"] == null) draft["accountDevices"] = next; else reconcile(next)(draft["accountDevices"]); } });
    else errors.push(message(devices.error));
  } else {
    setRemote(draft => { Object.assign(draft, { environments: [], saved: [], accountDevices: [] }); });
  }
  setRemote(draft => { Object.assign(draft, { loading: false, loadError: errors[0] ?? null }); });
}

/**
 * Run one change. A dropped connection leaves its outcome unknown, which is said plainly and
 * never retried. Returns the reply, or undefined on failure.
 */
export async function act<T = Json>(method: string, params: Json = {}): Promise<T | undefined> {
  if (remote.busy || state.connection.kind !== "connected") return undefined;
  generation++;
  setRemote(draft => { Object.assign(draft, { busy: method, error: null }); });
  try {
    const value = await rpc.call<T>(method, params);
    return value;
  } catch (error) {
    setRemote(draft => {
      draft["error"] = error instanceof RpcError && error.outcomeUnknown
        ? "The change may have completed before the connection closed. Check its current state before trying again."
        : message(error);
    });
    return undefined;
  } finally {
    setRemote(draft => { draft["busy"] = null; });
    void refresh();
  }
}

const loopback = (origin: string) => {
  try {
    return ["localhost", "127.0.0.1", "[::1]"].includes(new URL(origin).hostname);
  } catch {
    return false;
  }
};

export async function createInvitation(label: string) {
  const reply = await act<{ fragment?: string; origin?: string | null; browserOrigin?: string | null; browserSelfSigned?: boolean }>(M.CREATE_PAIRING, { label });
  if (!reply?.fragment) return;
  const join = (origin: string) => `${origin.replace(/\/$/, "")}/${reply.fragment}`;
  const app = reply.origin ?? null;
  // Browsers pair only over HTTPS or on the computer itself; plain LAN routes suit apps.
  const browser = reply.browserOrigin ?? (app && (app.startsWith("https:") || loopback(app)) ? app : null);
  if (!browser && !app) {
    setRemote(draft => { draft["error"] = "The invitation was created, but its connection URL is unavailable. Turn on one of the connections above before sharing it."; });
    return;
  }
  setRemote(draft => {
    draft["invitation"] = {
      browserLink: browser ? join(browser) : null,
      appLink: app && app !== browser ? join(app) : null,
      selfSigned: reply.browserOrigin ? reply.browserSelfSigned === true : false,
    };
  });
}

export function clearInvitation() {
  setRemote(draft => { draft["invitation"] = null; });
}

createRoot(() => {
  // A new connection or account invalidates everything shown; nothing carries over.
  createEffect(() => [state.epoch, accountUser()?.id] as const, () => {
    generation++;
    setRemote(draft => { Object.assign(draft, { environments: [], saved: [], accountDevices: [], invitation: null, busy: null, error: null, loadError: null }); });
  }, { defer: true });
});

onConnected(() => rpc.subscribe<Json>(M.WATCH_MANAGED_HOST, {}, (host) => setRemote(draft => { draft["host"] = host; }), () => setRemote(draft => { draft["host"] = null; })));
