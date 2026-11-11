// Account computers: a controller engine (this tab's own engine or paired computer) issues a
// short-lived grant, and this tab speaks RPC directly to the selected computer with it.
// The tab URL carries only public metadata; the grant never leaves memory.
import { ConnectionBlocked } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcClient, RpcError } from "@/core/rpc";
import type { Transport } from "@/core/transport";
import { readAccountTarget, writeAccountTarget } from "./computers";
import type { AccountTarget } from "./status";

const PROTOCOL = "zuse-rpc-json-v1";
const REJECTED = "account_connection.rejected";
const PING_MS = 20_000;
const REPLY_MS = 10_000;

interface Grant {
  environmentId: string;
  origin: string;
  connectToken: string;
  expiresAt: number;
}

/** Validate public metadata from a `#computer=` link or session storage. */
export function parseTarget(value: unknown): AccountTarget {
  const fail = () => {
    throw new Error("Invalid account computer link");
  };
  if (!value || typeof value !== "object") fail();
  const { environmentId, expectedSubject, label, controllers } = value as Record<string, unknown>;
  // Chains through several controllers need each hop's grant; this client opens one hop.
  if (Array.isArray(controllers) && controllers.length > 0) {
    throw new Error("Return to the controller tab, then open this computer from there.");
  }
  const ok = (text: unknown, max: number): text is string =>
    typeof text === "string" && text.length > 0 && text.length <= max && !/[\p{Cc}]/u.test(text);
  if (!ok(environmentId, 256) || !ok(expectedSubject, 256) || !ok(label, 128)) fail();
  return { environmentId: environmentId as string, expectedSubject: expectedSubject as string, label: label as string };
}

/** Reads `#computer=` once, keeps it in this tab's storage and clears the URL. */
export function takeAccountTarget(): AccountTarget | null {
  const hash = location.hash;
  if (hash.startsWith("#computer=")) {
    writeAccountTarget(null);
    if (hash.length > 24_576) throw new Error("Invalid account computer link");
    const target = parseTarget(JSON.parse(decodeURIComponent(hash.slice(10))));
    writeAccountTarget(JSON.stringify(target));
    // Keep the public target in the URL when storage failed, so a reload never falls back to this engine.
    if (readAccountTarget()) history.replaceState(null, "", location.pathname + location.search);
    return target;
  }
  const saved = readAccountTarget();
  return saved ? parseTarget(JSON.parse(saved)) : null;
}

/** Open an account computer in a new tab, isolated from this one. */
export function openAccountComputer(target: AccountTarget) {
  const url = `${location.origin}${location.pathname}${location.search}#computer=${encodeURIComponent(JSON.stringify(target))}`;
  const child = window.open(url, "_blank");
  if (!child) throw new Error("Allow pop-ups to open this computer in a new tab");
  child.opener = null;
}

export function returnToController() {
  writeAccountTarget(null);
  location.replace(location.pathname + location.search);
}

function socketUrl(grant: Grant): string {
  const origin = new URL(grant.origin);
  const trimmed = grant.origin.replace(/\/$/, "");
  const now = Date.now();
  if (
    origin.origin !== trimmed ||
    !["https:", "http:"].includes(origin.protocol) ||
    !grant.connectToken ||
    grant.connectToken.length > 16_384 ||
    grant.expiresAt <= now ||
    grant.expiresAt > now + 86_400_000
  ) {
    throw new Error("Invalid account connection authorization");
  }
  return `${origin.protocol === "https:" ? "wss" : "ws"}://${origin.host}/api/account-rpc`;
}

function renewIn(expiresAt: number, now: number): number | null {
  const remaining = expiresAt - now;
  if (remaining <= 0) return null;
  const margin = Math.min(30_000, Math.max(1, Math.floor(remaining / 3)));
  return Math.max(1, remaining - margin);
}

function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}

export class AccountTransport implements Transport {
  onMessage: ((text: string) => void) | null = null;
  onClose: ((reason: string) => void) | null = null;
  /** The computer's public origin once connected. */
  origin = "";
  private url = "";
  private controller = new RpcClient();
  private controllerTransport: Transport | null = null;
  private socket: WebSocket | null = null;
  private until = 0;
  private timers: ReturnType<typeof setTimeout>[] = [];
  private pong: string | null = null;
  private renewal: Grant | null = null;
  private failures = 0;
  private closed = false;
  private replies: ((text: string) => void) | null = null;

  constructor(
    private readonly target: AccountTarget,
    private readonly base: () => Transport | Promise<Transport>,
  ) {}

  async open(): Promise<void> {
    try {
      await this.connect();
    } catch (error) {
      this.teardown();
      throw error;
    }
  }

  private async connect() {
    const base = await this.base();
    await base.open();
    this.controllerTransport = base;
    base.onClose = (reason) => this.lose(`The controller connection closed: ${reason}`);
    this.controller.attach(base);
    await this.watchAccount();
    const grant = await this.grant();
    const url = socketUrl(grant);
    this.url = url;
    const socket = await withTimeout(this.dial(url), 5_000, "Could not reach the account computer");
    this.socket = socket;
    socket.send(JSON.stringify({ protocol: PROTOCOL, connectToken: grant.connectToken }));
    this.until = this.acknowledge(await this.next("Account authentication timed out"), grant, false);
    socket.send(JSON.stringify({ id: 0, method: M.ENGINE_INFO, params: {} }));
    const info = JSON.parse(await this.next("Computer identity verification timed out")) as { id?: number; ok?: { deviceId?: string } };
    if (info.id !== 0 || info.ok?.deviceId !== this.target.environmentId) {
      throw new ConnectionBlocked({ kind: "accountRequired", message: "The selected computer returned another identity." });
    }
    this.origin = grant.origin.replace(/\/$/, "");
    await withTimeout(
      this.controller.call(M.REMEMBER_ACCOUNT_ROUTE, {
        expectedSubject: this.target.expectedSubject,
        environmentId: this.target.environmentId,
        label: this.target.label,
        origin: this.origin,
      }),
      2_000,
      "",
    ).catch(() => {});
    this.replies = null;
    this.schedule(PING_MS, () => this.ping());
    this.scheduleRenewal(renewIn(this.until, Date.now()) ?? 1);
  }

  /** Account changes on the controller retire this tab's access. */
  private watchAccount(): Promise<void> {
    return new Promise((resolve, reject) => {
      let first = true;
      this.controller.subscribe<{ user?: { id?: string } }>(
        M.CLOUD_ACCOUNT_STATUS,
        {},
        (status) => {
          const matches = status?.user?.id === this.target.expectedSubject;
          if (first) {
            first = false;
            if (matches) resolve();
            else
              reject(
                new ConnectionBlocked({
                  kind: "accountRequired",
                  message: status?.user ? "The controller account changed. Select the computer again." : "Sign in on the controller before selecting this computer.",
                }),
              );
          } else if (!matches) {
            this.lose("The controller account changed. Return to the controller and select the computer again.");
          }
        },
        (error) => {
          if (first) reject(error ?? new Error("Controller account status ended"));
          else this.lose("The controller account connection ended");
        },
      );
    });
  }

  private async grant(): Promise<Grant> {
    let value: Grant;
    try {
      value = await withTimeout(
        this.controller.call<Grant>(M.BOOTSTRAP_ACCOUNT_CONNECTION, {
          environmentId: this.target.environmentId,
          expectedSubject: this.target.expectedSubject,
        }),
        60_000,
        "Account request timed out",
      );
    } catch (error) {
      if (error instanceof RpcError && error.message === REJECTED) {
        throw new ConnectionBlocked({
          kind: "accountRequired",
          message: "The computer rejected account access. Check access and app versions before selecting it again.",
        });
      }
      throw error;
    }
    if (value?.environmentId !== this.target.environmentId) throw new Error("The selected computer identity changed");
    return value;
  }

  private dial(url: string): Promise<WebSocket> {
    return new Promise((resolve, reject) => {
      // The subprotocol opts only this grant-authenticated endpoint into cross-origin use; no cookie is involved.
      const socket = new WebSocket(url, PROTOCOL);
      let opened = false;
      socket.onopen = () => {
        opened = true;
        resolve(socket);
      };
      socket.onmessage = (event: MessageEvent) => {
        if (typeof event.data !== "string") return this.lose("Unexpected account transport frame");
        this.receive(event.data);
      };
      socket.onclose = socket.onerror = () => {
        if (!opened) reject(new Error("Could not reach the account computer"));
        else this.lose("Account computer connection closed");
      };
    });
  }

  private next(timeout: string): Promise<string> {
    return withTimeout(new Promise<string>((resolve) => (this.replies = resolve)), REPLY_MS, timeout);
  }

  private acknowledge(text: string, grant: Grant, renewed: boolean): number {
    const value = JSON.parse(text) as Record<string, unknown>;
    const until = value.expiresAt;
    if (
      value.authenticated !== true ||
      value.protocol !== PROTOCOL ||
      value.deviceId !== grant.environmentId ||
      (value.renewed === true) !== renewed ||
      typeof until !== "number" ||
      until <= Date.now() ||
      until > grant.expiresAt + 1000
    ) {
      throw new Error("Account authorization was not confirmed");
    }
    return until;
  }

  private receive(text: string) {
    if (this.replies) {
      const reply = this.replies;
      this.replies = null;
      reply(text);
      return;
    }
    if (text.startsWith('{"protocol"') || text.includes('"protocol":')) {
      let control: Record<string, unknown> | null = null;
      try {
        control = JSON.parse(text) as Record<string, unknown>;
      } catch {
        control = null;
      }
      if (control && typeof control.protocol === "string" && control.id === undefined) {
        if (control.protocol !== PROTOCOL) return this.lose("Invalid account control protocol");
        if (typeof control.pong === "string") {
          if (control.pong !== this.pong) return this.lose("Invalid computer heartbeat");
          this.pong = null;
          this.schedule(PING_MS, () => this.ping());
          return;
        }
        const grant = this.renewal;
        if (!grant) return this.lose("Unexpected account acknowledgement");
        this.renewal = null;
        try {
          const until = this.acknowledge(text, grant, true);
          if (until <= this.until) throw new Error("Account authorization did not renew");
          this.until = until;
          this.failures = 0;
          this.scheduleRenewal(renewIn(until, Date.now()) ?? 1);
        } catch (error) {
          this.lose(error instanceof Error ? error.message : "Account authorization did not renew");
        }
        return;
      }
    }
    this.onMessage?.(text);
  }

  private ping() {
    if (this.closed) return;
    const nonce = crypto.randomUUID();
    this.pong = nonce;
    this.write(JSON.stringify({ protocol: PROTOCOL, ping: nonce }));
    this.schedule(REPLY_MS, () => {
      if (this.pong === nonce) this.lose("Account computer stopped responding");
    });
  }

  private scheduleRenewal(ms: number) {
    this.schedule(ms, () => void this.renew());
    this.schedule(Math.max(0, this.until - Date.now()), () => {
      if (Date.now() >= this.until) this.lose("Account connection authorization ended");
    });
  }

  private async renew() {
    if (this.closed || this.renewal) return;
    try {
      const grant = await this.grant();
      if (socketUrl(grant) !== this.url) return this.lose("The computer endpoint changed. Reopen it from Remote access.");
      if (grant.expiresAt > this.until + 1000) {
        this.renewal = grant;
        this.write(JSON.stringify({ protocol: PROTOCOL, connectToken: grant.connectToken }));
        this.schedule(REPLY_MS, () => {
          if (this.renewal === grant) this.lose("Account computer stopped responding");
        });
        return;
      }
    } catch (error) {
      if (error instanceof ConnectionBlocked) return this.lose(error.message);
    }
    this.failures++;
    this.schedule(Math.min(30, 2 ** Math.min(this.failures, 5)) * 1000, () => void this.renew());
  }

  private schedule(ms: number, run: () => void) {
    this.timers.push(setTimeout(run, ms));
  }

  private write(text: string) {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) throw new Error("The account computer is not connected");
    if (socket.bufferedAmount + text.length > 20 * 1024 * 1024) {
      this.lose("Account computer connection is too slow");
      throw new Error("The account computer is not connected");
    }
    socket.send(text);
  }

  send(text: string) {
    if (this.closed || Date.now() >= this.until) throw new Error("Account connection authorization ended");
    this.write(text);
  }

  close() {
    this.lose("Account connection closed");
  }

  private lose(reason: string) {
    if (this.closed) return;
    this.teardown();
    this.onClose?.(reason);
  }

  private teardown() {
    this.closed = true;
    for (const timer of this.timers) clearTimeout(timer);
    this.timers = [];
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      socket.close();
    }
    this.controller.detach("Account connection closed");
    const base = this.controllerTransport;
    this.controllerTransport = null;
    if (base) {
      base.onClose = null;
      base.close();
    }
  }
}
