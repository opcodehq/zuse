// Connection lifecycle: authenticate, open a transport, start the standing watches, reconnect.
import { batch } from "solid-js";
import { checkSession, unlockFromLink } from "./auth";
import { M } from "./methods";
import { RpcClient, type Subscription } from "./rpc";
import { applySnapshot, setState, state, type ConnectionStatus } from "./store";
import { OwnerTransport, type Transport } from "./transport";

export const rpc = new RpcClient();

/** Opens a transport for the current target. Replaced by the paired transport when a pairing is selected. */
export type TransportFactory = () => Transport | Promise<Transport>;
let factory: TransportFactory = () => new OwnerTransport();
/** Paired and account transports authenticate themselves; only the owner socket needs the session cookie. */
let ownerSession = true;
export function setTransportFactory(next: TransportFactory, options: { ownerSession?: boolean } = {}) {
  factory = next;
  ownerSession = options.ownerSession ?? true;
}

/** Thrown by a transport that cannot connect until the user acts, such as a revoked pairing. */
export class ConnectionBlocked extends Error {
  constructor(readonly status: Exclude<ConnectionStatus, { kind: "connected" | "connecting" | "reconnecting" }>) {
    super(status.message ?? status.kind);
  }
}
let unblock: (() => void) | null = null;
/** Retry now: ends a blocked wait or a reconnect delay. */
export function retryConnection() {
  unblock?.();
}

type Watch = (epoch: number) => Subscription | void;
const watches = new Set<Watch>();
let active: Subscription[] = [];
const ranAt = new WeakMap<Watch, number>();
/** Starts a watch for this connection once, however it was reached. */
function run(watch: Watch, epoch: number) {
  if (ranAt.get(watch) === epoch) return;
  ranAt.set(watch, epoch);
  const sub = watch(epoch);
  if (sub) active.push(sub);
}

/** Register work that runs on every (re)connection; its subscription is cancelled on disconnect. */
export function onConnected(watch: Watch): () => void {
  watches.add(watch);
  if (state.connection.kind === "connected") {
    // Modules register at top level; a module loaded after connecting must finish evaluating before its watch runs.
    const epoch = state.epoch;
    queueMicrotask(() => {
      if (watches.has(watch) && state.connection.kind === "connected" && state.epoch === epoch) run(watch, epoch);
    });
  }
  return () => watches.delete(watch);
}

const delays = [250, 500, 1000, 2000, 4000, 8000];
let started = false;

export async function start() {
  if (started) return;
  started = true;
  const unlocked = await unlockFromLink().catch(() => false);
  if (!unlocked) setState("connection", { kind: "authRequired", message: "This access link is no longer valid." });
  void loop();
}

async function loop() {
  for (let attempt = 0; ; attempt++) {
    if (ownerSession && (await checkSession()) === "signedOut") {
      setState("connection", { kind: "authRequired", message: "Open the access link printed by zuse-server." });
      await sleep(8000);
      continue;
    }
    try {
      const transport = await factory();
      await transport.open();
      const lost = new Promise<string>((resolve) => (transport.onClose = resolve));
      rpc.attach(transport);
      attempt = 0;
      batch(() => {
        setState("epoch", (epoch) => epoch + 1);
        setState("connection", { kind: "connected" });
      });
      active = [];
      for (const watch of [...watches]) run(watch, state.epoch);
      const reason = await lost;
      for (const sub of active) sub.cancel();
      active = [];
      const uncertain = rpc.detach(reason);
      batch(() => {
        if (uncertain > 0) setState("uncertainMutation", true);
        setState("connection", { kind: "reconnecting", attempt: 0, message: reason });
      });
    } catch (error) {
      if (error instanceof ConnectionBlocked) {
        setState("connection", error.status);
        await new Promise<void>((resolve) => (unblock = resolve));
        unblock = null;
        attempt = -1;
        continue;
      }
      setState("connection", {
        kind: "reconnecting",
        attempt,
        message: error instanceof Error ? error.message : "Could not reach the engine",
      });
      await sleep(delays[Math.min(attempt, delays.length - 1)]!);
    }
  }
}

const sleep = (ms: number) =>
  new Promise<void>((resolve) => {
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      unblock = null;
      resolve();
    }
    unblock = done;
  });

// Standing workspace watches, matching what desktop keeps open.
onConnected(() => rpc.subscribe(M.WATCH_CHATS, {}, (rows) => {
  batch(() => {
    applySnapshot("chats", rows);
    setState("workspaceSynced", true);
  });
}));
onConnected(() => rpc.subscribe(M.WATCH_SESSIONS, {}, (rows) => applySnapshot("sessions", rows)));
onConnected(() => rpc.subscribe(M.WATCH_SPACES, {}, (rows) => applySnapshot("spaces", rows)));
onConnected(() => rpc.subscribe(M.WATCH_DEVICES, {}, (rows) => applySnapshot("devices", rows)));
onConnected(() => rpc.subscribe(M.CLOUD_ACCOUNT_STATUS, {}, (status) => setState("account", status as Record<string, unknown>)));
onConnected(() => {
  void rpc.call<{ deviceId: string }>(M.LOCAL_DEVICE).then((value) => setState("localDeviceId", value.deviceId), () => {});
  void rpc.call(M.AUTH_STATUS).then((value) => setState("auth", value), () => {});
  void refreshRepos();
  void refreshHarnesses();
});

export function refreshRepos() {
  return rpc.call<unknown[]>(M.LIST_REPOS).then((repos) => setState("repos", repos as never), () => {});
}
export function refreshHarnesses() {
  return rpc.call<unknown[]>(M.LIST_HARNESSES).then((rows) => setState("harnesses", rows as never), () => {});
}
